//! The device's state and the scheduling rules, with no I/O of its own.
//!
//! A bot run makes network calls that take seconds, so it never happens
//! under the lock the admin site uses. The scheduler thread asks
//! [`Service::due`] for jobs (under the lock), runs them with [`execute`]
//! (without it), and reports with [`Service::finish`] (under it again).
//!
//! Rules:
//! - A scheduled run is for a *slot* (07:30 on a given day). The last slot
//!   handled is stored, so a restart never repeats one.
//! - A slot may still run up to the bot's grace period late (after a power
//!   cut, or while retrying); later than that it is recorded as missed.
//! - Failures that might pass (no answer, 5xx, 429) are retried after 1, 5,
//!   15, then every 30 minutes, within the grace period, with the same
//!   Idempotency-Key, so a post that did land is not repeated.
//! - Turning a bot on does not run slots from before it was on.

use crate::auth::{Sessions, Verifier};
use crate::bot::{Bot, BotInfo, Run, RunError};
use crate::mastodon::{normalize_instance, HttpClient, Mastodon};
use crate::openrouter;
use crate::schedule::Schedule;
use crate::settings::Settings;
use crate::store::KvStore;
use crate::tz::iso;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

const MINUTE_MS: u64 = 60_000;
const RETRY_MINUTES: [u64; 4] = [1, 5, 15, 30];
const MAX_EVENTS: usize = 200;
const MAX_TOKEN: usize = 256;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotConfig {
    pub enabled: bool,
    pub instance: String,
    /// The Mastodon API key the bot posts with. Never sent to the browser.
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub slot_ms: u64,
    pub started_ms: u64,
    pub finished_ms: u64,
    pub manual: bool,
    pub ok: bool,
    pub summary: String,
}

/// Stored under `b.<id>`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotRecord {
    pub config: BotConfig,
    pub last_slot_ms: Option<u64>,
    pub last_run: Option<RunRecord>,
    pub last_check: Option<RunRecord>,
    pub runs: u64,
    pub failures: u64,
    /// The bot's own settings (`Bot::settings`), flat strings.
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
    /// The bot's memory between runs (`Run::state`), flat strings.
    #[serde(default)]
    pub state: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct Runtime {
    pub running: bool,
    pub attempt: u32,
    pub retry_at: Option<u64>,
    pub manual_requested: bool,
    pub check_requested: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Scheduled,
    Manual,
    /// Sign in to the server only: does the API key work? (And for a bot
    /// that uses a model: does OpenRouter answer?)
    Check,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub index: usize,
    pub id: &'static str,
    pub kind: JobKind,
    pub slot_ms: u64,
    pub instance: String,
    pub token: String,
    pub settings: Settings,
    pub state: BTreeMap<String, String>,
    pub openrouter: Option<openrouter::Config>,
    pub uses_llm: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub seq: u64,
    pub at: Option<String>,
    pub bot: Option<&'static str>,
    /// `info`, `ok` or `error`.
    pub level: &'static str,
    pub text: String,
}

#[derive(Debug, Default)]
pub struct Activity {
    events: VecDeque<Event>,
    next: u64,
}

impl Activity {
    pub fn add(
        &mut self,
        now: Option<u64>,
        bot: Option<&'static str>,
        level: &'static str,
        text: impl Into<String>,
    ) {
        if self.events.len() >= MAX_EVENTS {
            self.events.pop_front();
        }
        self.next += 1;
        self.events.push_back(Event {
            seq: self.next,
            at: now.map(iso),
            bot,
            level,
            text: text.into(),
        });
    }

    /// Newest first, after `since` (a `seq`), at most `limit`.
    pub fn recent(&self, since: u64, limit: usize) -> Vec<&Event> {
        self.events
            .iter()
            .rev()
            .filter(|e| e.seq > since)
            .take(limit)
            .collect()
    }
}

