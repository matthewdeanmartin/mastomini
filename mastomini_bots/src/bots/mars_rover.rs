//! Mars Expedition: all authoritative state lives in bot-authored MR2
//! checkpoints in the mission thread. Board state is never needed.
use crate::bot::{Bot, BotInfo, Run, RunError};
use crate::bots::{fishtank::epoch, good_morning::visibility_setting};
use crate::mastodon::{Account, NewStatus, Visibility};
use crate::schedule::Schedule;
use crate::settings::{schedule_from, schedule_settings, Kind, Setting, Settings};
use crate::tz::days_from_civil;

pub struct MarsRover;
const PAGE: usize = 40;
const HISTORY_PAGES: usize = 10;
const MAX_REPLAY: u64 = 100_000;

impl Bot for MarsRover {
    fn info(&self) -> BotInfo {
        BotInfo {
            id: "mars_rover", name: "Mars Expedition",
            description: "Explore Mars together: steer one shared rover, reveal terrain and collect samples by replying to its posts.",
            schedule: Schedule::Manual, grace_minutes: 30,
            default_instance: "https://mastomini.local", uses_llm: false, needs_mastodon: true,
        }
    }
    fn settings(&self) -> Vec<Setting> {
        let mut s = schedule_settings("every", "12:00", "us_eastern", "5");
        s.extend([
            Setting::new("seed", "Mars terrain seed", Kind::Number { min: 0, max: u32::MAX as i64 }, "42"),
            Setting::new("epoch", "Mission start (YYYY-MM-DD)", Kind::Text, "2026-10-08")
                .help("Keep seed, start date and zone fixed. Changing seed or date selects a different mission thread."),
            Setting::new("daily_time", "Daily mission log time", Kind::Time, "12:00")
                .help("First poll at or after this local time advances the rover and posts a daily checkpoint."),
            Setting::new("commands_per_run", "Commands per poll", Kind::Number { min: 1, max: 10 }, "3"),
            visibility_setting(),
        ]);
        s
    }
    fn ready(&self, settings: &Settings) -> Result<(), String> {
        epoch(settings.get("epoch")).map(|_| ())
    }
    fn schedule(&self, settings: &Settings) -> Schedule {
        schedule_from(settings)
    }
    fn run(&self, run: &mut Run<'_>) -> Result<String, RunError> {
        let start = epoch(run.settings.get("epoch")).map_err(RunError::permanent)? as u64;
        let local = run.settings.tz("zone").local(run.slot_ms);
        let today = days_from_civil(local.year, local.month, local.day);
        if today < start as i64 {
            return Ok("Mars mission has not launched yet".into());
        }
        let today = today as u64;
        if today - start > MAX_REPLAY {
            return Err(RunError::permanent("Mars replay exceeds 100,000 days"));
        }
        let seed = run.settings.number("seed") as u64;
        let me = run.mastodon().me()?;
        if me.id.is_empty() || me.acct.is_empty() {
            return Err(RunError::permanent(
                "Mars account response was incomplete; refusing to create a mission",
            ));
        }
        let limit = run.mastodon().max_characters()?;
        let recovered = recover(run, &me, seed, start)?;
        let Some((mut rover, mut head)) = recovered else {
            // Ignore mentions from before landing, without retaining a local cursor.
            let mentions = run.mastodon().mentions(None, 1)?;
            let cursor = mentions.last().map(|m| id(&m.id)).transpose()?.unwrap_or(0);
            let mut rover = Rover::new(seed, start, cursor);
            rover.advance(today);
            let posted = checkpoint(
                run,
                &rover,
                None,
                &format!("Mars landing! We share one rover. Explore Mars and collect samples together.\n{}\nReply scan to look around, or help to learn how to play.", rover.status()),
                &format!("land:{seed}:{start}"),
                limit,
            )?;
            id(&posted.id)?;
            return Ok(format!("Mars mission landed: {}", posted.url));
        };
        if rover.day > today {
            return Err(RunError::permanent(
                "Mars checkpoint is ahead of the slot date; check the clock",
            ));
        }
        let (h, m) = run.settings.time("daily_time");
        let mut daily = false;
        if rover.day < today && (local.hour, local.minute) >= (h, m) {
            let elapsed = today - rover.day;
            let movement = rover.advance(today);
            let message = format!(
                "Mars • Sol {} (mission day)\n{}\nAuto-exploration: {elapsed} daily recharge/move cycle(s). Latest: {movement}\n{}\nReply scan, sample, map or help.",
                rover.day - rover.epoch + 1,
                weather(seed, today),
                rover.status()
            );
            head = checkpoint(
                run,
                &rover,
                Some(&head.id),
                &message,
                &format!("sol:{}:{today}", base36(rover.root)),
                limit,
            )?;
            daily = true;
        }
        let mentions = run
            .mastodon()
            .mentions(Some(&rover.cursor.to_string()), PAGE)?;
        let mut handled = 0;
        let mut cursor_dirty = false;
        for mention in mentions {
            if handled >= run.settings.number("commands_per_run") as usize {
                break;
            }
            let notification = id(&mention.id)?;
            let status = &mention.status;
            if status.account.id == me.id || status.account.bot || status.visibility == "direct" {
                rover.cursor = notification;
                cursor_dirty = true;
                continue;
            }
            // A command must actually be in this mission. Trust only the
            // account's own checkpoints, never an MR2 line supplied by a user.
            let ancestors = run.mastodon().ancestors(&status.id)?;
            let in_mission = ancestors.iter().any(|p| {
                if p.account.id != me.id {
                    return false;
                }
                id(&p.id).ok() == Some(rover.root)
                    || Rover::parse(&p.text).ok().is_some_and(|r| {
                        r.seed == seed
                            && r.epoch == start
                            && (r.root == rover.root
                                || r.root == 0 && id(&p.id).ok() == Some(rover.root))
                    })
            });
            if !in_mission || status.visibility != head_visibility(run) {
                rover.cursor = notification;
                cursor_dirty = true;
                continue;
            }
            let mut next = rover.clone();
            next.cursor = notification;
            let command = strip_address(&status.text, &me.acct);
            let result = next.command(command);
            let message = format!("@{} {result}", status.account.acct);
            // The final save commits the result and cursor after all narrative
            // parts. Every part has a stable key for partial-write retries.
            head = checkpoint(
                run,
                &next,
                Some(&head.id),
                &message,
                &format!("cmd:{}:{}", base36(rover.root), base36(id(&status.id)?)),
                limit,
            )?;
            rover = next;
            cursor_dirty = false;
            handled += 1;
        }
        if cursor_dirty {
            checkpoint(
                run,
                &rover,
                Some(&head.id),
                "",
                &format!("cursor:{}:{}", base36(rover.root), base36(rover.cursor)),
                limit,
            )?;
        }
        Ok(format!(
            "Mars: {handled} commands{}",
            if daily { "; daily log posted" } else { "" }
        ))
    }
}

