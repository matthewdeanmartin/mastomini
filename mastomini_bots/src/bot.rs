//! What a bot is. Bots are Rust code compiled into the firmware: implement
//! [`Bot`] in a file under `src/bots/` and add it to `src/bots/mod.rs`.
//!
//! A bot says who it is ([`BotInfo`]), what the admin can set
//! ([`Bot::settings`], drawn as a form by the admin app), when it runs
//! ([`Bot::schedule`], usually from its settings), and does one run at a
//! time ([`Bot::run`]). The scheduler decides when; the admin site chooses
//! which Mastodon server and account (an API key) each bot uses.
//!
//! Keep it light. Settings and a bot's memory between runs ([`Run::state`])
//! are flat string maps. Text for a model is plain lines (`@alice: text`),
//! not JSON. Ask models for plain text, not structured output. JSON is only
//! what the Mastodon and OpenRouter APIs speak.

use crate::mastodon::{HttpClient, Mastodon, MastodonError, NewStatus, Posted};
use crate::openrouter::{self, OpenRouter};
use crate::schedule::Schedule;
use crate::settings::{Setting, Settings};
use crate::tz::{Local, Tz};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct BotInfo {
    /// Stable, short (at most 13 of `a-z0-9_`): it names the bot's stored
    /// settings. Never rename one that is in use.
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// When it runs if [`Bot::schedule`] is not overridden.
    pub schedule: Schedule,
    /// How late a scheduled run may still happen: after a power cut, a slow
    /// server, or while retrying. A morning greeting at noon is wrong.
    pub grace_minutes: u32,
    /// Filled in on the admin page until the admin chooses otherwise.
    pub default_instance: &'static str,
    /// Needs the device's OpenRouter key (Device page).
    pub uses_llm: bool,
}

/// Why a run failed, and whether trying again later might help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunError {
    pub message: String,
    pub retryable: bool,
}

impl RunError {
    /// A failure retrying will not fix (bad configuration, rejected post).
    pub fn permanent(message: impl Into<String>) -> RunError {
        RunError {
            message: message.into(),
            retryable: false,
        }
    }
}

impl From<MastodonError> for RunError {
    fn from(e: MastodonError) -> RunError {
        RunError {
            retryable: e.retryable(),
            message: e.to_string(),
        }
    }
}

pub trait Bot: Send + Sync {
    fn info(&self) -> BotInfo;

    /// What the admin can set. Values reach [`Run::settings`].
    fn settings(&self) -> Vec<Setting> {
        Vec::new()
    }

    /// When it runs, given its settings.
    fn schedule(&self, _settings: &Settings) -> Schedule {
        self.info().schedule
    }

    /// Do one run. `Ok` carries a one-line summary for the activity log.
    fn run(&self, run: &mut Run<'_>) -> Result<String, RunError>;
}

/// Everything a run may use.
pub struct Run<'a> {
    pub bot_id: &'static str,
    /// The scheduled instant this run is for (for a manual run, now). Use it
    /// rather than `now_ms` for what the post says: a retry at 07:41 of the
    /// 07:30 run is still the 07:30 run.
    pub slot_ms: u64,
    pub now_ms: u64,
    pub manual: bool,
    pub settings: &'a Settings,
    /// The bot's memory between runs (a cursor, a counter): flat strings,
    /// saved when the run ends, even if it fails.
    pub state: BTreeMap<String, String>,
    pub(crate) http: &'a mut dyn HttpClient,
    pub(crate) instance: &'a str,
    pub(crate) token: &'a str,
    pub(crate) openrouter: Option<&'a openrouter::Config>,
    pub(crate) log: Vec<String>,
}

impl<'a> Run<'a> {
    /// A run for tests and tools; the scheduler builds its own.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        bot_id: &'static str,
        slot_ms: u64,
        now_ms: u64,
        settings: &'a Settings,
        http: &'a mut dyn HttpClient,
        instance: &'a str,
        token: &'a str,
        openrouter: Option<&'a openrouter::Config>,
    ) -> Run<'a> {
        Run {
            bot_id,
            slot_ms,
            now_ms,
            manual: false,
            settings,
            state: BTreeMap::new(),
            http,
            instance,
            token,
            openrouter,
            log: Vec::new(),
        }
    }

    /// A line in the admin page's activity log.
    pub fn log(&mut self, line: impl Into<String>) {
        if self.log.len() < 20 {
            self.log.push(line.into());
        }
    }

    pub fn mastodon(&mut self) -> Mastodon<'_> {
        Mastodon::new(&mut *self.http, self.instance, self.token)
    }

    /// The device's OpenRouter connection.
    pub fn llm(&mut self) -> Result<OpenRouter<'_>, RunError> {
        let key = self
            .openrouter
            .map(|c| c.key.as_str())
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                RunError::permanent("Add an OpenRouter API key on the Device page first")
            })?;
        Ok(OpenRouter {
            http: &mut *self.http,
            key,
        })
    }

    /// The device's default model.
    pub fn default_model(&self) -> &str {
        self.openrouter
            .map_or(openrouter::DEFAULT_MODEL, |c| c.model())
    }

    /// The same for every attempt at this slot, different for every slot.
    pub fn idempotency_key(&self) -> String {
        let kind = if self.manual { "manual" } else { "slot" };
        format!("mastomini-bots:{}:{kind}:{}", self.bot_id, self.slot_ms)
    }

    /// The slot's local time in a POSIX zone.
    pub fn local(&self, tz: &str) -> Result<Local, RunError> {
        Ok(Tz::parse(tz)
            .map_err(RunError::permanent)?
            .local(self.slot_ms))
    }

    /// Post, safe to retry: one post per slot.
    pub fn post(&mut self, status: &NewStatus) -> Result<Posted, RunError> {
        let key = self.idempotency_key();
        Ok(self.mastodon().post_status(status, &key)?)
    }

    /// Post one of several things in a run (replies): `what` makes the
    /// key unique, e.g. the id of the post being answered.
    pub fn post_keyed(&mut self, status: &NewStatus, what: &str) -> Result<Posted, RunError> {
        let key = format!("mastomini-bots:{}:{what}", self.bot_id);
        Ok(self.mastodon().post_status(status, &key)?)
    }
}

/// A valid bot id: it becomes part of a storage key (NVS keys are short).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 13
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}