/// What the admin changes on a bot. `None` leaves a field as it is.
#[derive(Debug, Default, Deserialize)]
pub struct ConfigUpdate {
    pub enabled: Option<bool>,
    pub instance: Option<String>,
    /// An empty string removes the key.
    pub token: Option<String>,
    /// The bot's own settings to change, by key.
    pub settings: Option<BTreeMap<String, String>>,
}

/// The device's OpenRouter settings. `None` leaves a field as it is; an
/// empty key removes it.
#[derive(Debug, Default, Deserialize)]
pub struct OpenRouterUpdate {
    pub key: Option<String>,
    pub model: Option<String>,
}

pub struct Service<S: KvStore> {
    store: S,
    pub bots: Arc<Vec<Box<dyn Bot>>>,
    pub infos: Vec<BotInfo>,
    pub records: Vec<BotRecord>,
    pub runtime: Vec<Runtime>,
    pub activity: Activity,
    pub verifier: Option<Verifier>,
    pub sessions: Sessions,
    pub openrouter: openrouter::Config,
    waiting_for_clock: bool,
}

fn key(id: &str) -> String {
    format!("b.{id}")
}

impl<S: KvStore> Service<S> {
    pub fn open(store: S, bots: Vec<Box<dyn Bot>>) -> Result<Service<S>, String> {
        let infos: Vec<BotInfo> = bots.iter().map(|b| b.info()).collect();
        let mut records = Vec::new();
        for info in &infos {
            let record = match store.get(&key(info.id))? {
                Some(text) => {
                    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", info.id))?
                }
                None => BotRecord {
                    config: BotConfig {
                        instance: info.default_instance.to_string(),
                        ..BotConfig::default()
                    },
                    ..BotRecord::default()
                },
            };
            records.push(record);
        }
        let verifier = match store.get("admin")? {
            Some(text) => Some(serde_json::from_str(&text).map_err(|e| format!("admin: {e}"))?),
            None => None,
        };
        let openrouter = match store.get("i.openrouter")? {
            Some(text) => serde_json::from_str(&text).map_err(|e| format!("openrouter: {e}"))?,
            None => openrouter::Config::default(),
        };
        let mut activity = Activity::default();
        activity.add(
            None,
            None,
            "info",
            format!("Started with {} bots", bots.len()),
        );
        Ok(Service {
            store,
            runtime: vec![Runtime::default(); bots.len()],
            bots: Arc::new(bots),
            infos,
            records,
            activity,
            verifier,
            sessions: Sessions::default(),
            openrouter,
            waiting_for_clock: false,
        })
    }

    pub fn index(&self, id: &str) -> Option<usize> {
        self.infos.iter().position(|i| i.id == id)
    }

    fn save(&mut self, i: usize) -> Result<(), String> {
        let text = serde_json::to_string(&self.records[i]).map_err(|e| e.to_string())?;
        self.store.put(&key(self.infos[i].id), &text)
    }

    /// Bot `i`'s settings with their values.
    pub fn settings(&self, i: usize) -> Settings {
        Settings::new(self.bots[i].settings(), self.records[i].settings.clone())
    }

    /// When bot `i` runs, from its settings.
    pub fn schedule(&self, i: usize) -> Schedule {
        self.bots[i].schedule(&self.settings(i))
    }

