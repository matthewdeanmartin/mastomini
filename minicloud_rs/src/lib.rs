pub mod broker;
pub mod http;
pub mod layout;
#[cfg(feature = "sqlite")]
pub mod sql;
pub mod store;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use store::{Blob, Store};

pub type Shared = Arc<Mutex<Cloud>>;
pub type Result<T> = std::result::Result<T, Error>;
pub const MAX_PAYLOAD: usize = 2048;
pub const MAX_JOBS: usize = 12;
pub const MAX_NOTICES: usize = 24;
pub const NOTICE_LIFETIME: u64 = 24 * 60 * 60;
pub fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[derive(Debug)]
pub struct Error {
    pub status: u16,
    pub message: String,
}
impl Error {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::new(507, e.to_string())
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::new(400, e.to_string())
    }
}
pub fn name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        && s != "."
        && s != ".."
}

#[derive(Clone, Default, Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TextSize {
    Small,
    #[default]
    Medium,
    Large,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ImageRef {
    pub bucket: String,
    pub key: String,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Notice {
    pub source: String,
    pub id: String,
    pub recipient: String,
    pub text: String,
    #[serde(default)]
    pub expires_at: u64,
    #[serde(default)]
    pub size: TextSize,
    #[serde(default)]
    pub image: Option<ImageRef>,
}
impl Notice {
    pub fn identity(&self) -> String {
        format!("{}/{}", self.source, self.id)
    }
    pub fn validate(&self) -> Result<()> {
        if !name(&self.source)
            || !name(&self.id)
            || self.recipient.len() > 64
            || self.text.len() > 256
            || self.text.trim().is_empty()
        {
            return Err(Error::new(400, "source/id must be safe identifiers; recipient <=64 and nonblank text <=256 UTF-8 bytes"));
        }
        if let Some(image) = &self.image {
            store::object_name(&image.bucket, &image.key)?;
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Job {
    pub id: String,
    pub topic: String,
    pub payload: Value,
    pub target: String,
    pub attempts: u8,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Receipt {
    pub id: String,
    pub status: String,
    pub detail: Value,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct State {
    pub revision: u64,
    pub blobs: Vec<Blob>,
    pub notices: Vec<Notice>,
    pub dismissed: VecDeque<String>,
    pub jobs: VecDeque<Job>,
    pub receipts: VecDeque<Receipt>,
}
pub trait Plugin: Send {
    fn name(&self) -> &'static str;
    fn accepts(&self, topic: &str) -> bool;
    // Plugins can only mutate the pending transaction. Commit and job acknowledgment
    // happen together. External side effects must implement their own idempotency.
    fn run(&self, job: &Job, state: &mut State, store: &Store) -> Result<Value>;
}
include!(concat!(env!("OUT_DIR"), "/plugins.rs"));

pub struct Cloud {
    pub store: Store,
    pub state: State,
    pub plugins: Vec<Box<dyn Plugin>>,
    pub token: String,
    pub broker: broker::Hub,
    cursor: usize,
    rotated: Instant,
    screen_revision: u64,
    screen_order: Vec<(usize, u64)>,
    clock: fn() -> u64,
}
impl Cloud {
    pub fn open(root: &Path, token: String) -> Result<Self> {
        Self::open_with_clock(root, token, unix_time)
    }
    pub fn open_with_clock(root: &Path, token: String, clock: fn() -> u64) -> Result<Self> {
        if token.len() < 16 {
            return Err(Error::new(
                400,
                "admin token must have at least 16 characters",
            ));
        }
        let store = Store::open(root)?;
        let state = store.load()?;
        Ok(Self {
            store,
            state,
            plugins: plugins(),
            token,
            broker: broker::Hub::default(),
            cursor: 0,
            rotated: Instant::now(),
            screen_revision: u64::MAX,
            screen_order: Vec::new(),
            clock,
        })
    }
    pub fn commit(&mut self, mut next: State) -> Result<()> {
        next.revision = self
            .state
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::new(507, "revision exhausted"))?;
        self.store.save(&next)?;
        self.state = next;
        Ok(())
    }
    pub fn enqueue(
        &mut self,
        topic: &str,
        mut payload: Value,
        target: Option<&str>,
    ) -> Result<String> {
        self.expire()?;
        if (topic == "minicloud/screen/notify"
            || topic == "minicloud/image/show"
            || target == Some("screen"))
            && topic != "minicloud/screen/read"
        {
            let now = (self.clock)();
            if now < 1_700_000_000 {
                return Err(Error::new(503, "screen clock is not synchronized yet"));
            }
            let object = payload
                .as_object_mut()
                .ok_or_else(|| Error::new(400, "expected JSON object"))?;
            let expires = match object.get("expires_at") {
                None => now + NOTICE_LIFETIME,
                Some(value) => value
                    .as_u64()
                    .filter(|v| *v > 0)
                    .map(|v| v.min(now + NOTICE_LIFETIME))
                    .ok_or_else(|| Error::new(400, "expires_at must be positive Unix seconds"))?,
            };
            object.insert("expires_at".into(), json!(expires));
        }
        let bytes = serde_json::to_vec(&payload)?;
        if bytes.len() > MAX_PAYLOAD {
            return Err(Error::new(413, "plugin payload exceeds 2048 bytes"));
        }
        let id = payload
            .get("event_id")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::new(400, "event_id is required for durable deduplication"))?;
        if !name(id) {
            return Err(Error::new(400, "invalid event_id"));
        }
        let plugin = self
            .plugins
            .iter()
            .find(|p| target.map_or_else(|| p.accepts(topic), |t| p.name() == t))
            .ok_or_else(|| Error::new(404, "no compiled plugin handles this topic/target"))?;
        let job_id = format!("{}:{id}", plugin.name());
        if self.state.jobs.iter().any(|j| j.id == job_id)
            || self.state.receipts.iter().any(|r| r.id == job_id)
        {
            return Ok(job_id);
        }
        if self.state.jobs.len() >= MAX_JOBS {
            return Err(Error::new(429, "plugin queue is full"));
        }
        let job = Job {
            id: job_id.clone(),
            topic: topic.into(),
            payload,
            target: plugin.name().into(),
            attempts: 0,
        };
        let mut next = self.state.clone();
        next.jobs.push_back(job);
        self.commit(next)?;
        Ok(job_id)
    }
    pub fn dismiss(&mut self, source: &str, id: &str) -> Result<()> {
        if !name(source) || !name(id) {
            return Err(Error::new(400, "invalid notification identity"));
        }
        let identity = format!("{source}/{id}");
        if self.state.dismissed.contains(&identity) {
            return Ok(());
        }
        let mut next = self.state.clone();
        dismiss_state(&mut next, &identity);
        self.commit(next)?;
        self.publish(
            "minicloud/screen/changed",
            json!({"revision":self.state.revision}),
        )?;
        Ok(())
    }
    pub fn work_once(&mut self) -> Result<()> {
        self.expire()?;
        let Some(job) = self.state.jobs.front().cloned() else {
            return Ok(());
        };
        let mut next = self.state.clone();
        let expired = job.target == "screen"
            && job.topic != "minicloud/screen/read"
            && job.payload["expires_at"]
                .as_u64()
                .is_some_and(|t| t <= self.now());
        let result = if expired {
            Ok(json!({"expired":true}))
        } else {
            self.plugins
                .iter()
                .find(|p| p.name() == job.target)
                .ok_or_else(|| Error::new(404, "plugin removed from this build"))
                .and_then(|p| p.run(&job, &mut next, &self.store))
        };
        let receipt = match result {
            Ok(value) => Receipt {
                id: job.id.clone(),
                status: "done".into(),
                detail: value,
            },
            Err(error) => {
                // Discard partial plugin mutations. Transient storage errors get
                // three attempts, then an inspectable failed receipt.
                next = self.state.clone();
                if error.status >= 500 && job.attempts < 2 {
                    let mut retry = next.jobs.pop_front().unwrap();
                    retry.attempts += 1;
                    next.jobs.push_back(retry);
                    return self.commit(next);
                }
                Receipt {
                    id: job.id.clone(),
                    status: "failed".into(),
                    detail: json!({"error":error.message}),
                }
            }
        };
        next.jobs.pop_front();
        if next.receipts.len() == 32 {
            next.receipts.pop_front();
        }
        next.receipts.push_back(receipt.clone());
        self.commit(next)?;
        self.publish("minicloud/jobs/result", serde_json::to_value(receipt)?)?;
        self.publish(
            "minicloud/screen/changed",
            json!({"revision":self.state.revision}),
        )?;
        Ok(())
    }
    pub fn publish(&mut self, topic: &str, value: Value) -> Result<()> {
        self.broker
            .publish(topic, &serde_json::to_vec(&value)?, false)
    }
    fn rotation(&self) -> Vec<usize> {
        let stats: Vec<_> = self
            .state
            .notices
            .iter()
            .enumerate()
            .filter(|(_, n)| n.source == "nanacoin" && n.id.starts_with("stats-"))
            .map(|(i, _)| i)
            .collect();
        let messages: Vec<_> = self
            .state
            .notices
            .iter()
            .enumerate()
            .filter(|(_, n)| !(n.source == "nanacoin" && n.id.starts_with("stats-")))
            .map(|(i, _)| i)
            .collect();
        if messages.is_empty() {
            return stats;
        }
        if stats.is_empty() {
            return messages;
        }
        let mut order = Vec::new();
        for index in 0..messages.len().max(stats.len()) {
            order.push(messages[index % messages.len()]);
            order.push(stats[index % stats.len()]);
        }
        order
    }
    pub fn current(&mut self) -> Option<Notice> {
        if self.screen_revision != self.state.revision {
            self.screen_order = self
                .rotation()
                .into_iter()
                .map(|index| {
                    let n = &self.state.notices[index];
                    (index, 8 * layout::pages(&n.text, &n.size).len() as u64)
                })
                .collect();
            self.screen_revision = self.state.revision;
        }
        if self.screen_order.is_empty() {
            return None;
        }
        let (_, duration) = self.screen_order[self.cursor % self.screen_order.len()];
        if self.rotated.elapsed() >= Duration::from_secs(duration) {
            self.advance();
        }
        let (index, _) = self.screen_order[self.cursor % self.screen_order.len()];
        Some(self.state.notices[index].clone())
    }
    pub fn screen_page(&self) -> usize {
        (self.rotated.elapsed().as_secs() / 8) as usize
    }
    pub fn now(&self) -> u64 {
        (self.clock)()
    }
    pub fn expire(&mut self) -> Result<()> {
        let now = self.now();
        if now < 1_700_000_000 || !self.state.notices.iter().any(|n| n.expires_at <= now) {
            return Ok(());
        }
        let mut next = self.state.clone();
        let expired: Vec<_> = next
            .notices
            .iter()
            .filter(|n| n.expires_at <= now)
            .map(Notice::identity)
            .collect();
        for id in expired {
            dismiss_state(&mut next, &id);
        }
        self.commit(next)?;
        self.publish(
            "minicloud/screen/changed",
            json!({"revision":self.state.revision}),
        )
    }
    pub fn advance(&mut self) {
        self.cursor = self.cursor.wrapping_add(1);
        self.rotated = Instant::now();
    }
}
pub fn dismiss_state(state: &mut State, identity: &str) {
    state.notices.retain(|n| n.identity() != identity);
    if !state.dismissed.iter().any(|s| s == identity) {
        if state.dismissed.len() == 64 {
            state.dismissed.pop_front();
        }
        state.dismissed.push_back(identity.into());
    }
}
