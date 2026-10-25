//! Turns what each board says about itself into findings a person can act
//! on: what happened, whether it is normal *for this board*, and what to do.
//!
//! - Resets are events, found by the board's boot time moving (now minus
//!   uptime), not by its "Last reset" text, which stays until the next reset.
//!   A power-on reset is an unplug or outage, never a fault; crash and
//!   watchdog are software faults; RST after the bot saw the board down is a
//!   hang. Repeats within seven days escalate.
//! - Heap and Wi-Fi are judged against the board's own recent readings: a
//!   board that always runs at 12 KiB free is not news. The board's own check
//!   must also be off-ok, so a big board dipping a little stays quiet.
//! - Every finding has a rule id; [`Rules`] maps it to an action, with
//!   defaults from `spec/OPS.md` and per-household overrides in settings.
//!
//! The memory is one compact state value, capped so the bot's whole record
//! stays inside one NVS string (4000 bytes) next to its settings.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const KEY: &str = "sre_memory";
const MAX_BYTES: usize = 1536;
const MINUTE: u64 = 60_000;
const DAY_MIN: u32 = 24 * 60;
const WEEK_MIN: u32 = 7 * DAY_MIN;
/// Readings kept per board, newest last; the baseline is their median.
const SAMPLES: usize = 5;
/// Readings needed before "lower than usual" means anything.
const LEARNING: usize = 3;
const RESETS: usize = 6;
const OPEN: usize = 24;
/// Boot times this close are the same boot (clock and request jitter).
const SAME_BOOT_MIN: u32 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    /// Normal; not posted.
    Ignore,
    /// A reply under the digest.
    Digest,
    /// Needs a person soon (sprint 2 sends it out).
    Notify,
    /// A bug or other work item (sprint 4 files it).
    Issue,
}

impl Action {
    fn parse(s: &str) -> Option<Action> {
        match s.trim() {
            "ignore" => Some(Action::Ignore),
            "digest" => Some(Action::Digest),
            "notify" => Some(Action::Notify),
            "issue" => Some(Action::Issue),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Action::Ignore => "ignore",
            Action::Digest => "digest",
            Action::Notify => "notify",
            Action::Issue => "issue",
        }
    }
}

pub struct Rule {
    pub id: &'static str,
    pub action: Action,
    /// One plain sentence: what to do.
    pub todo: &'static str,
}

const fn rule(id: &'static str, action: Action, todo: &'static str) -> Rule {
    Rule { id, action, todo }
}

/// The defaults. Ids are stable: overrides and open findings refer to them.
pub const RULES: &[Rule] = &[
    rule("down", Action::Notify, "Check its power and Wi-Fi; if it is powered and still silent, press RST."),
    rule("no-ops", Action::Digest, "Flash a build with miniframework's ops feature, or remove it from Boards."),
    rule("reset.power", Action::Ignore, "Unplugged or a power cut. Nothing to do."),
    rule("reset.planned", Action::Ignore, "A restart someone or something asked for. Nothing to do."),
    rule("reset.fault", Action::Digest, "A software fault, not an unplug. Once is worth watching; a repeat gets filed."),
    rule("reset.fault.repeat", Action::Issue, "Crashing repeatedly: a firmware bug. Keep the error log with the report."),
    rule("reset.manual", Action::Digest, "Restarted with the RST button. If it was stuck, that is a hang worth reporting."),
    rule("reset.hung", Action::Notify, "It stayed down until RST was pressed: it hangs instead of recovering. A firmware bug."),
    rule("reset.hung.repeat", Action::Issue, "Needs RST to recover again and again: a firmware bug. Keep the error log with the report."),
    rule("reset.brownout", Action::Notify, "Its supply sagged: replace the cable or the power supply."),
    rule("reset.other", Action::Digest, "An unusual reset cause; check the error log."),
    rule("heap.low", Action::Notify, "Free memory is well below this board's usual: watch for a leak. A planned restart buys time."),
    rule("heap.fragmented", Action::Digest, "Its largest free block is well below usual: large requests (TLS) may fail."),
    rule("heap.allocations", Action::Issue, "Allocations failed since the last look: requests are being dropped. A firmware bug or too many connections."),
    rule("heap.learning", Action::Ignore, "Not enough history yet to say whether this is normal for this board."),
    rule("wifi.weak", Action::Digest, "Signal well below its usual: something moved, or the access point changed."),
    rule("check.fail", Action::Notify, "The board reports a failure; follow its runbook."),
    rule("check.warn", Action::Digest, "The board reports a warning; follow its runbook."),
];