fn head_visibility<'a>(run: &'a Run<'_>) -> &'a str {
    run.settings.get("visibility")
}
fn id(text: &str) -> Result<u64, RunError> {
    text.parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| RunError::permanent("Mars requires nonzero numeric Mastodon IDs"))
}

fn recover(
    run: &mut Run<'_>,
    me: &Account,
    seed: u64,
    start: u64,
) -> Result<Option<(Rover, crate::mastodon::Posted)>, RunError> {
    let mut before: Option<String> = None;
    for _ in 0..HISTORY_PAGES {
        let posts = run
            .mastodon()
            .account_posts(&me.id, before.as_deref(), PAGE)?;
        for post in &posts {
            if post.account.id != me.id || !post.text.lines().any(|l| l.starts_with("MR2:")) {
                continue;
            }
            let mut rover = Rover::parse(&post.text).map_err(RunError::permanent)?;
            if rover.seed != seed || rover.epoch != start {
                continue;
            }
            if rover.root == 0 {
                rover.root = id(&post.id)?;
            }
            if post.visibility != head_visibility(run) {
                return Err(RunError::permanent(
                    "Keep the Mars mission visibility fixed; restore its previous setting",
                ));
            }
            return Ok(Some((
                rover,
                crate::mastodon::Posted {
                    id: post.id.clone(),
                    url: String::new(),
                },
            )));
        }
        if posts.len() < PAGE {
            return Ok(None);
        }
        let oldest = posts.last().unwrap().id.clone();
        id(&oldest)?;
        if before
            .as_ref()
            .is_some_and(|previous| crate::mastodon::id_order(&oldest, previous).is_ge())
        {
            return Err(RunError::permanent(
                "Mars history pagination did not advance",
            ));
        }
        before = Some(oldest);
    }
    Err(RunError::permanent(
        "No Mars checkpoint in 400 account posts; refusing to reset the mission",
    ))
}

fn checkpoint(
    run: &mut Run<'_>,
    rover: &Rover,
    parent: Option<&str>,
    message: &str,
    key: &str,
    limit: usize,
) -> Result<crate::mastodon::Posted, RunError> {
    // Narrative posts stay readable; machine state is a separate collapsed post.
    // The first narrative post anchors the mission, including landing replies.
    const SAVE_LABEL: &str = "Game save (ignore)";
    let mut saved = rover.clone();
    // Reserve the longest possible root ID before writing any narrative.
    saved.root = u64::MAX;
    if saved.encode().len() + SAVE_LABEL.len() > limit {
        return Err(RunError::permanent(
            "Server character limit is too small for a Mars checkpoint",
        ));
    }
    saved.root = rover.root;
    let mut parent = parent.map(str::to_string);
    for (part, text) in narrative_parts(message, limit)?.into_iter().enumerate() {
        let posted = run.post_keyed(
            &NewStatus {
                text,
                visibility: Visibility::parse(head_visibility(run)),
                spoiler_text: None,
                in_reply_to_id: parent.clone(),
            },
            &format!("m2:{key}:t:{part}"),
        )?;
        let post_id = id(&posted.id)?;
        if saved.root == 0 {
            saved.root = post_id;
        }
        parent = Some(posted.id);
    }
    let posted = run.post_keyed(
        &NewStatus {
            text: saved.encode(),
            visibility: Visibility::parse(head_visibility(run)),
            spoiler_text: Some(SAVE_LABEL.into()),
            in_reply_to_id: parent,
        },
        &format!("m2:{key}"),
    )?;
    id(&posted.id)?;
    Ok(posted)
}

