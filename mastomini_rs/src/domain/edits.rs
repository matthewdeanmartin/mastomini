//! Editing statuses (`PUT /api/v1/statuses/:id`) and their history
//! (spec/02 "Crash consistency": write the old revision, then overwrite the
//! status, which is the commit point).
//!
//! Up to three previous versions per status and 1,024 in all, oldest dropped
//! first. Direct messages are re-encrypted and keep no history, which would
//! otherwise sit unencrypted in flash.

use super::polls::{validate_poll, NewPoll};
use super::*;
use crate::text;

/// What an edit may change. Visibility and the reply target can't change,
/// as on Mastodon.
#[derive(Debug, Clone, Default)]
pub struct StatusEdit {
    pub text: String,
    pub spoiler_text: String,
    pub sensitive: bool,
    pub language: Option<String>,
    /// `None` removes a poll, as Mastodon does when an edit omits it.
    pub poll: Option<NewPoll>,
}

impl<S: Store> Service<S> {
    /// Previous versions of a status, oldest first.
    pub fn revisions(&self, id: u64) -> Vec<&RevisionRec> {
        let mut out: Vec<&RevisionRec> = self
            .state
            .history
            .range((id, 0)..=(id, MAX_REVISIONS_PER_STATUS - 1))
            .map(|(_, r)| r)
            .collect();
        out.sort_by_key(|r| r.created_ms);
        out
    }

    pub(crate) fn erase_history(&mut self, id: u64) -> Result<()> {
        let keys: Vec<(u64, u8)> = self
            .state
            .history
            .range((id, 0)..=(id, MAX_REVISIONS_PER_STATUS - 1))
            .map(|(k, _)| *k)
            .collect();
        for (status, n) in keys {
            self.erase(Ns::Hist, &keys::revision(status, n))?;
            self.state.history.remove(&(status, n));
        }
        Ok(())
    }

    /// Keep `rec` as a previous version of `id`, making room first.
    fn keep_revision(&mut self, id: u64, rec: RevisionRec) -> Result<()> {
        if self.state.history.len() >= MAX_REVISIONS {
            let oldest = self
                .state
                .history
                .iter()
                .min_by_key(|(_, r)| r.created_ms)
                .map(|(k, _)| *k);
            if let Some((status, n)) = oldest {
                self.erase(Ns::Hist, &keys::revision(status, n))?;
                self.state.history.remove(&(status, n));
            }
        }
        let used: Vec<(u8, u64)> = self
            .state
            .history
            .range((id, 0)..=(id, MAX_REVISIONS_PER_STATUS - 1))
            .map(|((_, n), r)| (*n, r.created_ms))
            .collect();
        let n = (0..MAX_REVISIONS_PER_STATUS)
            .find(|n| !used.iter().any(|(u, _)| u == n))
            .or_else(|| used.iter().min_by_key(|(_, t)| *t).map(|(n, _)| *n))
            .unwrap_or(0);
        self.put(Ns::Hist, &keys::revision(id, n), Kind::Revision, &rec)?;
        self.state.history.insert((id, n), rec);
        Ok(())
    }

    /// The author edits their status.
    pub fn edit_status(&mut self, slot: u8, id: u64, edit: StatusEdit, now_ms: u64) -> Result<()> {
        self.writable()?;
        let status = self.state.statuses.get(&id).ok_or(Error::NotFound)?;
        if status.rec.author != slot {
            return Err(Error::Forbidden("This action is not allowed".into()));
        }
        let old = status.rec.clone();
        let old_mentions = status.mentions;
        let text = edit.text.replace("\r\n", "\n");
        let spoiler = edit.spoiler_text.trim().to_string();
        text::validate_status(&text, &spoiler).map_err(Error::Invalid)?;
        if edit.language.as_ref().is_some_and(|l| l.len() > 8) {
            return invalid("Validation failed: Language is invalid");
        }
        let direct = old.visibility == Visibility::Direct;
        if let Some(poll) = &edit.poll {
            if direct {
                return invalid("Validation failed: Direct messages can't have polls");
            }
            validate_poll(poll)?;
        }
        let mentions: Vec<(u64, u8)> = text::mentions(&text, &self.config.host)
            .iter()
            .filter_map(|name| self.state.account_by_username(name))
            .map(|a| (a.rec.id, a.slot))
            .collect();
        let participants: Vec<u8> = std::iter::once(slot)
            .chain(mentions.iter().map(|(_, s)| *s).filter(|s| *s != slot))
            .collect();
        let envelope = if direct {
            Some(self.seal_dm(id, &participants, &text, &spoiler)?)
        } else {
            None
        };
        self.govern(Some(slot), now_ms)?;

        // The current version becomes history (never for direct messages).
        let since = old.edited_at_ms.unwrap_or(ids::millis(id));
        if !direct {
            self.keep_revision(
                id,
                RevisionRec {
                    text: old.text.clone(),
                    spoiler_text: old.spoiler_text.clone(),
                    sensitive: old.sensitive,
                    created_ms: since,
                },
            )?;
        }
        // An unchanged poll keeps its votes; a changed one starts over.
        let old_poll = self.state.polls.get(&id).cloned();
        let new_poll = edit.poll.as_ref().map(|p| match &old_poll {
            Some(existing)
                if existing.options
                    == p.options
                        .iter()
                        .map(|o| o.trim().to_string())
                        .collect::<Vec<_>>()
                    && existing.multiple == p.multiple =>
            {
                PollRec {
                    hide_totals: p.hide_totals,
                    ..existing.clone()
                }
            }
            _ => p.record(now_ms),
        });
        match (&new_poll, &old_poll) {
            (Some(poll), _) if Some(poll) != old_poll.as_ref() => {
                self.put(Ns::Stat, &keys::poll(id), Kind::Poll, poll)?;
                self.state.polls.insert(id, poll.clone());
                self.state.polls_announced.remove(&id);
            }
            (None, Some(_)) => {
                self.erase(Ns::Stat, &keys::poll(id))?;
                self.state.polls.remove(&id);
            }
            _ => {}
        }

        let mut rec = StatusRec {
            text,
            spoiler_text: spoiler,
            sensitive: edit.sensitive,
            language: edit.language,
            mentions: mentions.iter().map(|(id, _)| *id).collect(),
            // Strictly after the version it replaces, so boot can tell a
            // finished edit from an interrupted one.
            edited_at_ms: Some(now_ms.max(since + 1)),
            ..old
        };
        if let Some(envelope) = envelope {
            self.put(Ns::Stat, &keys::dm(id), Kind::Dm, &envelope)?;
            self.state.dms.insert(id, envelope);
            rec.sensitive |= !rec.spoiler_text.is_empty();
            rec.text.clear();
            rec.spoiler_text.clear();
        }
        self.put(Ns::Stat, &keys::status(id), Kind::Status, &rec)?;

        let mask = mentions.iter().fold(0, |m, (_, s)| m | bit(*s));
        let tags = text::tags(&rec.text);
        let boosted = match self.state.statuses.get_mut(&id) {
            Some(s) => {
                s.rec = rec;
                s.mentions = mask;
                s.tags = tags;
                s.boosted
            }
            None => 0,
        };
        for (_, to) in mentions {
            if old_mentions & bit(to) == 0 {
                self.notify(NotificationKind::Mention, to, slot, Some(id), now_ms);
            }
        }
        // As on Mastodon, people who boosted it hear about the edit.
        for to in 0..MAX_ACCOUNTS as u8 {
            if boosted & bit(to) != 0 {
                self.notify(NotificationKind::Update, to, slot, Some(id), now_ms);
            }
        }
        Ok(())
    }
}