/// Defaults plus the household's `rule = action` lines.
pub struct Rules {
    overrides: BTreeMap<String, Action>,
}

impl Rules {
    pub fn parse(text: &str) -> Rules {
        let overrides = text
            .lines()
            .map(|l| l.split('#').next().unwrap_or(""))
            .filter_map(|l| l.split_once('='))
            .map(|(id, a)| (id.trim(), a))
            .filter(|(id, _)| RULES.iter().any(|r| r.id == *id))
            .filter_map(|(id, a)| Some((id.to_string(), Action::parse(a)?)))
            .collect();
        Rules { overrides }
    }

    pub fn action(&self, id: &str) -> Action {
        self.overrides
            .get(id)
            .copied()
            .or_else(|| RULES.iter().find(|r| r.id == id).map(|r| r.action))
            .unwrap_or(Action::Digest)
    }

    pub fn todo(id: &str) -> &'static str {
        RULES.iter().find(|r| r.id == id).map_or("", |r| r.todo)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub rule: &'static str,
    pub action: Action,
    /// What happened, in this board's terms.
    pub text: String,
    /// Resets are one-off events; the rest are conditions that stay open
    /// until they clear.
    pub event: bool,
    /// For conditions: "(new)" or "(day N)", filled by [`track`].
    pub age: String,
}

