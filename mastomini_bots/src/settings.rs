//! Bot settings: the one way a bot plugs its configuration into the admin
//! site. A bot lists [`Setting`]s; the service validates and stores the
//! values; the admin app draws the form from the list. No bot writes UI code.
//!
//! Values are flat strings keyed by the setting's key: the lightest shape
//! there is, stored as-is and shown as-is. Typed getters on [`Settings`]
//! parse them. [`schedule_settings`] and [`schedule_from`] give every bot the
//! same way to say when it runs.

use crate::schedule::{Schedule, MAX_TIMES};
use crate::tz::{Tz, ZONES};
use serde::Serialize;
use std::collections::BTreeMap;

pub const MAX_TEXT: usize = 200;
pub const MAX_LONG_TEXT: usize = 4000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Kind {
    /// One line.
    Text,
    /// Several lines: instructions, templates. `placeholders` are the
    /// `{{names}}` a template may use; anything else is refused.
    LongText {
        placeholders: Vec<&'static str>,
    },
    Number {
        min: i64,
        max: i64,
    },
    /// One of `options` (value, label).
    Choice {
        options: Vec<(&'static str, &'static str)>,
    },
    /// `HH:MM`, 24-hour.
    Time,
    /// One to [`MAX_TIMES`] `HH:MM` times, stored as `08:00, 20:00`.
    Times,
    /// One of the time zones in [`ZONES`], by id.
    Zone,
    /// `yes` or `no`.
    Toggle,
    /// Stored, never shown again (an API key). Empty removes it.
    Secret,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Setting {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    #[serde(flatten)]
    pub kind: Kind,
    pub default: &'static str,
    /// May be left empty.
    pub optional: bool,
    /// A heading the admin app draws once above consecutive settings that
    /// share it (a column of checkboxes). Display only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<&'static str>,
    /// Shown only while another setting has one of these values (a
    /// strategy's own parameters). Display only: values are still checked
    /// and kept either way.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shown_when: Option<ShownWhen>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShownWhen {
    pub key: &'static str,
    pub values: Vec<&'static str>,
}

impl Setting {
    pub fn new(
        key: &'static str,
        label: &'static str,
        kind: Kind,
        default: &'static str,
    ) -> Setting {
        Setting {
            key,
            label,
            help: "",
            kind,
            default,
            optional: false,
            group: None,
            shown_when: None,
        }
    }

    pub fn help(mut self, help: &'static str) -> Setting {
        self.help = help;
        self
    }

    pub fn optional(mut self) -> Setting {
        self.optional = true;
        self
    }

    pub fn group(mut self, heading: &'static str) -> Setting {
        self.group = Some(heading);
        self
    }

    pub fn shown_when(mut self, key: &'static str, values: &[&'static str]) -> Setting {
        self.shown_when = Some(ShownWhen {
            key,
            values: values.to_vec(),
        });
        self
    }

    /// The value in its stored form, or why it can't be.
    pub fn check(&self, value: &str) -> Result<String, String> {
        let value = match self.kind {
            Kind::LongText { .. } => value.replace("\r\n", "\n").trim().to_string(),
            _ => value.trim().to_string(),
        };
        let bad = |why: &str| Err(format!("{}: {why}", self.label));
        if value.is_empty() {
            return if self.optional || matches!(self.kind, Kind::Secret) {
                Ok(value)
            } else {
                bad("can't be empty")
            };
        }
        match &self.kind {
            Kind::Text | Kind::Secret if value.chars().count() > MAX_TEXT => bad("too long"),
            Kind::Text | Kind::Secret if value.chars().any(char::is_control) => {
                bad("one line only")
            }
            Kind::LongText { .. } if value.chars().count() > MAX_LONG_TEXT => bad("too long"),
            Kind::LongText { placeholders } => {
                let unknown: Vec<String> = crate::template::names(&value)
                    .into_iter()
                    .filter(|n| !placeholders.contains(&n.as_str()))
                    .collect();
                if unknown.is_empty() {
                    Ok(value)
                } else {
                    bad(&format!(
                        "unknown {} (available: {})",
                        unknown
                            .iter()
                            .map(|n| format!("{{{{{n}}}}}"))
                            .collect::<Vec<_>>()
                            .join(", "),
                        placeholders
                            .iter()
                            .map(|n| format!("{{{{{n}}}}}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                }
            }
            Kind::Number { min, max } => match value.parse::<i64>() {
                Ok(n) if (*min..=*max).contains(&n) => Ok(n.to_string()),
                _ => bad(&format!("a whole number from {min} to {max}")),
            },
            Kind::Choice { options } if options.iter().any(|(v, _)| *v == value) => Ok(value),
            Kind::Choice { .. } => bad("not one of the choices"),
            Kind::Time => parse_time(&value)
                .map(|(h, m)| format!("{h:02}:{m:02}"))
                .ok_or_else(|| format!("{}: a time like 07:30", self.label)),
            Kind::Times => parse_times(&value)
                .map(|times| format_times(&times))
                .ok_or_else(|| format!("{}: 1 to {MAX_TIMES} times like 08:00, 20:00", self.label)),
            Kind::Zone if ZONES.iter().any(|z| z.id == value) => Ok(value),
            Kind::Zone => bad("not a known time zone"),
            Kind::Toggle if matches!(value.as_str(), "yes" | "no") => Ok(value),
            Kind::Toggle => bad("yes or no"),
            _ => Ok(value),
        }
    }
}

fn parse_time(text: &str) -> Option<(u32, u32)> {
    let (h, m) = text.split_once(':')?;
    let (h, m): (u32, u32) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    (h < 24 && m < 60).then_some((h, m))
}

/// `08:00, 20:00` (commas or spaces): sorted, without repeats.
fn parse_times(text: &str) -> Option<Vec<(u32, u32)>> {
    let mut times = text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .map(parse_time)
        .collect::<Option<Vec<_>>>()?;
    times.sort_unstable();
    times.dedup();
    (1..=MAX_TIMES).contains(&times.len()).then_some(times)
}

fn format_times(times: &[(u32, u32)]) -> String {
    times
        .iter()
        .map(|(h, m)| format!("{h:02}:{m:02}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A bot's settings with their current values.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    specs: Vec<Setting>,
    values: BTreeMap<String, String>,
}

impl Settings {
    pub fn new(specs: Vec<Setting>, values: BTreeMap<String, String>) -> Settings {
        Settings { specs, values }
    }

    pub fn specs(&self) -> &[Setting] {
        &self.specs
    }

    /// The stored value, else the default ("" for an unknown key).
    pub fn get(&self, key: &str) -> &str {
        match self.values.get(key) {
            Some(v) => v,
            None => self
                .specs
                .iter()
                .find(|s| s.key == key)
                .map_or("", |s| s.default),
        }
    }

    pub fn is_set(&self, key: &str) -> bool {
        !self.get(key).is_empty()
    }

    pub fn number(&self, key: &str) -> i64 {
        self.get(key).parse().unwrap_or(0)
    }

    pub fn yes(&self, key: &str) -> bool {
        self.get(key) == "yes"
    }

    /// `(hour, minute)` of a [`Kind::Time`] setting (the first of a
    /// [`Kind::Times`] one).
    pub fn time(&self, key: &str) -> (u32, u32) {
        self.times(key).first().copied().unwrap_or((0, 0))
    }

    /// Every `(hour, minute)` of a [`Kind::Times`] (or `Time`) setting.
    pub fn times(&self, key: &str) -> Vec<(u32, u32)> {
        parse_times(self.get(key)).unwrap_or_else(|| vec![(0, 0)])
    }

    /// The POSIX rule and label of a [`Kind::Zone`] setting.
    pub fn zone(&self, key: &str) -> (&'static str, &'static str) {
        let id = self.get(key);
        let zone = ZONES.iter().find(|z| z.id == id).unwrap_or(&ZONES[0]);
        (zone.posix, zone.label)
    }

    pub fn tz(&self, key: &str) -> Tz {
        Tz::parse(self.zone(key).0).expect("ZONES are valid")
    }

    /// Validate `changes` against the specs and merge them in. `None` for
    /// a key leaves it; a secret given as "" is removed.
    pub fn merged(
        &self,
        changes: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, String>, String> {
        let mut values = self.values.clone();
        for (key, value) in changes {
            let spec = self
                .specs
                .iter()
                .find(|s| s.key == key)
                .ok_or_else(|| format!("No setting called {key}"))?;
            let value = spec.check(value)?;
            if value.is_empty() && spec.kind == Kind::Secret {
                values.remove(key);
            } else {
                values.insert(key.clone(), value);
            }
        }
        Ok(values)
    }

    /// For the admin site: every spec with its value (secrets only say
    /// whether they are set).
    pub fn public(&self) -> Vec<serde_json::Value> {
        self.specs
            .iter()
            .map(|s| {
                let mut v = serde_json::to_value(s).unwrap_or_default();
                if s.kind == Kind::Zone {
                    v["options"] = ZONES
                        .iter()
                        .map(|z| serde_json::json!([z.id, z.label]))
                        .collect();
                }
                if s.kind == Kind::Secret {
                    v["set"] = self.values.contains_key(s.key).into();
                } else {
                    v["value"] = self.get(s.key).into();
                }
                v
            })
            .collect()
    }
}

/// The settings every scheduled bot shares: when it runs.
pub fn schedule_settings(
    default_kind: &'static str,
    time: &'static str,
    zone: &'static str,
    every: &'static str,
) -> Vec<Setting> {
    vec![
        Setting::new(
            "run",
            "Runs",
            Kind::Choice {
                options: vec![
                    ("daily", "Every day at a time"),
                    ("every", "Every few minutes"),
                    ("manual", "Only by hand"),
                ],
            },
            default_kind,
        ),
        Setting::new("time", "Times of day", Kind::Times, time).help(
            "For “every day”: 24-hour clock, in the time zone below. Up to four, e.g. 08:00, 20:00.",
        ),
        Setting::new("zone", "Time zone", Kind::Zone, zone)
            .help("Daylight saving is followed automatically."),
        Setting::new(
            "every",
            "Every (minutes)",
            Kind::Number { min: 1, max: 1440 },
            every,
        )
        .help("For “every few minutes”."),
    ]
}

/// The [`Schedule`] those settings describe.
pub fn schedule_from(s: &Settings) -> Schedule {
    match s.get("run") {
        "daily" => {
            let (tz, label) = s.zone("zone");
            Schedule::Daily {
                times: s.times("time"),
                tz: tz.to_string(),
                label: label.to_string(),
            }
        }
        "every" => Schedule::Every {
            minutes: s.number("every").clamp(1, 1440) as u32,
        },
        _ => Schedule::Manual,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specs() -> Vec<Setting> {
        let mut s = schedule_settings("daily", "07:30", "us_eastern", "60");
        s.push(Setting::new(
            "message",
            "Message",
            Kind::LongText {
                placeholders: vec!["date"],
            },
            "Hi {{date}}",
        ));
        s.push(Setting::new("key", "API key", Kind::Secret, ""));
        s.push(Setting::new("loud", "Loud", Kind::Toggle, "no"));
        s
    }

    fn change(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn defaults_validation_and_secrets() {
        let s = Settings::new(specs(), BTreeMap::new());
        assert_eq!(s.get("time"), "07:30");
        assert_eq!(s.time("time"), (7, 30));
        assert_eq!(
            schedule_from(&s).describe(),
            "Every day at 07:30 US Eastern"
        );
        let merged = s
            .merged(&change(&[
                ("time", " 6:05 "),
                ("key", "sk-1"),
                ("run", "every"),
                ("every", "15"),
            ]))
            .unwrap();
        let s = Settings::new(specs(), merged);
        assert_eq!(s.get("time"), "06:05");
        assert_eq!(schedule_from(&s), Schedule::Every { minutes: 15 });
        let public = s.public();
        let key = public.iter().find(|v| v["key"] == "key").unwrap();
        assert_eq!(key["set"], true);
        assert!(key.get("value").is_none(), "secrets never go out");
        let cleared = s.merged(&change(&[("key", "")])).unwrap();
        assert!(!cleared.contains_key("key"));
        for (k, v) in [
            ("time", "25:00"),
            ("every", "0"),
            ("run", "hourly"),
            ("loud", "maybe"),
            ("zone", "mars"),
            ("nope", "x"),
            ("message", ""),
        ] {
            assert!(s.merged(&change(&[(k, v)])).is_err(), "{k}={v}");
        }
    }

    #[test]
    fn several_times_of_day() {
        let s = Settings::new(specs(), BTreeMap::new());
        let merged = s.merged(&change(&[("time", "20:00 8:00,20:00")])).unwrap();
        assert_eq!(merged["time"], "08:00, 20:00");
        let s = Settings::new(specs(), merged);
        assert_eq!(s.times("time"), vec![(8, 0), (20, 0)]);
        assert_eq!(
            schedule_from(&s).describe(),
            "Every day at 08:00 and 20:00 US Eastern"
        );
        for bad in ["", "8", "1:00 2:00 3:00 4:00 5:00", "08:00, 25:00"] {
            assert!(s.merged(&change(&[("time", bad)])).is_err(), "{bad}");
        }
    }

    #[test]
    fn groups_and_conditions_reach_the_admin_app() {
        let specs = vec![
            Setting::new("strategy", "Strategy", Kind::Toggle, "no"),
            Setting::new("a", "A", Kind::Toggle, "yes").group("Post about"),
            Setting::new("b", "B", Kind::Number { min: 0, max: 9 }, "1")
                .shown_when("strategy", &["yes"]),
        ];
        let public = Settings::new(specs, BTreeMap::new()).public();
        assert!(public[0].get("group").is_none());
        assert_eq!(public[1]["group"], "Post about");
        assert_eq!(
            public[2]["shown_when"],
            serde_json::json!({"key": "strategy", "values": ["yes"]})
        );
    }

    #[test]
    fn templates_may_use_only_their_placeholders() {
        let s = Settings::new(specs(), BTreeMap::new());
        assert!(s
            .merged(&change(&[("message", "Morning, {{date}}!")]))
            .is_ok());
        let e = s
            .merged(&change(&[("message", "{{weather}}")]))
            .unwrap_err();
        assert!(e.contains("{{weather}}") && e.contains("{{date}}"), "{e}");
    }
}
