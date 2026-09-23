//! Posting, reactions, boosts, follows, and the RAM-only notifications and
//! markers.

use super::*;
use crate::text;

#[derive(Debug, Clone, Default)]
pub struct NewStatus {
    pub text: String,
    pub spoiler_text: String,
    pub visibility: Option<Visibility>,
    pub sensitive: bool,
    pub language: Option<String>,
    pub in_reply_to_id: Option<u64>,
    pub idempotency_key: Option<String>,
    pub app_id: Option<u64>,
}

const MAX_IDEMPOTENCY_KEY: usize = 80;

impl<S: Store> Service<S> {
    fn visible_status(&self, viewer: u8, id: u64) -> Result<&Status> {
        self.state
            .statuses
            .get(&id)
            .filter(|s| self.can_see(Some(viewer), s))
            .ok_or(Error::NotFound)
    }

    /// `POST /api/v1/statuses`. Returns the new (or idempotently repeated)
    /// status id.
    pub fn post_status(&mut self, slot: u8, new: NewStatus, now_ms: u64) -> Result<u64> {
        self.writable()?;
        let author = self.state.account(slot).ok_or(Error::NotFound)?;
        let default_visibility = author.rec.privacy;
        if let Some(key) = &new.idempotency_key {
            if key.len() > MAX_IDEMPOTENCY_KEY {
                return invalid("Idempotency-Key is too long");
            }
            self.state
                .idempotency
                .retain(|(_, _, _, expires)| *expires > now_ms);
            let repeat = self
                .state
                .idempotency
                .iter()
                .find(|(s, k, _, _)| *s == slot && k == key)
                .map(|(_, _, id, _)| *id);
            if let Some(id) = repeat.filter(|id| self.state.statuses.contains_key(id)) {
                return Ok(id);
            }
        }
        let text = new.text.replace("\r\n", "\n");
        let spoiler = new.spoiler_text.trim().to_string();
        text::validate_status(&text, &spoiler).map_err(Error::Invalid)?;
        if new.language.as_ref().is_some_and(|l| l.len() > 8) {
            return invalid("Validation failed: Language is invalid");
        }
        let parent_author = match new.in_reply_to_id {
            Some(parent) => Some(self.visible_status(slot, parent)?.rec.author),
            None => None,
        };
        let mentions: Vec<(u64, u8)> = text::mentions(&text, &self.config.host)
            .iter()
            .filter_map(|name| self.state.account_by_username(name))
            .map(|a| (a.rec.id, a.slot))
            .collect();
        let visibility = new.visibility.unwrap_or(default_visibility);
        // A direct message is readable only by its author and the people it
        // mentions: all of them need a key before anything is written.
        let participants: Vec<u8> = std::iter::once(slot)
            .chain(mentions.iter().map(|(_, s)| *s).filter(|s| *s != slot))
            .collect();
        if visibility == Visibility::Direct {
            self.seal_dm(0, &participants, "", "")?;
        }

        self.govern(Some(slot), now_ms)?;
        self.make_room(Room::Status)?;
        let mut rec = StatusRec {
            id: self.next_id(now_ms),
            author: slot,
            text,
            spoiler_text: spoiler,
            visibility,
            sensitive: new.sensitive,
            language: new.language,
            in_reply_to_id: new.in_reply_to_id,
            in_reply_to_account_id: parent_author
                .and_then(|p| self.state.account(p))
                .map(|a| a.rec.id),
            mentions: mentions.iter().map(|(id, _)| *id).collect(),
            app_id: new.app_id,
            edited_at_ms: None,
        };
        let id = rec.id;
        if visibility == Visibility::Direct {
            // Envelope first; the status record is the commit point. The
            // stored status keeps no text, and stays marked sensitive if it
            // had a content warning.
            let envelope = self.seal_dm(id, &participants, &rec.text, &rec.spoiler_text)?;
            self.put(Ns::Stat, &keys::dm(id), Kind::Dm, &envelope)?;
            self.state.dms.insert(id, envelope);
            rec.sensitive |= !rec.spoiler_text.is_empty();
            rec.text.clear();
            rec.spoiler_text.clear();
        }
        self.put(Ns::Stat, &keys::status(id), Kind::Status, &rec)?;

        let mask = mentions.iter().fold(0, |m, (_, s)| m | bit(*s));
        let tags = text::tags(&rec.text);
        if let Some(parent) = rec
            .in_reply_to_id
            .and_then(|p| self.state.statuses.get_mut(&p))
        {
            parent.replies += 1;
        }
        self.state.statuses.insert(
            id,
            Status {
                rec,
                mentions: mask,
                tags,
                favourited: 0,
                bookmarked: 0,
                pinned: 0,
                boosted: 0,
                muted: 0,
                replies: 0,
            },
        );
        for (_, to) in mentions {
            self.notify(NotificationKind::Mention, to, slot, Some(id), now_ms);
        }
        if let Some(key) = new.idempotency_key {
            if self.state.idempotency.len() >= MAX_IDEMPOTENCY {
                self.state.idempotency.pop_front();
            }
            self.state
                .idempotency
                .push_back((slot, key, id, now_ms + IDEMPOTENCY_MS));
        }
        Ok(id)
    }