    pub fn set_openrouter(&mut self, update: OpenRouterUpdate) -> Result<(), String> {
        let mut config = self.openrouter.clone();
        if let Some(key) = update.key {
            let key = key.trim();
            if key.len() > MAX_TOKEN || key.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err("That does not look like an OpenRouter API key".into());
            }
            config.key = key.to_string();
        }
        if let Some(model) = update.model {
            let model = model.trim();
            if model.len() > 100 || model.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err("A model id looks like google/gemma-4-31b-it".into());
            }
            config.model = model.to_string();
        }
        let text = serde_json::to_string(&config).map_err(|e| e.to_string())?;
        self.store.put("i.openrouter", &text)?;
        self.openrouter = config;
        self.activity
            .add(None, None, "info", "OpenRouter settings saved");
        Ok(())
    }

    pub fn configured(&self, i: usize) -> bool {
        let c = &self.records[i].config;
        !c.instance.is_empty() && !c.token.is_empty()
    }

    /// Set the admin password: first visit only, or replacing the old one.
    pub fn set_password(&mut self, password: &str) -> Result<(), String> {
        let verifier = Verifier::new(password)?;
        let text = serde_json::to_string(&verifier).map_err(|e| e.to_string())?;
        self.store.put("admin", &text)?;
        self.verifier = Some(verifier);
        self.sessions.end_all();
        Ok(())
    }

    pub fn configure(
        &mut self,
        i: usize,
        update: ConfigUpdate,
        now: Option<u64>,
    ) -> Result<(), String> {
        let mut config = self.records[i].config.clone();
        if let Some(instance) = update.instance {
            config.instance = normalize_instance(&instance)?;
        }
        if let Some(token) = update.token {
            let token = token.trim();
            if token.len() > MAX_TOKEN || token.chars().any(|c| c.is_whitespace() || c.is_control())
            {
                return Err("That does not look like an API key".into());
            }
            config.token = token.to_string();
        }
        let turned_on = update.enabled == Some(true) && !config.enabled;
        if let Some(enabled) = update.enabled {
            config.enabled = enabled;
        }
        if config.enabled && (config.instance.is_empty() || config.token.is_empty()) {
            return Err("Choose a server and an API key before turning the bot on".into());
        }
        let id = self.infos[i].id;
        let mut record = self.records[i].clone();
        record.config = config;
        let before = self.schedule(i);
        if let Some(changes) = &update.settings {
            record.settings = self.settings(i).merged(changes)?;
        }
        let after = Settings::new(self.bots[i].settings(), record.settings.clone());
        let schedule = self.bots[i].schedule(&after);
        schedule.check()?;
        if turned_on || (record.config.enabled && schedule != before) {
            // Only slots from now on: turning on at 07:40 (or moving the
            // time to before now) does not run a slot that has passed.
            record.last_slot_ms = now.and_then(|n| schedule.latest_at(n));
            self.runtime[i].retry_at = None;
            self.runtime[i].attempt = 0;
        }
        let previous = std::mem::replace(&mut self.records[i], record);
        if let Err(e) = self.save(i) {
            self.records[i] = previous;
            return Err(e);
        }
        let state = if self.records[i].config.enabled {
            "on"
        } else {
            "off"
        };
        self.activity.add(
            now,
            Some(id),
            "info",
            format!("Settings saved; the bot is {state}"),
        );
        Ok(())
    }

    pub fn request(&mut self, i: usize, kind: JobKind, now: Option<u64>) -> Result<(), String> {
        if !self.configured(i) {
            return Err("Choose a server and an API key first".into());
        }
        let what = match kind {
            JobKind::Check => {
                self.runtime[i].check_requested = true;
                "Checking the API key"
            }
            _ => {
                self.runtime[i].manual_requested = true;
                "Run requested"
            }
        };
        self.activity.add(now, Some(self.infos[i].id), "info", what);
        Ok(())
    }

    /// The next time this bot will run, if it will.
    pub fn next_run(&self, i: usize, now: u64) -> Option<u64> {
        if !self.records[i].config.enabled || !self.configured(i) {
            return None;
        }
        if let Some(retry) = self.runtime[i].retry_at {
            return Some(retry);
        }
        let after = self.records[i].last_slot_ms.unwrap_or(now).max(now);
        self.schedule(i).next_after(after)
    }

    /// Jobs to start now; each bot is marked running until [`finish`].
    pub fn due(&mut self, now: Option<u64>) -> Vec<Job> {
        let mut jobs = Vec::new();
        for i in 0..self.infos.len() {
            if self.runtime[i].running || !self.configured(i) {
                continue;
            }
            let Some(now) = now else {
                continue;
            };
            let next = if std::mem::take(&mut self.runtime[i].check_requested) {
                Some((JobKind::Check, now))
            } else if std::mem::take(&mut self.runtime[i].manual_requested) {
                Some((JobKind::Manual, now))
            } else if self.records[i].config.enabled {
                self.scheduled(i, now)
                    .map(|slot| (JobKind::Scheduled, slot))
            } else {
                None
            };
            if let Some((kind, slot_ms)) = next {
                self.runtime[i].running = true;
                jobs.push(Job {
                    index: i,
                    id: self.infos[i].id,
                    kind,
                    slot_ms,
                    instance: self.records[i].config.instance.clone(),
                    token: self.records[i].config.token.clone(),
                    settings: self.settings(i),
                    state: self.records[i].state.clone(),
                    openrouter: Some(self.openrouter.clone()).filter(|c| !c.key.is_empty()),
                    uses_llm: self.infos[i].uses_llm,
                });
            }
        }
        if now.is_none() && !self.waiting_for_clock && self.records.iter().any(|r| r.config.enabled)
        {
            self.waiting_for_clock = true;
            self.activity.add(
                None,
                None,
                "info",
                "Waiting for the clock (internet time) before running bots",
            );
        }
        if now.is_some() {
            self.waiting_for_clock = false;
        }
        jobs
    }

    /// The slot to run now, if any; records slots missed beyond the grace.
    fn scheduled(&mut self, i: usize, now: u64) -> Option<u64> {
        let slot = self.schedule(i).latest_at(now)?;
        let info = &self.infos[i];
        let last = match self.records[i].last_slot_ms {
            Some(last) => last,
            None => {
                // Turned on before the clock was set: start from here.
                self.records[i].last_slot_ms = Some(slot);
                let _ = self.save(i);
                return None;
            }
        };
        if slot <= last {
            return None;
        }
        let grace = u64::from(info.grace_minutes) * MINUTE_MS;
        if now - slot > grace {
            let id = info.id;
            self.records[i].last_slot_ms = Some(slot);
            self.runtime[i].retry_at = None;
            self.runtime[i].attempt = 0;
            let _ = self.save(i);
            self.activity.add(
                Some(now),
                Some(id),
                "error",
                format!(
                    "Missed the run due at {} (the device was off, or it kept failing)",
                    iso(slot)
                ),
            );
            return None;
        }
        match self.runtime[i].retry_at {
            Some(at) if at > now => None,
            _ => Some(slot),
        }
    }

    pub fn finish(&mut self, job: &Job, started: u64, now: u64, outcome: Outcome) {
        let i = job.index;
        self.runtime[i].running = false;
        for line in &outcome.log {
            self.activity
                .add(Some(now), Some(job.id), "info", line.clone());
        }
        let (ok, summary) = match &outcome.result {
            Ok(summary) => (true, summary.clone()),
            Err(e) => (false, e.message.clone()),
        };
        let record = RunRecord {
            slot_ms: job.slot_ms,
            started_ms: started,
            finished_ms: now,
            manual: job.kind != JobKind::Scheduled,
            ok,
            summary: summary.clone(),
        };
        self.activity.add(
            Some(now),
            Some(job.id),
            if ok { "ok" } else { "error" },
            summary,
        );
        if job.kind == JobKind::Check {
            self.records[i].last_check = Some(record);
            let _ = self.save(i);
            return;
        }
        if let Some(state) = outcome.state {
            self.records[i].state = state;
        }
        self.records[i].runs += 1;
        self.records[i].failures += u64::from(!ok);
        self.records[i].last_run = Some(record);
        if job.kind == JobKind::Scheduled {
            match &outcome.result {
                Ok(_) => self.settle(i, job.slot_ms),
                Err(e) => {
                    let attempt = self.runtime[i].attempt as usize;
                    let wait = RETRY_MINUTES[attempt.min(RETRY_MINUTES.len() - 1)] * MINUTE_MS;
                    let grace = u64::from(self.infos[i].grace_minutes) * MINUTE_MS;
                    if e.retryable && now + wait <= job.slot_ms + grace {
                        self.runtime[i].attempt += 1;
                        self.runtime[i].retry_at = Some(now + wait);
                        self.activity.add(
                            Some(now),
                            Some(job.id),
                            "info",
                            format!("Will try again at {}", iso(now + wait)),
                        );
                    } else {
                        self.settle(i, job.slot_ms);
                        self.activity
                            .add(Some(now), Some(job.id), "error", "Gave up on this run");
                    }
                }
            }
        }
        let _ = self.save(i);
    }

    fn settle(&mut self, i: usize, slot: u64) {
        self.records[i].last_slot_ms = Some(slot);
        self.runtime[i].retry_at = None;
        self.runtime[i].attempt = 0;
    }
}