/// Preserve lines when possible, wrap at words, and number continuations.
fn narrative_parts(message: &str, limit: usize) -> Result<Vec<String>, RunError> {
    if message.is_empty() {
        return Ok(Vec::new());
    }
    if message.chars().count() <= limit {
        return Ok(vec![message.into()]);
    }
    // Repeat the player mention before the part number on every continuation.
    let (prefix, content) = match message.split_once(' ') {
        Some((mention, body)) if mention.starts_with('@') => (format!("{mention} "), body),
        _ => (String::new(), message),
    };
    let budget = limit
        .checked_sub(12 + prefix.chars().count())
        .filter(|n| *n > 0)
        .ok_or_else(|| RunError::permanent("Mars post limit is too small"))?;
    let mut parts = Vec::new();
    let mut current = String::new();
    for line in content.lines() {
        if !current.is_empty() && current.chars().count() + 1 + line.chars().count() <= budget {
            current.push('\n');
            current.push_str(line);
            continue;
        }
        if !current.is_empty() {
            parts.push(std::mem::take(&mut current));
        }
        for word in line.split_whitespace() {
            if word.chars().count() > budget {
                return Err(RunError::permanent(
                    "A Mars post word exceeds the character limit",
                ));
            }
            if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > budget {
                parts.push(std::mem::take(&mut current));
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    let count = parts.len();
    if count > 9999 {
        return Err(RunError::permanent("Too many Mars post continuations"));
    }
    Ok(parts
        .into_iter()
        .enumerate()
        .map(|(i, text)| format!("{prefix}[{}/{}] {text}", i + 1, count))
        .collect())
}

fn strip_address<'a>(text: &'a str, acct: &str) -> &'a str {
    let mut body = text.trim();
    while let Some(first) = body.split_whitespace().next() {
        if !first
            .trim_end_matches([',', ':'])
            .eq_ignore_ascii_case(&format!("@{acct}"))
        {
            break;
        }
        body = body[first.len()..].trim_start();
    }
    body
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Rover {
    seed: u64,
    epoch: u64,
    day: u64,
    pos: u64,
    battery: u64,
    scanned: u64,
    samples: u64,
    cursor: u64,
    root: u64,
    charged: u64,
    named: u64,
    name: String,
}
impl Rover {
    fn new(seed: u64, epoch: u64, cursor: u64) -> Self {
        Self {
            seed,
            epoch,
            day: epoch,
            pos: 27,
            battery: 100,
            scanned: 1 << 27,
            samples: 0,
            cursor,
            root: 0,
            charged: 0,
            named: 64,
            name: "-".into(),
        }
    }
    fn encode(&self) -> String {
        let fields = [
            self.seed,
            self.epoch,
            self.day,
            self.pos,
            self.battery,
            self.scanned,
            self.samples,
            self.cursor,
            self.root,
            self.charged,
            self.named,
        ];
        format!("MR2:{},{}", fields.map(base36).join(","), self.name)
    }
    fn parse(text: &str) -> Result<Self, String> {
        let bad = || "Invalid Mars checkpoint; refusing to reset or roll back".to_string();
        let line = text
            .lines()
            .find_map(|l| l.strip_prefix("MR2:"))
            .ok_or_else(bad)?;
        let fields: Vec<_> = line.split(',').collect();
        if fields.len() != 12 {
            return Err(bad());
        }
        let nums: Vec<u64> = fields[..11]
            .iter()
            .map(|s| u64::from_str_radix(s, 36).map_err(|_| bad()))
            .collect::<Result<_, _>>()?;
        let r = Self {
            seed: nums[0],
            epoch: nums[1],
            day: nums[2],
            pos: nums[3],
            battery: nums[4],
            scanned: nums[5],
            samples: nums[6],
            cursor: nums[7],
            root: nums[8],
            charged: nums[9],
            named: nums[10],
            name: fields[11].into(),
        };
        if r.seed > u32::MAX as u64
            || r.epoch > 2_932_896
            || r.day < r.epoch
            || r.day - r.epoch > MAX_REPLAY
            || r.pos >= 64
            || r.battery > 100
            || r.named > 64
            || r.charged > r.day
            || r.samples & !r.scanned != 0
            || !valid_name(&r.name)
            || r.scanned & (1 << r.pos) == 0
            || r.named < 64 && r.samples & (1 << r.named) == 0
            || r.named == 64 && r.name != "-"
            || r.named < 64 && r.name == "-"
        {
            return Err(bad());
        }
        Ok(r)
    }
    fn status(&self) -> String {
        let mut status = format!(
            "Location {} • Battery {}/100 • Samples {}",
            coordinate(self.pos),
            self.battery,
            self.samples.count_ones()
        );
        if self.named < 64 {
            status.push_str(&format!(
                "
Latest named site: {} at {}",
                self.name,
                coordinate(self.named)
            ));
        }
        status
    }
    fn advance(&mut self, day: u64) -> String {
        let mut latest = String::new();
        while self.day < day {
            self.day += 1;
            let recharge = if weather(self.seed, self.day) == "dust storm" {
                5
            } else {
                20
            };
            self.battery = (self.battery + recharge).min(100);
            let direction =
                ["north", "east", "south", "west"][(mix(self.seed ^ self.day) % 4) as usize];
            latest = format!("{direction}: {}", self.movement(direction));
        }
        latest
    }
    fn movement(&mut self, direction: &str) -> String {
        let (x, y) = ((self.pos % 8) as i32, (self.pos / 8) as i32);
        let (dx, dy) = match direction {
            "north" | "n" => (0, -1),
            "south" | "s" => (0, 1),
            "east" | "e" => (1, 0),
            "west" | "w" => (-1, 0),
            _ => return "Use move north/east/south/west.".into(),
        };
        let (nx, ny) = (x + dx, y + dy);
        if !(0..8).contains(&nx) || !(0..8).contains(&ny) {
            return "Map edge. Choose another direction.".into();
        }
        let next = (ny * 8 + nx) as u64;
        if terrain(self.seed, next) == "cliff" {
            return "Cliff ahead. Route blocked.".into();
        }
        let cost = if terrain(self.seed, next) == "crater" {
            15
        } else {
            10
        };
        if self.battery < cost {
            return "Low battery. Try charge.".into();
        }
        self.pos = next;
        self.battery -= cost;
        self.scanned |= 1 << next;
        format!(
            "Moved to {}: {}.
Battery {}/100 (-{cost}). Reply scan or sample.",
            coordinate(next),
            terrain(self.seed, next),
            self.battery
        )
    }
    fn command(&mut self, input: &str) -> String {
        let words: Vec<_> = input.split_whitespace().collect();
        let lower: Vec<_> = words.iter().map(|s| s.to_ascii_lowercase()).collect();
        let tokens: Vec<_> = lower.iter().map(String::as_str).collect();
        match tokens.as_slice() {
            ["help"] => format!(
                "Mars Expedition: everyone here controls ONE shared rover. Explore Mars and collect samples from new sites. There is no finish line.
You are at {} on {}. Battery: {}/100. Samples: {}.
Reply to a mission post, mentioning this bot, with one command. Commands run at the next poll.
Try scan (8 battery), then move north (10) or sample (15). Reply map or status for a free report; charge to recharge.
A sol is a mission day. Each daily update recharges the rover and tries an automatic move, even without commands.
More: help moves, help energy, help samples.
Game save (ignore) posts hold the bot's memory. You never need to read or copy the code inside.",
                coordinate(self.pos), terrain(self.seed, self.pos), self.battery, self.samples.count_ones()
            ),
            ["help", "moves"] => "Exploring Mars
scan reveals the surrounding 3×3 area for 8 battery. map lists known terrain for free. Unknown means not yet scanned or visited.
Reply move north, move east, move south or move west (or n/e/s/w). Each move costs 10 battery; entering a crater costs 15.
Cliffs and map edges block travel. No diagonal moves.
The map is 8×8: A–H run west to east; 1–8 run north to south. D4 means column D, row 4.".into(),
            ["help", "energy"] => "Rover energy
Battery ranges from 0 to 100. Reply charge to add 25 battery (5 in dust storms), once per sol.
A sol is one local calendar day. Each daily update adds 20 battery (5 in storms), then tries one automatic move.
Missed daily updates catch up when the bot runs. Reply status to see current battery and weather.".into(),
            ["help", "samples"] => "Collecting Mars samples
Reply sample to collect material at your location for 15 battery, once per site. Visit new sites to grow our shared sample collection.
Find red dust, basalt (volcanic rock), hematite (an iron mineral), water ice and craters. These are fictional discoveries.
After sampling, reply name discovery Kevin to label the site. Use 1–8 letters, digits, _ or -. Only the latest name is remembered.".into(),
            ["status"] => format!(
                "Mars • Sol {} (mission day) • {}
{}
Here: {}.
Reply scan to explore, sample to collect, or help for instructions.",
                self.day - self.epoch + 1,
                weather(self.seed, self.day), self.status(), terrain(self.seed, self.pos)
            ),
            ["map"] => self.map(),
            ["move", direction] => self.movement(direction),
            [direction @ ("n" | "e" | "s" | "w")] => self.movement(direction),
            ["scan"] => {
                if self.battery < 8 {
                    return "Scan needs 8 battery. Try charge.".into();
                }
                self.battery -= 8;
                let (x, y) = ((self.pos % 8) as i32, (self.pos / 8) as i32);
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let (nx, ny) = (x + dx, y + dy);
                        if (0..8).contains(&nx) && (0..8).contains(&ny) {
                            self.scanned |= 1 << (ny * 8 + nx);
                        }
                    }
                }
                format!(
                    "Scan complete. Battery {}/100 (-8).
{}",
                    self.battery, self.map()
                )
            }
            ["sample"] | ["sample", "rock"] => {
                let bit = 1 << self.pos;
                if self.samples & bit != 0 {
                    return "Already sampled this site. Move to a new site to collect another sample.".into();
                }
                if self.battery < 15 {
                    return "Sample needs 15 battery. Try charge.".into();
                }
                self.battery -= 15;
                self.samples |= bit;
                format!(
                    "Collected {} at {}!
Samples: {} • Battery {}/100 (-15).
Reply name discovery NAME to label this site, or move north/east/south/west to explore.",
                    terrain(self.seed, self.pos),
                    coordinate(self.pos), self.samples.count_ones(), self.battery
                )
            }
            ["charge"] => {
                if self.charged == self.day {
                    return "Already charged this sol (mission day). Try again after the next daily update.".into();
                }
                if self.battery == 100 {
                    return "Battery already full.".into();
                }
                self.charged = self.day;
                self.battery = (self.battery
                    + if weather(self.seed, self.day) == "dust storm" {
                        5
                    } else {
                        25
                    })
                .min(100);
                format!("Solar charge complete: battery {}/100. Reply scan or sample.", self.battery)
            }
            ["name", "discovery", _] => {
                if self.samples & (1 << self.pos) == 0 {
                    return "Sample this site before naming it.".into();
                }
                if !valid_name(words[2]) || words[2] == "-" {
                    return "Name: 1-8 letters, digits, _ or -.".into();
                }
                self.named = self.pos;
                self.name = words[2].into();
                format!("{} is now {}.", coordinate(self.pos), self.name)
            }
            _ => "I did not recognize that command. You control a shared Mars rover. Reply scan to look around, move north to drive, or help for instructions.".into(),
        }
    }
    fn map(&self) -> String {
        let mut text = format!(
            "Around {} (you): {}
",
            coordinate(self.pos),
            terrain(self.seed, self.pos)
        );
        let (x, y) = ((self.pos % 8) as i32, (self.pos / 8) as i32);
        for (direction, dx, dy) in [
            ("North", 0, -1),
            ("East", 1, 0),
            ("South", 0, 1),
            ("West", -1, 0),
            ("Northwest", -1, -1),
            ("Northeast", 1, -1),
            ("Southwest", -1, 1),
            ("Southeast", 1, 1),
        ] {
            let (nx, ny) = (x + dx, y + dy);
            if !(0..8).contains(&nx) || !(0..8).contains(&ny) {
                text.push_str(&format!(
                    "{direction}: map edge
"
                ));
            } else {
                let p = (ny * 8 + nx) as u64;
                let ground = if self.scanned & (1 << p) == 0 {
                    "unknown"
                } else {
                    terrain(self.seed, p)
                };
                text.push_str(&format!(
                    "{direction}: {} — {ground}{}
",
                    coordinate(p),
                    if ground == "cliff" { " (blocked)" } else { "" }
                ));
            }
        }
        text.push_str("Reply scan to reveal unknown terrain (8 battery). Move north/east/south/west; no diagonal moves.");
        text
    }
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 8
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn coordinate(pos: u64) -> String {
    format!("{}{}", (b'A' + (pos % 8) as u8) as char, pos / 8 + 1)
}
fn base36(mut n: u64) -> String {
    let mut out = Vec::new();
    loop {
        out.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}
fn mix(mut n: u64) -> u64 {
    n = n.wrapping_add(0x9e3779b97f4a7c15);
    n = (n ^ (n >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    n = (n ^ (n >> 27)).wrapping_mul(0x94d049bb133111eb);
    n ^ (n >> 31)
}
fn terrain(seed: u64, pos: u64) -> &'static str {
    if pos == 27 {
        return "basalt";
    }
    match mix(seed ^ pos.wrapping_mul(7919)) % 8 {
        0 => "cliff",
        1 => "crater",
        2 => "water ice",
        3 => "hematite",
        4 => "basalt",
        _ => "red dust",
    }
}
fn weather(seed: u64, day: u64) -> &'static str {
    if mix(seed ^ day.wrapping_mul(104729)).is_multiple_of(5) {
        "dust storm"
    } else {
        "clear sky"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mastodon::testing::Scripted;
    use serde_json::{json, Value};

    fn start() -> u64 {
        epoch("2026-10-08").unwrap() as u64
    }
    fn rover() -> Rover {
        let mut r = Rover::new(42, start(), 50);
        r.root = 100;
        r
    }
    fn status(id: &str, author: &str, text: &str, parent: Option<&str>) -> Value {
        json!({"id":id,"account":{"id":author,"acct":if author=="9" {"mars"} else {"alice"},"bot":author=="9"},
            "content":format!("<p>{}</p>",text.replace('\n',"<br>")),"visibility":"public","in_reply_to_id":parent})
    }
    fn basics(http: &Scripted, posts: Value) {
        http.answer(200, json!({"id":"9","acct":"mars","bot":true}));
        http.answer(
            200,
            json!({"configuration":{"statuses":{"max_characters":140}}}),
        );
        http.answer(200, posts);
    }
    fn execute(http: &mut Scripted, day: u64) -> Result<String, RunError> {
        let settings = Settings::new(MarsRover.settings(), [("zone".into(), "utc".into())].into());
        let slot = day * 86_400_000 + 43_200_000;
        let mut run = Run::new(
            "mars_rover",
            slot,
            slot,
            &settings,
            http,
            "https://mastomini.local",
            "K",
            None,
        );
        run.state.insert("game".into(), "deliberately wrong".into());
        let result = MarsRover.run(&mut run);
        assert_eq!(run.state.len(), 1, "game state must live remotely");
        result
    }
    fn body(http: &Scripted, n: usize) -> Value {
        serde_json::from_slice(&http.sent.lock().unwrap()[n].body).unwrap()
    }

    fn answer_narrative(http: &Scripted, message: &str, first: u64) -> usize {
        let count = narrative_parts(message, 140).unwrap().len();
        for n in 0..count {
            http.answer(
                200,
                json!({"id": (first + n as u64).to_string(), "url":"u"}),
            );
        }
        count
    }
    fn posted_bodies(http: &Scripted) -> Vec<Value> {
        http.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == "POST")
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect()
    }
    fn last_post(http: &Scripted) -> Value {
        posted_bodies(http).pop().unwrap()
    }

    #[test]
    fn checkpoint_round_trips_and_bounds_every_field() {
        let mut r = rover();
        r.scanned = u64::MAX;
        r.samples = u64::MAX;
        r.cursor = u64::MAX;
        r.root = u64::MAX;
        r.named = 63;
        r.name = "ABCDEFGH".into();
        r.seed = u32::MAX as u64;
        r.day += MAX_REPLAY;
        r.charged = r.day;
        assert_eq!(Rover::parse(&r.encode()).unwrap(), r);
        assert!(r.encode().len() + 20 <= 140);
        assert!(Rover::parse("MR2:garbage").is_err());
        r.battery = 101;
        assert!(Rover::parse(&r.encode()).is_err());
        r.battery = 100;
        r.samples = 0;
        assert!(Rover::parse(&r.encode()).is_err());
    }
    #[test]
    fn commands_use_battery_preserve_discoveries_and_charge_once_per_sol() {
        let mut r = rover();
        assert!(r.command("name discovery Kevin").contains("Sample"));
        assert!(r.command("scan").contains("basalt"));
        assert_eq!(r.battery, 92);
        assert_eq!(r.scanned.count_ones(), 9);
        assert!(r.command("sample rock").contains("basalt"));
        assert_eq!(r.battery, 77);
        let before = r.clone();
        assert!(r.command("sample").contains("Already"));
        assert_eq!(r, before);
        assert!(r.command("name discovery Kevin").contains("Kevin"));
        assert_eq!(Rover::parse(&r.encode()).unwrap().name, "Kevin");
        assert!(r.command("charge").contains("Solar"));
        let before = r.clone();
        assert!(r.command("charge").contains("Already"));
        assert_eq!(r, before);
        assert!(r.command("teleport").contains("help"));
        assert!(r.command("map").contains('R'));
        assert_eq!(strip_address("@MARS: scan", "mars"), "scan");
    }
    #[test]
    fn missed_days_replay_identically_and_movement_respects_boundaries_and_cliffs() {
        let mut live = rover();
        let mut jump = live.clone();
        for day in start() + 1..=start() + 36 {
            live.advance(day);
        }
        jump.advance(start() + 36);
        assert_eq!(live, jump);
        assert_eq!(live.day - live.epoch + 1, 37);
        assert!(live.battery <= 100 && live.pos < 64);
        let mut edge = rover();
        edge.pos = 0;
        edge.scanned |= 1;
        assert!(edge.command("move west").contains("edge"));
        assert_eq!(edge.pos, 0);
        for p in 0..64 {
            if p % 8 < 7 && terrain(42, p + 1) == "cliff" {
                edge.pos = p;
                assert!(edge.command("move east").contains("Cliff"));
                assert_eq!(edge.pos, p);
                break;
            }
        }
    }
    #[test]
    fn landing_is_a_root_checkpoint_and_ignores_old_mentions() {
        let mut http = Scripted::default();
        basics(&http, json!([]));
        http.answer(
            200,
            json!([{"id":"50","type":"mention","status":status("90","1","old",None)}]),
        );
        let landing = format!("Mars landing! We share one rover. Explore Mars and collect samples together.{}{}{}Reply scan to look around, or help to learn how to play.", char::from(10), rover().status(), char::from(10));
        answer_narrative(&http, &landing, 100);
        http.answer(200, json!({"id":"199","url":"u"}));
        assert!(execute(&mut http, start()).unwrap().contains("landed"));
        let posted = body(&http, 4);
        assert!(posted.get("in_reply_to_id").is_none());
        let r = Rover::parse(last_post(&http)["status"].as_str().unwrap()).unwrap();
        assert_eq!(r.cursor, 50);
        assert_eq!(r.root, 100);
        assert_eq!(r.battery, 100);
        assert!(http
            .sent
            .lock()
            .unwrap()
            .iter()
            .all(|p| !p.url.contains("openrouter")));
    }
    #[test]
    fn command_commit_and_restart_recovery_need_only_the_thread() {
        let r = rover();
        let mut http = Scripted::default();
        basics(&http, json!([status("100", "9", &r.encode(), None)]));
        http.answer(200,json!([{"id":"51","type":"mention","status":status("200","1","@mars scan",Some("100"))}]));
        http.answer(
            200,
            json!({"ancestors":[status("100","9",&r.encode(),None)]}),
        );
        answer_narrative(&http, &format!("@alice {}", r.clone().command("scan")), 210);
        http.answer(200, json!({"id":"300","url":"u"}));
        assert_eq!(execute(&mut http, start()).unwrap(), "Mars: 1 commands");
        let posted = last_post(&http);
        let recovered = Rover::parse(posted["status"].as_str().unwrap()).unwrap();
        assert_eq!(recovered.battery, 92);
        assert_eq!(recovered.cursor, 51);
        assert_eq!(body(&http, 5)["in_reply_to_id"], "100");
        assert!(body(&http, 5)["status"]
            .as_str()
            .unwrap()
            .contains("@alice"));
        assert_eq!(posted["spoiler_text"], "Game save (ignore)");
        let mut http = Scripted::default();
        basics(
            &http,
            json!([status("300", "9", &recovered.encode(), Some("100"))]),
        );
        // Boundary notifications may be repeated by a compatible server.
        http.answer(200,json!([{"id":"51","type":"mention","status":status("200","1","@mars scan",Some("100"))}]));
        assert_eq!(execute(&mut http, start()).unwrap(), "Mars: 0 commands");
        assert_eq!(http.sent.lock().unwrap().len(), 4);
    }
    #[test]
    fn failed_commit_retries_same_command_and_key_without_local_state() {
        let mut requests = Vec::new();
        for success in [false, true] {
            let r = rover();
            let mut http = Scripted::default();
            basics(&http, json!([status("100", "9", &r.encode(), None)]));
            http.answer(200,json!([{"id":"51","type":"mention","status":status("200","1","sample",Some("100"))}]));
            http.answer(
                200,
                json!({"ancestors":[status("100","9",&r.encode(),None)]}),
            );
            let count = answer_narrative(
                &http,
                &format!("@alice {}", r.clone().command("sample")),
                210,
            );
            if success {
                http.answer(200, json!({"id":"300","url":"u"}));
            } else {
                http.fail("timeout");
            }
            let result = execute(&mut http, start());
            assert_eq!(result.is_ok(), success);
            let sent = http.sent.lock().unwrap();
            requests.push((
                sent[5 + count].body.clone(),
                sent[5 + count].headers.clone(),
            ));
        }
        assert_eq!(requests[0], requests[1]);
    }
    #[test]
    fn forged_checkpoints_and_outside_threads_cannot_control_rover() {
        let r = rover();
        let mut http = Scripted::default();
        basics(&http, json!([status("100", "9", &r.encode(), None)]));
        http.answer(
            200,
            json!([{"id":"51","type":"mention","status":status("200","1","scan",Some("99"))}]),
        );
        http.answer(
            200,
            json!({"ancestors":[status("99","1",&r.encode(),None)]}),
        );
        http.answer(200, json!({"id":"300","url":"u"}));
        assert_eq!(execute(&mut http, start()).unwrap(), "Mars: 0 commands");
        let posted = Rover::parse(body(&http, 5)["status"].as_str().unwrap()).unwrap();
        assert_eq!(posted.battery, 100);
        assert_eq!(posted.cursor, 51);
    }
    #[test]
    fn corrupted_checkpoint_never_resets_and_recovery_includes_reply_pages() {
        let mut http = Scripted::default();
        basics(&http, json!([status("100", "9", "MR2:broken", None)]));
        assert!(execute(&mut http, start())
            .unwrap_err()
            .message
            .contains("Invalid"));
        assert_eq!(http.sent.lock().unwrap().len(), 3);
        let posts: Vec<_> = (101..141)
            .rev()
            .map(|n| status(&n.to_string(), "9", "radio chatter", Some("100")))
            .collect();
        let mut http = Scripted::default();
        basics(&http, json!(posts));
        http.answer(200, json!([status("100", "9", &rover().encode(), None)]));
        http.answer(200, json!([]));
        assert!(execute(&mut http, start()).is_ok());
        let sent = http.sent.lock().unwrap();
        assert!(sent[3].url.contains("max_id=101"));
        assert!(!sent[2].url.contains("exclude_replies=true"));
    }
    #[test]
    fn help_splits_into_thread_posts_without_truncating_checkpoint_or_instructions() {
        let r = rover();
        let mut http = Scripted::default();
        basics(&http, json!([status("100", "9", &r.encode(), None)]));
        http.answer(
            200,
            json!([{"id":"51","type":"mention","status":status("200","1","help",Some("100"))}]),
        );
        http.answer(
            200,
            json!({"ancestors":[status("100","9",&r.encode(),None)]}),
        );
        answer_narrative(&http, &format!("@alice {}", r.clone().command("help")), 210);
        http.answer(200, json!({"id":"300","url":"u"}));
        assert!(execute(&mut http, start()).is_ok());
        let texts: Vec<String> = http
            .sent
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == "POST")
            .map(|r| {
                let b: Value = serde_json::from_slice(&r.body).unwrap();
                b["status"].as_str().unwrap().to_string()
            })
            .collect();
        assert!(texts.iter().all(|s| s.chars().count() <= 140));
        assert!(texts.join(" ").contains("shared rover"));
        assert!(texts.join(" ").contains("Game save (ignore)"));
        assert_eq!(Rover::parse(texts.last().unwrap()).unwrap().cursor, 51);
    }

    #[test]
    fn daily_tick_commits_once_and_recovers_on_same_day_without_posting_again() {
        let r = rover();
        let day = start() + 36;
        let mut http = Scripted::default();
        basics(&http, json!([status("100", "9", &r.encode(), None)]));
        let mut expected = r.clone();
        let movement = expected.advance(day);
        let message = format!(
            "Mars • Sol 37 (mission day)
{}
Auto-exploration: 36 daily recharge/move cycle(s). Latest: {movement}
{}
Reply scan, sample, map or help.",
            weather(r.seed, day),
            expected.status()
        );
        answer_narrative(&http, &message, 210);
        http.answer(200, json!({"id":"300","url":"u"}));
        http.answer(200, json!([]));
        assert_eq!(
            execute(&mut http, day).unwrap(),
            "Mars: 0 commands; daily log posted"
        );
        let posted = body(&http, 3);
        assert!(posted["status"].as_str().unwrap().contains("Sol 37"));
        let saved = Rover::parse(last_post(&http)["status"].as_str().unwrap()).unwrap();
        let mut expected = r;
        expected.advance(day);
        assert_eq!(saved, expected);
        let mut http = Scripted::default();
        basics(
            &http,
            json!([status("300", "9", &saved.encode(), Some("100"))]),
        );
        http.answer(200, json!([]));
        assert_eq!(execute(&mut http, day).unwrap(), "Mars: 0 commands");
        assert_eq!(http.sent.lock().unwrap().len(), 4);
    }

    #[test]
    fn malformed_history_response_is_an_error_instead_of_a_new_landing() {
        let mut http = Scripted::default();
        basics(&http, json!({"unexpected":"response"}));
        assert!(execute(&mut http, start())
            .unwrap_err()
            .message
            .contains("refusing to reset"));
        assert_eq!(http.sent.lock().unwrap().len(), 3);
    }

    #[test]
    fn readable_posts_fit_and_preserve_every_word() {
        for command in [
            "help",
            "help moves",
            "help energy",
            "help samples",
            "map",
            "scan",
            "status",
            "sample",
        ] {
            let message = format!("@explorer {}", rover().command(command));
            for limit in [140, 500] {
                let parts = narrative_parts(&message, limit).unwrap();
                let mut rebuilt = Vec::new();
                if parts.len() > 1 {
                    rebuilt.push("@explorer");
                }
                for part in &parts {
                    assert!(part.chars().count() <= limit);
                    assert!(part.starts_with("@explorer "));
                    assert!(!part.contains("MR2:"));
                    let content = if parts.len() > 1 {
                        part.split_once("] ").unwrap().1
                    } else {
                        part.as_str()
                    };
                    rebuilt.extend(content.split_whitespace());
                }
                assert_eq!(rebuilt, message.split_whitespace().collect::<Vec<_>>());
            }
        }
    }

    #[test]
    fn map_explains_edges_unknowns_and_revealed_terrain_without_symbols() {
        let mut r = rover();
        r.pos = 0;
        r.scanned = 1;
        let map = r.map();
        assert!(map.contains("North: map edge"));
        assert!(map.contains("East: B1 — unknown"));
        assert!(map.contains("South: A2 — unknown"));
        let scan = r.command("scan");
        assert!(!scan.contains("— unknown"));
        assert!(scan.contains(&format!("East: B1 — {}", terrain(r.seed, 1))));
        assert!(scan.contains("Battery 92/100"));
    }

    #[test]
    fn replies_to_readable_landing_are_in_the_mission() {
        let r = rover();
        let mut http = Scripted::default();
        basics(&http, json!([status("110", "9", &r.encode(), Some("100"))]));
        http.answer(
            200,
            json!([{"id":"51","type":"mention","status":status("200","1","status",Some("100"))}]),
        );
        http.answer(
            200,
            json!({"ancestors":[status("100","9","Mars landing!",None)]}),
        );
        answer_narrative(
            &http,
            &format!("@alice {}", r.clone().command("status")),
            210,
        );
        http.answer(200, json!({"id":"300","url":"u"}));
        assert_eq!(execute(&mut http, start()).unwrap(), "Mars: 1 commands");
        assert_eq!(
            Rover::parse(last_post(&http)["status"].as_str().unwrap())
                .unwrap()
                .cursor,
            51
        );
        // mastomini's own length rules: `--features e2e` (make test).
        #[cfg(feature = "e2e")]
        for post in posted_bodies(&http) {
            mastomini::text::validate_status(
                post["status"].as_str().unwrap(),
                post["spoiler_text"].as_str().unwrap_or(""),
            )
            .unwrap();
        }
    }
}