    /// Author deletes their own status. Returns it for "delete & redraft".
    pub fn delete_status(&mut self, slot: u8, id: u64, now_ms: u64) -> Result<Status> {
        let status = self.state.statuses.get(&id).ok_or(Error::NotFound)?;
        if status.rec.author != slot {
            return Err(Error::Forbidden("This action is not allowed".into()));
        }
        let copy = status.clone();
        self.govern(Some(slot), now_ms)?;
        self.remove_status(id)?;
        Ok(copy)
    }

    /// Delete or evict: the status key is the commit point; boosts and
    /// reactions that point at it are cleaned up after (and by boot repair
    /// if power is lost in between).
    pub(crate) fn remove_status(&mut self, id: u64) -> Result<()> {
        self.erase(Ns::Stat, &keys::status(id))?;
        if self.state.dms.remove(&id).is_some() {
            self.erase(Ns::Stat, &keys::dm(id))?;
        }
        let Some(status) = self.state.statuses.remove(&id) else {
            return Ok(());
        };
        if let Some(parent) = status
            .rec
            .in_reply_to_id
            .and_then(|p| self.state.statuses.get_mut(&p))
        {
            parent.replies = parent.replies.saturating_sub(1);
        }
        self.state.notifications.retain(|n| n.status != Some(id));
        let boosts: Vec<u64> = self
            .state
            .boosts
            .values()
            .filter(|b| b.target == id)
            .map(|b| b.id)
            .collect();
        for boost in boosts {
            self.erase(Ns::Stat, &keys::boost(boost))?;
            self.state.boosts.remove(&boost);
        }
        let reactions: Vec<(u64, Reaction)> = self
            .state
            .reactions
            .iter()
            .filter(|(_, r)| r.status == id)
            .map(|(k, r)| (*k, *r))
            .collect();
        for (rid, r) in reactions {
            self.erase(Ns::Rx, &keys::reaction(r.kind, r.status, r.slot))?;
            self.state.reactions.remove(&rid);
            self.state.reaction_ids.remove(&(r.kind, r.status, r.slot));
        }
        Ok(())
    }

    pub(crate) fn remove_boost(&mut self, id: u64) -> Result<()> {
        self.erase(Ns::Stat, &keys::boost(id))?;
        if let Some(boost) = self.state.boosts.remove(&id) {
            if let Some(target) = self.state.statuses.get_mut(&boost.target) {
                target.boosted &= !bit(boost.booster);
            }
            self.state.notifications.retain(|n| {
                !(n.kind == NotificationKind::Reblog
                    && n.status == Some(boost.target)
                    && n.from == boost.booster)
            });
        }
        Ok(())
    }

