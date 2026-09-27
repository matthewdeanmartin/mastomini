//! Durable drafts; the bots board owns the clock and calls back to publish.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_SCHEDULED: usize = 16;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Bridge {
    pub bots_url: String,
    /// An existing Mastodon API key, also configured on the bots board.
    pub api_key: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Scheduled {
    pub id: u64,
    pub account_id: u64,
    pub epoch: u32,
    pub at: u64,
    pub revision: u64,
    pub post: NewStatus,
    pub target: Option<u64>,
    pub done: bool,
    pub cancelled: bool,
    pub failure: Option<String>,
}

#[derive(Default)]
pub struct Scheduling {
    pub last_handoff: u64,
    pub last_error: Option<String>,
    pub bridge: Bridge,
    pub jobs: BTreeMap<u64, Scheduled>,
    pub acknowledged: BTreeSet<(u64, u64)>,
}

impl Scheduled {
    /// Only metadata and the submission idempotency key stay in internal RAM.
    fn index(mut self) -> Self {
        let idempotency_key = self.post.idempotency_key.take();
        self.post = NewStatus {
            idempotency_key,
            ..Default::default()
        };
        self
    }
}

fn job_key(id: u64) -> Key {
    Key::from_parts(&[b"q", &ids::b32(id)]).expect("fits")
}

/// Only an origin, never a path or URL containing credentials. Plain HTTP is
/// restricted to loopback development; board-to-board credentials require TLS.
pub fn origin(value: &str) -> bool {
    let Some((scheme, host)) = value.split_once("://") else {
        return false;
    };
    let local = host == "localhost"
        || host == "127.0.0.1"
        || host.starts_with("localhost:")
        || host.starts_with("127.0.0.1:");
    !host.is_empty()
        && value.len() <= 255
        && (scheme == "https" || (cfg!(feature = "desktop") && scheme == "http" && local))
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
}

impl<S: Store> Service<S> {
    pub(crate) fn load_scheduled(&mut self) -> core::result::Result<(), StoreError> {
        if let Some(bytes) = self.store.get(Ns::Cfg, &Key::new("scheduler")?)? {
            self.scheduling.bridge = codec::decode(Kind::Scheduler, &bytes)?;
        }
        for (key, bytes) in load(&mut self.store, Ns::Cfg)? {
            if !key.as_str().starts_with('q') {
                continue;
            }
            let mut job: Scheduled = codec::decode(Kind::Scheduled, &bytes)?;
            if key != job_key(job.id) || self.scheduling.jobs.len() >= MAX_SCHEDULED {
                return Err(corrupt(&key, "invalid scheduled job"));
            }
            if self.state.account_by_id(job.account_id).is_none() && !job.cancelled {
                job.cancelled = true;
                job.failure = None;
                job.revision += 1;
                job = job.index();
                self.store
                    .set(Ns::Cfg, &key, &codec::encode(Kind::Scheduled, &job)?)?;
            }
            self.ids.observe(job.id);
            if let Some(id) = job.target {
                self.ids.observe(id);
                // A status write is the publication commit point. Repair an
                // interrupted acknowledgement before any deletion can occur.
                if self.state.statuses.contains_key(&id) && !job.done {
                    job.done = true;
                    self.store
                        .set(Ns::Cfg, &key, &codec::encode(Kind::Scheduled, &job)?)?;
                }
            }
            self.scheduling.jobs.insert(job.id, job.index());
        }
        Ok(())
    }

    pub fn set_scheduler(&mut self, bridge: Bridge) -> Result<()> {
        if !origin(&bridge.bots_url) || bridge.api_key.len() > 256 || bridge.api_key.is_empty() {
            return invalid("Use an HTTPS bots origin and an existing API key");
        }
        self.put(
            Ns::Cfg,
            &Key::new("scheduler").unwrap(),
            Kind::Scheduler,
            &bridge,
        )?;
        self.scheduling.bridge = bridge;
        self.scheduling.acknowledged.clear();
        Ok(())
    }

    pub fn scheduled_post(&mut self, id: u64) -> Result<Scheduled> {
        let bytes = self
            .store
            .get(Ns::Cfg, &job_key(id))
            .map_err(|e| self.map_store_error(e))?
            .ok_or(Error::NotFound)?;
        codec::decode(Kind::Scheduled, &bytes).map_err(|e| self.map_store_error(e))
    }

    fn save_scheduled(&mut self, job: Scheduled) -> Result<()> {
        self.put(Ns::Cfg, &job_key(job.id), Kind::Scheduled, &job)?;
        self.scheduling.jobs.insert(job.id, job.index());
        Ok(())
    }

