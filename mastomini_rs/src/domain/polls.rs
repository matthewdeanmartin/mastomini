//! Polls (spec/04 "Polls"): up to 4 options, one bit per account slot for
//! each option, ending within a week. Not allowed in direct messages, whose
//! options would sit unencrypted in flash.

use super::*;

/// `poll[...]` from a new or edited status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPoll {
    pub options: Vec<String>,
    pub expires_in_s: u64,
    pub multiple: bool,
    pub hide_totals: bool,
}

pub(crate) fn validate_poll(p: &NewPoll) -> Result<()> {
    if p.options.len() < 2 || p.options.len() > MAX_POLL_OPTIONS {
        return invalid("Validation failed: A poll needs 2 to 4 options");
    }
    for (i, option) in p.options.iter().enumerate() {
        if option.trim().is_empty() {
            return invalid("Validation failed: Poll options can't be blank");
        }
        if option.chars().count() > MAX_POLL_OPTION_CHARS {
            return invalid("Validation failed: Poll options are at most 50 characters");
        }
        if p.options[..i].contains(option) {
            return invalid("Validation failed: Poll options must be different");
        }
    }
    if !(POLL_MIN_SECONDS..=POLL_MAX_SECONDS).contains(&p.expires_in_s) {
        return invalid("Validation failed: A poll lasts from 5 minutes to 7 days");
    }
    Ok(())
}

impl NewPoll {
    pub(crate) fn record(&self, now_ms: u64) -> PollRec {
        PollRec {
            options: self.options.iter().map(|o| o.trim().to_string()).collect(),
            expires_ms: now_ms + self.expires_in_s * 1000,
            multiple: self.multiple,
            hide_totals: self.hide_totals,
            votes: vec![0; self.options.len()],
        }
    }
}

impl<S: Store> Service<S> {
    /// Vote on someone else's open poll, once. `choices` are option indexes.
    pub fn vote(&mut self, slot: u8, status_id: u64, choices: &[usize], now_ms: u64) -> Result<()> {
        let author = self
            .visible(Some(slot), status_id)
            .ok_or(Error::NotFound)?
            .rec
            .author;
        let poll = self.state.polls.get(&status_id).ok_or(Error::NotFound)?;
        if author == slot {
            return invalid("Validation failed: You can't vote in your own poll");
        }
        if poll.expires_ms <= now_ms {
            return invalid("Validation failed: The poll has already ended");
        }
        if poll.voters() & bit(slot) != 0 {
            return invalid("Validation failed: You have already voted in this poll");
        }
        let mut unique = choices.to_vec();
        unique.sort_unstable();
        unique.dedup();
        if unique.is_empty()
            || (!poll.multiple && unique.len() > 1)
            || unique.iter().any(|c| *c >= poll.options.len())
        {
            return invalid("Validation failed: Choose one of the poll's options");
        }
        let mut rec = poll.clone();
        for c in unique {
            rec.votes[c] |= bit(slot);
        }
        self.govern(Some(slot), now_ms)?;
        self.put(Ns::Stat, &keys::poll(status_id), Kind::Poll, &rec)?;
        self.state.polls.insert(status_id, rec);
        Ok(())
    }

    /// Tell the author and voters when a poll ends. Runs on every request;
    /// polls that ended before this boot are not announced again.
    pub(crate) fn announce_polls(&mut self, now_ms: u64) {
        if self
            .state
            .next_poll_check
            .is_some_and(|deadline| now_ms < deadline)
        {
            return;
        }
        let ended: Vec<(u64, u16)> = self
            .state
            .polls
            .iter()
            .filter(|(id, p)| p.expires_ms <= now_ms && !self.state.polls_announced.contains(id))
            .map(|(id, p)| (*id, p.voters()))
            .collect();
        for (id, voters) in ended {
            self.state.polls_announced.insert(id);
            if !self.state.polls_primed {
                continue;
            }
            let Some(author) = self.state.statuses.get(&id).map(|s| s.rec.author) else {
                continue;
            };
            let to = (0..MAX_ACCOUNTS as u8).filter(|s| *s == author || voters & bit(*s) != 0);
            for slot in to.collect::<Vec<_>>() {
                self.notify(NotificationKind::Poll, slot, author, Some(id), now_ms);
            }
        }
        self.state.polls_primed = true;
        self.state.next_poll_check = Some(
            self.state
                .polls
                .iter()
                .filter(|(id, _)| !self.state.polls_announced.contains(id))
                .map(|(_, p)| p.expires_ms)
                .min()
                .unwrap_or(u64::MAX),
        );
    }

    /// A departing member's votes, so a reused slot doesn't inherit them.
    pub(crate) fn forget_votes(&mut self, slot: u8) -> Result<()> {
        let voted: Vec<u64> = self
            .state
            .polls
            .iter()
            .filter(|(_, p)| p.voters() & bit(slot) != 0)
            .map(|(id, _)| *id)
            .collect();
        for id in voted {
            let Some(mut rec) = self.state.polls.get(&id).cloned() else {
                continue;
            };
            for v in &mut rec.votes {
                *v &= !bit(slot);
            }
            self.put(Ns::Stat, &keys::poll(id), Kind::Poll, &rec)?;
            self.state.polls.insert(id, rec);
        }
        Ok(())
    }
}