    /// Favourite, bookmark or pin (`on`), or undo it. Repeating is a no-op.
    pub fn set_reaction(
        &mut self,
        slot: u8,
        kind: ReactionKind,
        status_id: u64,
        on: bool,
        now_ms: u64,
    ) -> Result<()> {
        let status = self.visible_status(slot, status_id)?;
        let current = self
            .state
            .reaction_ids
            .get(&(kind, status_id, slot))
            .copied();
        if kind == ReactionKind::Pin && on {
            if status.rec.author != slot {
                return invalid("Validation failed: You can only pin your own posts");
            }
            if status.rec.visibility == Visibility::Direct {
                return invalid("Validation failed: Direct messages can't be pinned");
            }
            let pins = self
                .state
                .reactions
                .values()
                .filter(|r| r.kind == ReactionKind::Pin && r.slot == slot)
                .count();
            if current.is_none() && pins >= MAX_PINS {
                return invalid("Validation failed: You can pin at most 5 posts");
            }
        }
        let author = status.rec.author;
        match (on, current) {
            (true, Some(_)) | (false, None) => return Ok(()),
            (false, Some(rid)) => {
                self.govern(Some(slot), now_ms)?;
                self.erase(Ns::Rx, &keys::reaction(kind, status_id, slot))?;
                self.state.reactions.remove(&rid);
                self.state.reaction_ids.remove(&(kind, status_id, slot));
                self.update_mask(kind, status_id, slot, false);
                return Ok(());
            }
            (true, None) => {}
        }
        self.govern(Some(slot), now_ms)?;
        self.make_room(Room::Reaction)?;
        if !self.state.statuses.contains_key(&status_id) {
            return Err(Error::NotFound); // evicted to make room
        }
        let rid = self.next_id(now_ms);
        self.put(
            Ns::Rx,
            &keys::reaction(kind, status_id, slot),
            Kind::Reaction,
            &ReactionRec { id: rid },
        )?;
        self.state.reactions.insert(
            rid,
            Reaction {
                kind,
                status: status_id,
                slot,
            },
        );
        self.state.reaction_ids.insert((kind, status_id, slot), rid);
        self.update_mask(kind, status_id, slot, true);
        if kind == ReactionKind::Favourite {
            self.notify(
                NotificationKind::Favourite,
                author,
                slot,
                Some(status_id),
                now_ms,
            );
        }
        Ok(())
    }

    fn update_mask(&mut self, kind: ReactionKind, status_id: u64, slot: u8, on: bool) {
        if let Some(status) = self.state.statuses.get_mut(&status_id) {
            let mask = match kind {
                ReactionKind::Favourite => &mut status.favourited,
                ReactionKind::Bookmark => &mut status.bookmarked,
                ReactionKind::Pin => &mut status.pinned,
                ReactionKind::Mute => &mut status.muted,
            };
            if on {
                *mask |= bit(slot);
            } else {
                *mask &= !bit(slot);
            }
        }
        if !on && kind == ReactionKind::Favourite {
            self.state.notifications.retain(|n| {
                !(n.kind == NotificationKind::Favourite
                    && n.status == Some(status_id)
                    && n.from == slot)
            });
        }
    }