/// The result of one job.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub result: Result<String, RunError>,
    pub log: Vec<String>,
    /// The bot's memory after the run, kept even when it failed (a reply
    /// bot's cursor has moved past the mentions it did answer).
    pub state: Option<BTreeMap<String, String>>,
}

/// Run one job, without the service lock.
pub fn execute(
    bots: &[Box<dyn Bot>],
    job: &Job,
    http: &mut dyn HttpClient,
    now_ms: u64,
) -> Outcome {
    if job.kind == JobKind::Check {
        return Outcome {
            result: check(job, http),
            log: Vec::new(),
            state: None,
        };
    }
    let mut run = Run::new(
        job.id,
        job.slot_ms,
        now_ms,
        &job.settings,
        http,
        &job.instance,
        &job.token,
        job.openrouter.as_ref(),
    );
    run.manual = job.kind == JobKind::Manual;
    run.state = job.state.clone();
    let result = bots[job.index].run(&mut run);
    Outcome {
        result,
        log: run.log,
        state: Some(run.state),
    }
}

/// Does the API key work, and (for a bot that uses a model) does
/// OpenRouter answer? One tiny completion, a fraction of a cent.
fn check(job: &Job, http: &mut dyn HttpClient) -> Result<String, RunError> {
    let acct = Mastodon::new(http, &job.instance, &job.token).verify_credentials()?;
    let mastodon = format!("The API key works: {acct} on {}", job.instance);
    if !job.uses_llm {
        return Ok(mastodon);
    }
    let config = job.openrouter.as_ref().ok_or_else(|| {
        RunError::permanent(format!(
            "{mastodon}; add an OpenRouter API key on the Device page"
        ))
    })?;
    let model = match job.settings.get("model") {
        "" => config.model(),
        m => m,
    };
    let mut llm = openrouter::OpenRouter {
        http,
        key: &config.key,
    };
    let prompt = openrouter::Prompt {
        system: "",
        user: "Reply with the single word: ready",
        model,
        max_tokens: 16,
    };
    llm.complete(&prompt)
        .map(|_| format!("{mastodon}; OpenRouter answers ({model})"))
        .map_err(|e| RunError {
            message: format!("{mastodon}, but {}", e.message),
            retryable: e.retryable,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mastodon::testing::Scripted;
    use crate::store::MemStore;
    use crate::tz::days_from_civil;
    use serde_json::json;

    fn utc(y: i64, mo: u32, d: u32, h: u32, mi: u32) -> u64 {
        ((days_from_civil(y, mo, d) * 86_400 + i64::from(h) * 3600 + i64::from(mi) * 60) * 1000)
            as u64
    }

    // 07:30 EDT on Saturday 26 September 2026.
    fn slot() -> u64 {
        utc(2026, 9, 26, 11, 30)
    }

    fn service() -> Service<MemStore> {
        let mut s = Service::open(MemStore::default(), crate::bots::all()).unwrap();
        let i = s.index("good_morning").unwrap();
        s.configure(
            i,
            ConfigUpdate {
                enabled: Some(true),
                instance: Some("mastomini.local".into()),
                token: Some("KEY".into()),
                ..Default::default()
            },
            Some(slot() - 3_600_000),
        )
        .unwrap();
        s
    }

    fn step(s: &mut Service<MemStore>, http: &mut Scripted, now: u64) -> Vec<Outcome> {
        let jobs = s.due(Some(now));
        jobs.iter()
            .map(|job| {
                let outcome = execute(&s.bots.clone(), job, http, now);
                s.finish(job, now, now, outcome.clone());
                outcome
            })
            .collect()
    }

    #[test]
    fn runs_once_per_slot_and_survives_a_restart() {
        let mut s = service();
        let mut http = Scripted::default();
        assert!(step(&mut s, &mut http, slot() - 1).is_empty());
        http.answer(200, json!({"id": "1", "url": "u1"}));
        let done = step(&mut s, &mut http, slot() + 500);
        assert_eq!(done[0].result.as_ref().unwrap(), "Posted u1");
        assert!(step(&mut s, &mut http, slot() + 60_000).is_empty());
        assert_eq!(
            s.next_run(0, slot() + 60_000),
            Some(utc(2026, 9, 27, 11, 30))
        );
        // Restart: the slot is remembered.
        let store = std::mem::take(&mut s.store);
        let mut s = Service::open(store, crate::bots::all()).unwrap();
        assert!(step(&mut s, &mut http, slot() + 120_000).is_empty());
        assert_eq!(s.records[0].runs, 1);
    }

    #[test]
    fn retries_with_the_same_key_then_gives_up_at_the_grace() {
        let mut s = service();
        let mut http = Scripted::default();
        let sent = http.sent.clone();
        http.fail("timed out");
        step(&mut s, &mut http, slot());
        assert_eq!(s.runtime[0].retry_at, Some(slot() + MINUTE_MS));
        assert!(step(&mut s, &mut http, slot() + 30_000).is_empty());
        http.answer(503, json!({"error": "busy"}));
        step(&mut s, &mut http, slot() + MINUTE_MS);
        let keys: Vec<String> = sent
            .lock()
            .unwrap()
            .iter()
            .map(|r| {
                r.headers
                    .iter()
                    .find(|(k, _)| k == "Idempotency-Key")
                    .unwrap()
                    .1
                    .clone()
            })
            .collect();
        assert_eq!(keys[0], keys[1]);
        // A rejected key is not retried.
        http.answer(401, json!({"error": "The access token is invalid"}));
        step(&mut s, &mut http, slot() + 6 * MINUTE_MS);
        assert_eq!(s.runtime[0].retry_at, None);
        assert_eq!(s.records[0].last_slot_ms, Some(slot()));
        assert_eq!(s.records[0].failures, 3);
    }

    #[test]
    fn a_slot_past_its_grace_is_missed_not_run() {
        let mut s = service();
        let mut http = Scripted::default();
        // The device was off from before 07:30 until noon.
        assert!(step(&mut s, &mut http, slot() + 150 * MINUTE_MS).is_empty());
        assert_eq!(s.records[0].last_slot_ms, Some(slot()));
        assert!(s
            .activity
            .recent(0, 5)
            .iter()
            .any(|e| e.text.starts_with("Missed")));
        assert!(http.sent.lock().unwrap().is_empty());
    }

    #[test]
    fn turning_on_late_waits_for_tomorrow_and_manual_runs_do_not_count() {
        let mut s = Service::open(MemStore::default(), crate::bots::all()).unwrap();
        let mut http = Scripted::default();
        assert!(s.request(0, JobKind::Manual, Some(slot())).is_err());
        let update = ConfigUpdate {
            enabled: Some(true),
            instance: None,
            token: Some("KEY".into()),
            ..Default::default()
        };
        s.configure(0, update, Some(slot() + 10 * MINUTE_MS))
            .unwrap();
        assert_eq!(s.records[0].config.instance, "https://mastomini.local");
        assert!(step(&mut s, &mut http, slot() + 11 * MINUTE_MS).is_empty());
        s.request(0, JobKind::Manual, Some(slot() + 12 * MINUTE_MS))
            .unwrap();
        http.answer(200, json!({"id": "2", "url": "u2"}));
        let done = step(&mut s, &mut http, slot() + 12 * MINUTE_MS);
        assert!(done[0].result.is_ok());
        let key = &http.sent.lock().unwrap()[0].headers;
        assert!(key
            .iter()
            .any(|(k, v)| k == "Idempotency-Key" && v.contains(":manual:")));
        assert_eq!(
            s.next_run(0, slot() + 13 * MINUTE_MS),
            Some(utc(2026, 9, 27, 11, 30))
        );
    }

    #[test]
    fn checks_and_settings() {
        let mut s = service();
        let mut http = Scripted::default();
        s.request(0, JobKind::Check, Some(slot() - 10)).unwrap();
        http.answer(200, json!({"acct": "morningbot"}));
        let done = step(&mut s, &mut http, slot() - 10);
        assert_eq!(
            done[0].result.as_ref().unwrap(),
            "The API key works: @morningbot on https://mastomini.local"
        );
        assert!(s.records[0].last_check.as_ref().unwrap().ok);
        let bad = ConfigUpdate {
            token: Some("has space".into()),
            ..Default::default()
        };
        assert!(s.configure(0, bad, None).is_err());
        let off = ConfigUpdate {
            token: Some(String::new()),
            ..Default::default()
        };
        assert!(s.configure(0, off, None).is_err(), "on without a key");
        let off = ConfigUpdate {
            enabled: Some(false),
            token: Some(String::new()),
            ..Default::default()
        };
        s.configure(0, off, None).unwrap();
        assert!(s.due(Some(slot())).is_empty());
    }

    #[test]
    fn memory_survives_failures_and_moving_the_time_skips_passed_slots() {
        let mut s = service();
        let i = s.index("good_morning").unwrap();
        // Moving 07:30 to 07:00 at 07:10 does not post "late" at 07:10.
        let move_time = ConfigUpdate {
            settings: Some([("time".to_string(), "07:00".to_string())].into()),
            ..Default::default()
        };
        s.configure(i, move_time, Some(slot() - 20 * MINUTE_MS))
            .unwrap();
        assert_eq!(s.schedule(i).describe(), "Every day at 07:00 US Eastern");
        let mut http = Scripted::default();
        assert!(step(&mut s, &mut http, slot() - 19 * MINUTE_MS).is_empty());
        // A run's memory is kept even when it fails.
        let jobs = s
            .due(Some(slot() + DAY_MS_TEST))
            .into_iter()
            .collect::<Vec<_>>();
        let job = &jobs[0];
        let outcome = Outcome {
            result: Err(RunError::permanent("nope")),
            log: Vec::new(),
            state: Some([("cursor".to_string(), "52".to_string())].into()),
        };
        s.finish(job, 0, slot() + DAY_MS_TEST, outcome);
        let store = std::mem::take(&mut s.store);
        let s = Service::open(store, crate::bots::all()).unwrap();
        assert_eq!(s.records[i].state["cursor"], "52");
        assert_eq!(s.records[i].settings["time"], "07:00");
    }

    const DAY_MS_TEST: u64 = 24 * 60 * MINUTE_MS;

    #[test]
    fn waits_for_the_clock() {
        let mut s = service();
        assert!(s.due(None).is_empty());
        assert!(s.activity.recent(0, 1)[0]
            .text
            .starts_with("Waiting for the clock"));
    }
}