impl Finding {
    pub fn line(&self) -> String {
        let lead = if self.action >= Action::Notify {
            "Needs you: "
        } else {
            ""
        };
        let age = if self.age.is_empty() {
            String::new()
        } else {
            format!(" {}", self.age)
        };
        format!("{lead}{}{age}. {}", self.text, Rules::todo(self.rule))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BoardMem {
    pub h: String,
    /// Boot time, minutes since the epoch; 0 unknown.
    pub t: u32,
    /// Resets in the last week: minute and kind (c crash, w watchdog,
    /// m RST, g hung, b brownout, p power, s planned, o other).
    pub r: Vec<(u32, char)>,
    /// Failed allocations this boot, at the last look.
    pub a: u32,
    /// Free internal KiB, largest block KiB, RSSI dBm; newest last.
    pub f: Vec<u16>,
    pub l: Vec<u16>,
    pub w: Vec<i16>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Open {
    pub h: String,
    pub r: String,
    /// First seen, minutes since the epoch.
    pub s: u32,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Memory {
    pub b: Vec<BoardMem>,
    pub o: Vec<Open>,
}

impl Memory {
    pub fn read(state: &BTreeMap<String, String>) -> Memory {
        state
            .get(KEY)
            .filter(|s| s.len() <= MAX_BYTES)
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    pub fn board(&self, host: &str) -> Option<&BoardMem> {
        self.b.iter().find(|b| b.h == host)
    }

    /// Serialized within [`MAX_BYTES`]: oldest readings go first, then
    /// resets, then the oldest open findings.
    pub fn write(mut self, state: &mut BTreeMap<String, String>) {
        loop {
            let text = serde_json::to_string(&self).unwrap_or_default();
            if text.len() <= MAX_BYTES {
                state.insert(KEY.into(), text);
                return;
            }
            let long =
                |b: &BoardMem| [b.f.len(), b.l.len(), b.w.len()].into_iter().max() > Some(LEARNING);
            if let Some(b) = self.b.iter_mut().find(|b| long(b)) {
                if b.f.len() > LEARNING {
                    b.f.remove(0);
                }
                if b.l.len() > LEARNING {
                    b.l.remove(0);
                }
                if b.w.len() > LEARNING {
                    b.w.remove(0);
                }
            } else if let Some(b) = self.b.iter_mut().find(|b| !b.r.is_empty()) {
                b.r.remove(0);
            } else if !self.o.is_empty() {
                self.o.remove(0);
            } else {
                self.b.pop();
            }
        }
    }
}

fn minutes(ms: u64) -> u32 {
    (ms / MINUTE).min(u32::MAX as u64) as u32
}

pub(crate) fn median<T: Copy + Ord>(values: &[T]) -> Option<T> {
    if values.len() < LEARNING {
        return None;
    }
    let mut v = values.to_vec();
    v.sort();
    Some(v[v.len() / 2])
}

fn push<T>(ring: &mut Vec<T>, value: T) {
    ring.push(value);
    if ring.len() > SAMPLES {
        ring.remove(0);
    }
}

fn check<'a>(health: &'a Value, id: &str) -> Option<&'a Value> {
    health["checks"]
        .as_array()?
        .iter()
        .find(|c| c["id"].as_str() == Some(id))
}

fn off_ok(c: &Value) -> bool {
    c["status"].as_str().is_some_and(|s| s != "ok")
}

/// What the triage of one answering board found.
pub struct Triage {
    pub findings: Vec<Finding>,
    pub memory: BoardMem,
    /// A reset worth reading the error log for.
    pub evidence: bool,
}

/// One board that answered. `was_down`: the last digest saw it down.
pub fn triage(
    host: &str,
    health: &Value,
    old: Option<&BoardMem>,
    was_down: bool,
    now_ms: u64,
    rules: &Rules,
) -> Triage {
    let now = minutes(now_ms);
    let mut mem = old.cloned().unwrap_or_else(|| BoardMem {
        h: host.into(),
        ..Default::default()
    });
    mem.r.retain(|(at, _)| now.saturating_sub(*at) < WEEK_MIN);
    let mut findings = Vec::new();
    let mut add = |rule: &'static str, text: String, event: bool| {
        let action = rules.action(rule);
        findings.push(Finding {
            rule,
            action,
            text,
            event,
            age: String::new(),
        });
    };

    // Resets: by the boot time moving.
    let mut evidence = false;
    let uptime = health["uptime_ms"].as_u64().unwrap_or(0);
    let boot = if uptime > 0 {
        minutes(now_ms.saturating_sub(uptime))
    } else {
        0
    };
    let new_boot = boot > 0
        && match old {
            Some(m) if m.t > 0 => boot.abs_diff(m.t) > SAME_BOOT_MIN,
            // First look: only a reset since about the last daily digest is news.
            _ => uptime < 26 * 60 * MINUTE,
        };
    if new_boot {
        mem.a = 0;
        let reason = check(health, "reset")
            .and_then(|c| c["summary"].as_str())
            .and_then(|s| s.strip_prefix("Last reset: "))
            .unwrap_or("unknown")
            .to_string();
        let ago = if uptime >= 2 * 60 * MINUTE {
            format!("{} h ago", uptime / (60 * MINUTE))
        } else {
            format!("{} min ago", uptime / MINUTE)
        };
        let kind = match reason.as_str() {
            "crash" => 'c',
            "watchdog" => 'w',
            "reset pin" if was_down => 'g',
            "reset pin" => 'm',
            "brownout" => 'b',
            "power on" => 'p',
            r if r.starts_with("planned") || r == "software restart" => 's',
            _ => 'o',
        };
        mem.r.push((boot, kind));
        if mem.r.len() > RESETS {
            mem.r.remove(0);
        }
        let count = |kinds: &[char]| mem.r.iter().filter(|(_, k)| kinds.contains(k)).count();
        match kind {
            'c' | 'w' => {
                evidence = true;
                let n = count(&['c', 'w']);
                let what = if kind == 'c' {
                    "crashed"
                } else {
                    "was reset by its watchdog"
                };
                if n >= 2 {
                    add(
                        "reset.fault.repeat",
                        format!("{what} {ago}: {n} faults in 7 days"),
                        true,
                    );
                } else {
                    add("reset.fault", format!("{what} {ago}"), true);
                }
            }
            'g' | 'm' => {
                evidence = true;
                let n = count(&['g', 'm']);
                if n >= 2 {
                    add(
                        "reset.hung.repeat",
                        format!("needed RST again {ago}: {n} times in 7 days"),
                        true,
                    );
                } else if kind == 'g' {
                    add(
                        "reset.hung",
                        format!("was down at the last digest and came back by RST {ago}"),
                        true,
                    );
                } else {
                    add("reset.manual", format!("restarted by RST {ago}"), true);
                }
            }
            'b' => {
                evidence = true;
                add("reset.brownout", format!("reset by a brownout {ago}"), true);
            }
            'p' => add(
                "reset.power",
                format!("restarted from power-on {ago}"),
                true,
            ),
            's' => add("reset.planned", format!("restarted ({reason}) {ago}"), true),
            _ => {
                evidence = true;
                add("reset.other", format!("reset ({reason}) {ago}"), true);
            }
        }
    }
    if boot > 0 {
        mem.t = boot;
    }

    // Memory and signal, against this board's own readings.
    let value = |id: &str| check(health, id).and_then(|c| c["value"].as_f64());
    let heap = check(health, "heap.internal");
    let free = value("heap.internal").map(|v| (v / 1024.0).round().clamp(0.0, 65_535.0) as u16);
    if let (Some(c), Some(kib)) = (heap, free) {
        match median(&mem.f) {
            Some(usual) if off_ok(c) && (kib as u32) * 4 < (usual as u32) * 3 => add(
                "heap.low",
                format!("{kib} KiB free, usually {usual} KiB"),
                false,
            ),
            None if off_ok(c) => add(
                "heap.learning",
                format!("{} (no baseline yet)", c["summary"].as_str().unwrap_or("")),
                false,
            ),
            _ => {}
        }
    }
    let largest = value("heap.largest").map(|v| (v / 1024.0).round().clamp(0.0, 65_535.0) as u16);
    if let (Some(c), Some(kib), Some(usual)) =
        (check(health, "heap.largest"), largest, median(&mem.l))
    {
        if off_ok(c) && (kib as u32) * 4 < (usual as u32) * 3 {
            add(
                "heap.fragmented",
                format!("largest free block {kib} KiB, usually {usual} KiB"),
                false,
            );
        }
    }
    if let Some(failed) = value("heap.allocations").map(|v| v.max(0.0) as u32) {
        if failed > mem.a {
            add(
                "heap.allocations",
                format!("{} failed allocations since the last look", failed - mem.a),
                false,
            );
        }
        mem.a = failed;
    }
    let rssi = value("wifi.signal").map(|v| v.round().clamp(-128.0, 0.0) as i16);
    if let (Some(c), Some(dbm), Some(usual)) = (check(health, "wifi.signal"), rssi, median(&mem.w))
    {
        if off_ok(c) && dbm < usual - 6 {
            add(
                "wifi.weak",
                format!("Wi-Fi {dbm} dBm, usually {usual} dBm"),
                false,
            );
        }
    }
    if let Some(f) = free {
        push(&mut mem.f, f);
    }
    if let Some(l) = largest {
        push(&mut mem.l, l);
    }
    if let Some(w) = rssi {
        push(&mut mem.w, w);
    }

    // Everything else the board judges for itself.
    const JUDGED_HERE: &[&str] = &[
        "heap.internal",
        "heap.largest",
        "heap.allocations",
        "wifi.signal",
        "reset",
    ];
    for c in health["checks"].as_array().into_iter().flatten() {
        let id = c["id"].as_str().unwrap_or("?");
        if JUDGED_HERE.contains(&id) || !off_ok(c) {
            continue;
        }
        let rule = if c["status"] == "fail" {
            "check.fail"
        } else {
            "check.warn"
        };
        let runbook = match c["runbook"].as_str() {
            Some(r) if !r.is_empty() => format!(" (runbook {r})"),
            _ => String::new(),
        };
        add(
            rule,
            format!("{id}: {}{runbook}", c["summary"].as_str().unwrap_or("")),
            false,
        );
    }
    findings.sort_by_key(|f| std::cmp::Reverse(f.action));
    Triage {
        findings,
        memory: mem,
        evidence,
    }
}

/// A finding for a board that did not answer, or has no health endpoint.
pub fn unreachable(rule: &'static str, text: String, rules: &Rules) -> Finding {
    Finding {
        rule,
        action: rules.action(rule),
        text,
        event: false,
        age: String::new(),
    }
}

/// Ages each open condition ("(new)", "(day 3)") and returns the ones that
/// cleared since the last digest, as `host rule`.
pub fn track(
    open: &mut Vec<Open>,
    findings: &mut [(String, Vec<Finding>)],
    now_ms: u64,
) -> Vec<String> {
    let now = minutes(now_ms);
    let mut still = Vec::new();
    for (host, list) in findings.iter_mut() {
        for f in list
            .iter_mut()
            .filter(|f| !f.event && f.action > Action::Ignore)
        {
            let first = open
                .iter()
                .find(|o| &o.h == host && o.r == f.rule)
                .map_or(now, |o| o.s);
            let day = now.saturating_sub(first) / DAY_MIN + 1;
            f.age = if now - first < 60 {
                "(new)".into()
            } else {
                format!("(day {day})")
            };
            still.push(Open {
                h: host.clone(),
                r: f.rule.into(),
                s: first,
            });
        }
    }
    let cleared = open
        .iter()
        .filter(|o| !still.iter().any(|s| s.h == o.h && s.r == o.r))
        .map(|o| format!("{} {}", o.h.trim_end_matches(".local"), o.r))
        .collect();
    still.truncate(OPEN);
    *open = still;
    cleared
}

/// One sentence from a board's error log (`/.well-known/incidents`) and
/// log tail (`/api/v1/log`): what the previous boot recorded before it
/// reset. Board text is untrusted; it is only quoted, bounded.
pub fn evidence(incidents: Option<&Value>, log: Option<&Value>) -> String {
    const ROUTINE: &[&str] = &[
        "boot",
        "ready",
        "wifi_up",
        "peer_reset",
        "idle_expired",
        "evicted",
        "worker_recovered",
    ];
    let mut parts = Vec::new();
    let events: Vec<String> = incidents
        .and_then(|i| i["previous"]["events"].as_array())
        .into_iter()
        .flatten()
        .filter(|e| !ROUTINE.contains(&e["kind"].as_str().unwrap_or("")))
        .map(|e| {
            let kind = e["kind"].as_str().unwrap_or("?");
            let count = e["count"].as_u64().unwrap_or(1);
            let bytes = e["bytes"].as_u64().unwrap_or(0);
            let mut s = kind.to_string();
            if count > 1 {
                s.push_str(&format!(" x{count}"));
            }
            if bytes > 0 && matches!(kind, "allocation_failed" | "heap_low") {
                s.push_str(&format!(" ({bytes} B)"));
            }
            s
        })
        .collect();
    if let Some(i) = incidents {
        if events.is_empty() && i["previous"].is_object() {
            parts.push("Its error log shows nothing unusual before the reset".to_string());
        } else if !events.is_empty() {
            let newest: Vec<_> = events.iter().rev().take(4).rev().cloned().collect();
            parts.push(format!("Before the reset: {}", newest.join(", ")));
        } else {
            parts.push("Its error log kept nothing from before the reset".to_string());
        }
    }
    let tail = log
        .and_then(|l| l["previous"].as_str())
        .and_then(|p| p.lines().rev().find(|l| !l.trim().is_empty()))
        .map(|l| {
            let clean: String = l.chars().filter(|c| !c.is_control()).collect();
            crate::text::fit(clean.trim(), 120)
        });
    if let Some(tail) = tail {
        parts.push(format!("last log line \"{tail}\""));
    }
    if parts.is_empty() {
        "Its error log could not be read".into()
    } else {
        parts.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const H: u64 = 3_600_000;
    const NOW: u64 = 1_790_000_000_000;

    fn health(uptime_ms: u64, reset: &str, checks: Value) -> Value {
        let mut all = checks.as_array().cloned().unwrap_or_default();
        all.push(json!({"id": "reset", "status": "ok", "summary": format!("Last reset: {reset}")}));
        json!({"status": "warn", "uptime_ms": uptime_ms, "checks": all})
    }

    fn heap(kib: u32, status: &str) -> Value {
        json!({"id": "heap.internal", "status": status, "summary": format!("{kib} KiB internal RAM free"), "value": kib * 1024, "limit": 16384})
    }

    fn rules() -> Rules {
        Rules::parse("")
    }

    fn rule_ids(t: &Triage) -> Vec<&str> {
        t.findings.iter().map(|f| f.rule).collect()
    }

    #[test]
    fn an_old_crash_is_not_news_but_a_new_one_is() {
        // Up for 10 days: the "Last reset: crash" text is ancient history.
        let old = triage(
            "a.local",
            &health(240 * H, "crash", json!([])),
            None,
            false,
            NOW,
            &rules(),
        );
        assert!(old.findings.is_empty());
        let again = triage(
            "a.local",
            &health(264 * H, "crash", json!([])),
            Some(&old.memory),
            false,
            NOW + 24 * H,
            &rules(),
        );
        assert!(again.findings.is_empty(), "same boot a day later");
        let fresh = triage(
            "a.local",
            &health(H, "crash", json!([])),
            Some(&again.memory),
            false,
            NOW + 48 * H,
            &rules(),
        );
        assert_eq!(rule_ids(&fresh), ["reset.fault"]);
        assert!(fresh.evidence);
        assert!(fresh.findings[0].line().contains("not an unplug"));
    }

    #[test]
    fn power_on_is_an_unplug_and_repeats_of_faults_escalate() {
        let t = triage(
            "a.local",
            &health(H, "power on", json!([])),
            None,
            false,
            NOW,
            &rules(),
        );
        assert_eq!(t.findings[0].action, Action::Ignore);
        assert!(!t.evidence);
        let t = triage(
            "a.local",
            &health(H, "watchdog", json!([])),
            Some(&t.memory),
            false,
            NOW + 24 * H,
            &rules(),
        );
        assert_eq!(rule_ids(&t), ["reset.fault"]);
        let t = triage(
            "a.local",
            &health(H, "crash", json!([])),
            Some(&t.memory),
            false,
            NOW + 48 * H,
            &rules(),
        );
        assert_eq!(rule_ids(&t), ["reset.fault.repeat"]);
        assert_eq!(t.findings[0].action, Action::Issue);
        assert!(t.findings[0].text.contains("2 faults in 7 days"));
        // A week later the first fault has aged out.
        let t = triage(
            "a.local",
            &health(H, "crash", json!([])),
            Some(&t.memory),
            false,
            NOW + 8 * 24 * H,
            &rules(),
        );
        assert_eq!(
            rule_ids(&t),
            ["reset.fault.repeat"],
            "the crash 6 days before still counts"
        );
        let t = triage(
            "a.local",
            &health(H, "crash", json!([])),
            Some(&t.memory),
            false,
            NOW + 30 * 24 * H,
            &rules(),
        );
        assert_eq!(rule_ids(&t), ["reset.fault"]);
    }

    #[test]
    fn rst_after_being_down_is_a_hang() {
        let t = triage(
            "a.local",
            &health(H, "reset pin", json!([])),
            None,
            true,
            NOW,
            &rules(),
        );
        assert_eq!(rule_ids(&t), ["reset.hung"]);
        assert_eq!(t.findings[0].action, Action::Notify);
        let t = triage(
            "a.local",
            &health(H, "reset pin", json!([])),
            Some(&t.memory),
            false,
            NOW + 24 * H,
            &rules(),
        );
        assert_eq!(rule_ids(&t), ["reset.hung.repeat"]);
    }

    #[test]
    fn low_heap_is_judged_against_the_boards_usual() {
        let mut mem: Option<BoardMem> = None;
        // Always 12 KiB and the board always warns: normal for it.
        for day in 0..4 {
            let t = triage(
                "m.local",
                &health(
                    240 * H + day * 24 * H,
                    "power on",
                    json!([heap(12, "warn")]),
                ),
                mem.as_ref(),
                false,
                NOW + day * 24 * H,
                &rules(),
            );
            let ids = rule_ids(&t);
            if day < 3 {
                assert_eq!(ids, ["heap.learning"], "day {day}");
                assert_eq!(t.findings[0].action, Action::Ignore);
            } else {
                assert!(ids.is_empty(), "day {day}: {ids:?}");
            }
            mem = Some(t.memory);
        }
        let t = triage(
            "m.local",
            &health(360 * H, "power on", json!([heap(6, "fail")])),
            mem.as_ref(),
            false,
            NOW + 5 * 24 * H,
            &rules(),
        );
        assert_eq!(rule_ids(&t), ["heap.low"]);
        assert!(t.findings[0].text.contains("6 KiB free, usually 12 KiB"));
    }

    #[test]
    fn new_allocation_failures_are_an_issue_once() {
        let alloc = |n: u32| json!([{"id": "heap.allocations", "status": if n > 0 {"warn"} else {"ok"}, "summary": "", "value": n}]);
        let t = triage(
            "a.local",
            &health(240 * H, "power on", alloc(3)),
            None,
            false,
            NOW,
            &rules(),
        );
        assert_eq!(rule_ids(&t), ["heap.allocations"]);
        let t = triage(
            "a.local",
            &health(264 * H, "power on", alloc(3)),
            Some(&t.memory),
            false,
            NOW + 24 * H,
            &rules(),
        );
        assert!(t.findings.is_empty(), "no new failures");
    }

    #[test]
    fn app_checks_pass_through_and_overrides_apply() {
        let checks = json!([{"id": "ledger.storage", "status": "fail", "summary": "A journal write failed", "runbook": "storage"}]);
        let t = triage(
            "n.local",
            &health(240 * H, "power on", checks.clone()),
            None,
            false,
            NOW,
            &rules(),
        );
        assert_eq!(t.findings[0].line(), "Needs you: ledger.storage: A journal write failed (runbook storage). The board reports a failure; follow its runbook.");
        let quiet =
            Rules::parse("check.fail = digest # too noisy\nnonsense = issue\nwifi.weak = shout");
        let t = triage(
            "n.local",
            &health(240 * H, "power on", checks),
            None,
            false,
            NOW,
            &quiet,
        );
        assert_eq!(t.findings[0].action, Action::Digest);
        assert_eq!(quiet.action("wifi.weak"), Action::Digest);
    }

    #[test]
    fn conditions_age_and_clear() {
        let f = |rule| Finding {
            rule,
            action: Action::Notify,
            text: String::new(),
            event: false,
            age: String::new(),
        };
        let mut open = Vec::new();
        let mut found = vec![("m.local".to_string(), vec![f("heap.low"), f("down")])];
        assert!(track(&mut open, &mut found, NOW).is_empty());
        assert_eq!(found[0].1[0].age, "(new)");
        let mut found = vec![("m.local".to_string(), vec![f("heap.low")])];
        let cleared = track(&mut open, &mut found, NOW + 50 * H);
        assert_eq!(found[0].1[0].age, "(day 3)");
        assert_eq!(cleared, ["m down"]);
        assert_eq!(open.len(), 1);
    }

    #[test]
    fn evidence_names_what_happened_before_the_reset() {
        let incidents = json!({"previous": {"boot_id": 1, "events": [
            {"kind": "boot", "count": 1},
            {"kind": "wifi_down", "count": 2, "code": 8},
            {"kind": "allocation_failed", "count": 3, "bytes": 4096},
            {"kind": "worker_stalled", "count": 1}
        ], "counters": []}});
        let log = json!({"previous": "I (12) boot\nE (9000) tls: handshake failed\u{1b}[0m\n\n"});
        assert_eq!(
            evidence(Some(&incidents), Some(&log)),
            "Before the reset: wifi_down x2, allocation_failed x3 (4096 B), worker_stalled; last log line \"E (9000) tls: handshake failed[0m\""
        );
        assert_eq!(
            evidence(Some(&json!({"previous": null})), None),
            "Its error log kept nothing from before the reset"
        );
        assert_eq!(evidence(None, None), "Its error log could not be read");
    }

    #[test]
    fn memory_stays_inside_its_budget() {
        let mut m = Memory::default();
        for i in 0..12 {
            m.b.push(BoardMem {
                h: format!("board-number-{i}.local"),
                t: 29_800_000,
                r: vec![(29_800_000, 'c'); RESETS],
                a: 100,
                f: vec![120; SAMPLES],
                l: vec![60; SAMPLES],
                w: vec![-70; SAMPLES],
            });
            m.o.push(Open {
                h: format!("board-number-{i}.local"),
                r: "heap.fragmented".into(),
                s: 29_800_000,
            });
        }
        let mut state = BTreeMap::new();
        m.write(&mut state);
        assert!(state[KEY].len() <= MAX_BYTES);
        let back = Memory::read(&state);
        assert_eq!(back.b.len(), 12, "boards kept; old readings dropped first");
        assert!(back.b.iter().all(|b| b.f.len() >= LEARNING));
    }
}