    /// Reblog. Returns the boost id. Repeating returns the existing boost.
    pub fn boost(&mut self, slot: u8, status_id: u64, now_ms: u64) -> Result<u64> {
        let status = self.visible_status(slot, status_id)?;
        let author = status.rec.author;
        let boostable = matches!(
            status.rec.visibility,
            Visibility::Public | Visibility::Unlisted
        ) || (status.rec.visibility == Visibility::Private && author == slot);
        if !boostable {
            return Err(Error::Forbidden("This action is not allowed".into()));
        }
        if let Some(existing) = self
            .state
            .boosts
            .values()
            .find(|b| b.booster == slot && b.target == status_id)
        {
            return Ok(existing.id);
        }
        self.govern(Some(slot), now_ms)?;
        self.make_room(Room::Boost)?;
        if !self.state.statuses.contains_key(&status_id) {
            return Err(Error::NotFound);
        }
        let rec = BoostRec {
            id: self.next_id(now_ms),
            booster: slot,
            target: status_id,
        };
        self.put(Ns::Stat, &keys::boost(rec.id), Kind::Boost, &rec)?;
        self.state.boosts.insert(rec.id, rec);
        if let Some(target) = self.state.statuses.get_mut(&status_id) {
            target.boosted |= bit(slot);
        }
        self.notify(
            NotificationKind::Reblog,
            author,
            slot,
            Some(status_id),
            now_ms,
        );
        Ok(rec.id)
    }

    pub fn unboost(&mut self, slot: u8, status_id: u64, now_ms: u64) -> Result<()> {
        self.visible_status(slot, status_id)?;
        let existing = self
            .state
            .boosts
            .values()
            .find(|b| b.booster == slot && b.target == status_id)
            .map(|b| b.id);
        if let Some(id) = existing {
            self.govern(Some(slot), now_ms)?;
            self.remove_boost(id)?;
        }
        Ok(())
    }

    /// Follow (with options) or unfollow. Locked accounts are followed
    /// directly for now: follow requests arrive with the Tier 2 API.
    pub fn set_follow(
        &mut self,
        src: u8,
        dst: u8,
        on: bool,
        reblogs: Option<bool>,
        notify: Option<bool>,
        now_ms: u64,
    ) -> Result<()> {
        if self.state.account(dst).is_none() {
            return Err(Error::NotFound);
        }
        if src == dst {
            return Err(Error::Forbidden("You can't follow yourself".into()));
        }
        if on && self.state.blocked_either(src, dst) {
            return Err(Error::Forbidden("This action is not allowed".into()));
        }
        let existing = self.state.follows.get(&(src, dst)).copied();
        if !on {
            if existing.is_some() {
                self.govern(Some(src), now_ms)?;
                self.erase(Ns::Rel, &keys::follow(src, dst))?;
                self.state.follows.remove(&(src, dst));
            }
            return Ok(());
        }
        let rec = FollowRec {
            id: existing.map_or_else(|| self.next_id(now_ms), |e| e.id),
            reblogs: reblogs.unwrap_or(existing.is_none_or(|e| e.reblogs)),
            notify: notify.unwrap_or(existing.is_some_and(|e| e.notify)),
        };
        if existing == Some(rec) {
            return Ok(());
        }
        self.govern(Some(src), now_ms)?;
        self.put(Ns::Rel, &keys::follow(src, dst), Kind::Follow, &rec)?;
        self.state.follows.insert((src, dst), rec);
        if existing.is_none() {
            self.notify(NotificationKind::Follow, dst, src, None, now_ms);
        }
        Ok(())
    }

    pub fn dismiss_notification(&mut self, slot: u8, id: u64) -> Result<()> {
        let before = self.state.notifications.len();
        self.state
            .notifications
            .retain(|n| !(n.to == slot && n.id == id));
        if before == self.state.notifications.len() {
            return Err(Error::NotFound);
        }
        Ok(())
    }

    pub fn clear_notifications(&mut self, slot: u8) {
        self.state.notifications.retain(|n| n.to != slot);
    }

    /// Markers are RAM only (spec/02 "Ephemeral state stays in RAM").
    pub fn set_marker(
        &mut self,
        slot: u8,
        timeline: usize,
        last_read_id: u64,
        now_ms: u64,
    ) -> Marker {
        let entry = &mut self.state.markers[slot as usize][timeline];
        let marker = Marker {
            last_read_id,
            version: entry.map_or(0, |m| m.version + 1),
            updated_ms: now_ms,
        };
        *entry = Some(marker);
        marker
    }
}