    pub fn schedule_post(
        &mut self,
        slot: u8,
        mut post: NewStatus,
        at: u64,
        now: u64,
    ) -> Result<u64> {
        if self.scheduling.bridge.api_key.is_empty() {
            return Err(Error::Unavailable(
                "Configure the bots scheduler first".into(),
            ));
        }
        let account = self.state.account(slot).ok_or(Error::NotFound)?;
        let account_id = account.rec.id;
        let epoch = account.rec.token_epoch;
        if let Some(key) = &post.idempotency_key {
            if key.len() > 80 {
                return invalid("Idempotency-Key is too long");
            }
            if let Some(job) = self.scheduling.jobs.values().find(|j| {
                j.account_id == account_id && j.post.idempotency_key.as_ref() == Some(key)
            }) {
                return Ok(job.id);
            }
        }
        if at < now.saturating_add(300_000) {
            return invalid("Schedule at least five minutes ahead");
        }
        post.visibility = Some(post.visibility.unwrap_or(account.rec.privacy));
        if post.visibility == Some(Visibility::Direct) {
            return invalid(
                "Scheduled direct messages are not supported: their keys are session-only",
            );
        }
        if post.text.len() + post.spoiler_text.len() > 2048
            || post.language.as_ref().is_some_and(|s| s.len() > 8)
        {
            return invalid("Scheduled post parameters are too large");
        }
        crate::text::validate_status(&post.text, &post.spoiler_text).map_err(Error::Invalid)?;
        if let Some(poll) = &post.poll {
            super::polls::validate_poll(poll)?;
        }
        if let Some(id) = post.in_reply_to_id {
            if self.visible(Some(slot), id).is_none() {
                return Err(Error::NotFound);
            }
        }
        // Retired jobs are only receipts, and unknown callbacks are terminal.
        let retired: Vec<_> = self
            .scheduling
            .jobs
            .values()
            .filter(|j| {
                j.done
                    || (j.cancelled && self.scheduling.acknowledged.contains(&(j.id, j.revision)))
            })
            .map(|j| j.id)
            .collect();
        for id in retired {
            self.erase(Ns::Cfg, &job_key(id))?;
            self.scheduling.jobs.remove(&id);
            self.scheduling.acknowledged.retain(|(old, _)| *old != id);
        }
        if self.scheduling.jobs.len() >= MAX_SCHEDULED {
            return invalid("Scheduled post queue is full (16)");
        }
        self.govern(Some(slot), now)?;
        let id = self.next_id(now);
        self.save_scheduled(Scheduled {
            id,
            account_id,
            epoch,
            at,
            revision: 1,
            post,
            target: None,
            done: false,
            cancelled: false,
            failure: None,
        })?;
        Ok(id)
    }

    pub fn change_schedule(&mut self, slot: u8, id: u64, at: Option<u64>, now: u64) -> Result<()> {
        let account_id = self.state.account(slot).ok_or(Error::NotFound)?.rec.id;
        let mut job = self.scheduled_post(id)?;
        if job.account_id != account_id || job.done || job.cancelled {
            return Err(Error::NotFound);
        }
        if job
            .target
            .is_some_and(|id| self.state.statuses.contains_key(&id))
        {
            return invalid("The post has already been published");
        }
        job.target = None; // A reserved ID without a committed status is safe to discard.
        if at.is_some() && job.failure.is_some() {
            return invalid("Cancel the failed job and schedule a new post");
        }
        if let Some(at) = at {
            if at < now.saturating_add(300_000) {
                return invalid("Schedule at least five minutes ahead");
            }
            job.at = at;
        } else {
            job.cancelled = true;
            job.failure = None;
            job = job.index();
        }
        job.revision += 1;
        self.govern(Some(slot), now)?;
        self.save_scheduled(job)?;
        self.scheduling.acknowledged.retain(|(old, _)| *old != id);
        Ok(())
    }

    pub(super) fn cancel_account_schedules(&mut self, account_id: u64) -> Result<()> {
        let jobs: Vec<_> = self
            .scheduling
            .jobs
            .values()
            .filter(|j| j.account_id == account_id && !j.cancelled)
            .cloned()
            .collect();
        for mut job in jobs {
            job.cancelled = true;
            job.failure = None;
            job.revision += 1;
            self.scheduling.acknowledged.retain(|(id, _)| *id != job.id);
            // Metadata indexes contain no draft body: erase it from flash too.
            self.save_scheduled(job)?;
        }
        Ok(())
    }

    pub fn publish_scheduled(&mut self, id: u64, revision: u64, now: u64) -> Result<Option<u64>> {
        self.writable()?;
        let mut job = self.scheduled_post(id)?;
        if job.cancelled || job.revision != revision {
            return Err(Error::NotFound);
        }
        if job.done {
            return Ok(job.target);
        }
        if job.failure.is_some() {
            return Ok(None);
        }
        if now < job.at {
            return Err(Error::Unavailable("Not due yet".into()));
        }
        let Some(account) = self.state.account_by_id(job.account_id) else {
            job.failure = Some("The author no longer exists".into());
            self.save_scheduled(job)?;
            return Ok(None);
        };
        let slot = account.slot;
        if account.rec.token_epoch != job.epoch
            || account.rec.disabled
            || self.state.moderation(slot).suspended_ms.is_some()
        {
            job.failure = Some("The author can no longer publish this job".into());
            self.save_scheduled(job)?;
            return Ok(None);
        }
        let target = match job.target {
            Some(id) => id,
            None => {
                let target = self.next_id(now);
                job.target = Some(target);
                self.save_scheduled(job.clone())?;
                target
            }
        };
        if !self.state.statuses.contains_key(&target) {
            let mut post = job.post.clone();
            post.idempotency_key = None;
            if let Err(error) = self.post_status_with_id(slot, post, now, Some(target)) {
                if matches!(
                    error,
                    Error::NotFound | Error::Forbidden(_) | Error::Invalid(_)
                ) {
                    job.failure = Some(error.to_string());
                    self.save_scheduled(job)?;
                    return Ok(None);
                }
                return Err(error);
            }
        }
        job.done = true;
        self.save_scheduled(job)?;
        Ok(Some(target))
    }
}
