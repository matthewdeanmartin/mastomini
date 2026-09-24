//! Blocks, mutes, conversation mutes, reports, and what household admins can
//! do about a member: the Mastodon moderation actions plus deleting posts
//! and accounts (spec/04 "Moderation").

use super::query::{self, PageQuery};
use super::*;

pub const MAX_REPORT_COMMENT_CHARS: usize = 500;
pub const MAX_REPORT_COMMENT_BYTES: usize = 1000;
pub const MAX_REPORT_STATUSES: usize = 10;
const MAX_THREAD_DEPTH: usize = 40;

/// `POST /api/v1/reports`.
#[derive(Debug, Clone, Default)]
pub struct NewReport {
    pub target: u8,
    pub status_ids: Vec<u64>,
    pub comment: String,
    pub category: Option<ReportCategory>,
    /// 1-based, as `/api/v1/instance/rules` numbers them.
    pub rule_ids: Vec<u8>,
    pub forward: bool,
}

/// `POST /api/v1/admin/accounts/:id/action` types and their reversals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminAction {
    /// A warning only: resolves the report, changes nothing.
    None,
    Sensitive,
    Disable,
    Silence,
    Suspend,
    Unsensitive,
    Enable,
    Unsilence,
    Unsuspend,
}

impl AdminAction {
    pub fn parse(text: &str) -> Option<AdminAction> {
        match text {
            "none" => Some(AdminAction::None),
            "sensitive" => Some(AdminAction::Sensitive),
            "disable" => Some(AdminAction::Disable),
            "silence" => Some(AdminAction::Silence),
            "suspend" => Some(AdminAction::Suspend),
            _ => None,
        }
    }
}

fn not_allowed<T>() -> Result<T> {
    Err(Error::Forbidden("This action is not allowed".into()))
}

impl<S: Store> Service<S> {
    // --- Blocks and mutes ---------------------------------------------------

    /// Block (or unblock). Blocking ends follows in both directions and
    /// drops the notifications the two exchanged.
    pub fn set_block(&mut self, src: u8, dst: u8, on: bool, now_ms: u64) -> Result<()> {
        if self.state.account(dst).is_none() {
            return Err(Error::NotFound);
        }
        if src == dst {
            return not_allowed();
        }
        let existing = self.state.blocks.contains_key(&(src, dst));
        if on == existing {
            return Ok(());
        }
        self.govern(Some(src), now_ms)?;
        if !on {
            self.erase(Ns::Rel, &keys::block(src, dst))?;
            self.state.blocks.remove(&(src, dst));
            return Ok(());
        }
        let rec = BlockRec {
            id: self.next_id(now_ms),
        };
        // The block is the commit point; boot finishes the unfollows.
        self.put(Ns::Rel, &keys::block(src, dst), Kind::Block, &rec)?;
        self.state.blocks.insert((src, dst), rec);
        for (a, b) in [(src, dst), (dst, src)] {
            if self.state.follows.contains_key(&(a, b)) {
                self.erase(Ns::Rel, &keys::follow(a, b))?;
                self.state.follows.remove(&(a, b));
                self.leave_lists(a, b)?;
            }
            self.drop_follow_request(a, b)?;
        }
        self.state
            .notifications
            .retain(|n| !((n.to == src && n.from == dst) || (n.to == dst && n.from == src)));
        Ok(())
    }

    /// Mute (or unmute). Muting again updates the options.
    /// `duration_s` of `None` or `0` mutes indefinitely.
    pub fn set_mute(
        &mut self,
        src: u8,
        dst: u8,
        on: bool,
        notifications: Option<bool>,
        duration_s: Option<u64>,
        now_ms: u64,
    ) -> Result<()> {
        if self.state.account(dst).is_none() {
            return Err(Error::NotFound);
        }
        if src == dst {
            return not_allowed();
        }
        let existing = self.state.mutes.get(&(src, dst)).copied();
        if !on {
            if existing.is_some() {
                self.govern(Some(src), now_ms)?;
                self.erase(Ns::Rel, &keys::mute(src, dst))?;
                self.state.mutes.remove(&(src, dst));
            }
            return Ok(());
        }
        let rec = MuteRec {
            id: existing.map_or_else(|| self.next_id(now_ms), |m| m.id),
            notifications: notifications.unwrap_or(true),
            expires_ms: duration_s
                .filter(|d| *d > 0)
                .map(|d| now_ms.saturating_add(d.saturating_mul(1000))),
        };
        if existing == Some(rec) {
            return Ok(());
        }
        self.govern(Some(src), now_ms)?;
        self.put(Ns::Rel, &keys::mute(src, dst), Kind::Mute, &rec)?;
        self.state.mutes.insert((src, dst), rec);
        Ok(())
    }

    /// Blocked or muted accounts, paged by record id.
    pub fn relation_list(&self, slot: u8, blocks: bool, q: &PageQuery) -> Vec<(u64, u8)> {
        let mut rows: Vec<(u64, u8)> = if blocks {
            self.state
                .blocks
                .iter()
                .filter(|((src, _), _)| *src == slot)
                .map(|((_, dst), rec)| (rec.id, *dst))
                .collect()
        } else {
            self.state
                .mutes
                .keys()
                .filter(|(src, dst)| *src == slot && self.state.mute(*src, *dst).is_some())
                .filter_map(|(src, dst)| self.state.mutes.get(&(*src, *dst)).map(|m| (m.id, *dst)))
                .collect()
        };
        rows.retain(|(_, s)| self.state.account(*s).is_some());
        rows.sort_unstable();
        query::paginate(q, |bounds, asc| {
            let rows = rows.clone();
            let it: query::Scan<'_, u8> = if asc {
                Box::new(rows.into_iter().filter(move |(id, _)| bounds.contains(*id)))
            } else {
                Box::new(
                    rows.into_iter()
                        .rev()
                        .filter(move |(id, _)| bounds.contains(*id)),
                )
            };
            it
        })
    }

    // --- Conversation mutes ---------------------------------------------------

    /// The status itself and its ancestors still in RAM, nearest first.
    fn thread_chain(&self, status_id: u64) -> Vec<u64> {
        let mut chain = Vec::new();
        let mut next = Some(status_id);
        while let Some(id) = next {
            let Some(s) = self.state.statuses.get(&id) else {
                break;
            };
            chain.push(id);
            if chain.len() >= MAX_THREAD_DEPTH {
                break;
            }
            next = s.rec.in_reply_to_id;
        }
        chain
    }

    /// Has `viewer` muted the conversation this status belongs to?
    pub fn thread_muted(&self, viewer: u8, status_id: u64) -> bool {
        self.thread_chain(status_id).iter().any(|id| {
            self.state
                .statuses
                .get(id)
                .is_some_and(|s| s.muted & bit(viewer) != 0)
        })
    }

    /// Mute a conversation: recorded on the oldest status of the thread that
    /// is still stored. Unmuting clears every mute along the chain.
    pub fn set_conversation_mute(
        &mut self,
        slot: u8,
        status_id: u64,
        on: bool,
        now_ms: u64,
    ) -> Result<()> {
        if self.visible(Some(slot), status_id).is_none() {
            return Err(Error::NotFound);
        }
        let chain = self.thread_chain(status_id);
        if on {
            if self.thread_muted(slot, status_id) {
                return Ok(());
            }
            let root = *chain.last().unwrap_or(&status_id);
            let target = if self.visible(Some(slot), root).is_some() {
                root
            } else {
                status_id
            };
            return self.set_reaction(slot, ReactionKind::Mute, target, true, now_ms);
        }
        for id in chain {
            let muted = self
                .state
                .statuses
                .get(&id)
                .is_some_and(|s| s.muted & bit(slot) != 0);
            if muted {
                self.set_reaction(slot, ReactionKind::Mute, id, false, now_ms)?;
            }
        }
        Ok(())
    }

    // --- Reports ------------------------------------------------------------

    pub fn file_report(&mut self, reporter: u8, new: NewReport, now_ms: u64) -> Result<u64> {
        self.writable()?;
        if self.state.account(new.target).is_none() {
            return Err(Error::NotFound);
        }
        if new.target == reporter {
            return invalid("Validation failed: You can't report yourself");
        }
        let comment = new.comment.trim().to_string();
        if comment.chars().count() > MAX_REPORT_COMMENT_CHARS
            || comment.len() > MAX_REPORT_COMMENT_BYTES
        {
            return invalid("Validation failed: Comment is too long (maximum is 500 characters)");
        }
        if new.status_ids.len() > MAX_REPORT_STATUSES {
            return invalid("Validation failed: At most 10 posts per report");
        }
        for id in &new.status_ids {
            let ok = self
                .visible(Some(reporter), *id)
                .is_some_and(|s| s.rec.author == new.target);
            if !ok {
                return Err(Error::NotFound);
            }
        }
        let rules = self.state.server.rules.len();
        if new.rule_ids.iter().any(|r| *r == 0 || *r as usize > rules) {
            return invalid("Validation failed: Rules are invalid");
        }
        let category = new.category.unwrap_or(if new.rule_ids.is_empty() {
            ReportCategory::Other
        } else {
            ReportCategory::Violation
        });
        self.govern(Some(reporter), now_ms)?;
        if self.state.reports.len() >= MAX_REPORTS {
            let oldest_resolved = self
                .state
                .reports
                .values()
                .find(|r| r.action_taken_ms.is_some())
                .map(|r| r.id)
                .ok_or_else(|| {
                    Error::Invalid(
                        "Too many open reports; ask a household admin to resolve some".into(),
                    )
                })?;
            self.erase(Ns::Mod, &keys::report(oldest_resolved))?;
            self.state.reports.remove(&oldest_resolved);
        }
        let mut status_ids = new.status_ids;
        status_ids.dedup();
        let rec = ReportRec {
            id: self.next_id(now_ms),
            reporter,
            target: new.target,
            status_ids,
            comment,
            category,
            rule_ids: new.rule_ids,
            forward: new.forward,
            assigned: None,
            action_taken_ms: None,
            action_taken_by: None,
            updated_ms: now_ms,
        };
        let id = rec.id;
        self.put(Ns::Mod, &keys::report(id), Kind::Report, &rec)?;
        self.state.reports.insert(id, rec);
        let admins: Vec<u8> = self
            .state
            .active_accounts()
            .filter(|a| a.rec.role.is_admin() && !a.rec.disabled)
            .map(|a| a.slot)
            .collect();
        for admin in admins {
            self.notify_about(
                NotificationKind::AdminReport,
                admin,
                reporter,
                None,
                Some(id),
                now_ms,
            );
        }
        Ok(id)
    }

    /// Only admins moderate.
    pub fn require_admin(&self, actor: u8) -> Result<()> {
        match self.state.account(actor) {
            Some(a) if a.rec.role.is_admin() => Ok(()),
            _ => not_allowed(),
        }
    }

    /// May `actor` moderate `target`? Nobody moderates themselves or the
    /// owner, and only the owner moderates admins.
    pub(crate) fn require_power_over(&self, actor: u8, target: u8) -> Result<()> {
        self.require_admin(actor)?;
        let actor_role = self.state.account(actor).map(|a| a.rec.role);
        let target_role = self.state.account(target).ok_or(Error::NotFound)?.rec.role;
        if actor == target
            || target_role == Role::Owner
            || (target_role == Role::Admin && actor_role != Some(Role::Owner))
        {
            return not_allowed();
        }
        Ok(())
    }

    fn write_report(&mut self, rec: ReportRec) -> Result<()> {
        if self.state.reports.get(&rec.id) == Some(&rec) {
            return Ok(());
        }
        self.put(Ns::Mod, &keys::report(rec.id), Kind::Report, &rec)?;
        self.state.reports.insert(rec.id, rec);
        Ok(())
    }

    fn report_for_update(&self, actor: u8, id: u64) -> Result<ReportRec> {
        self.require_admin(actor)?;
        self.state.reports.get(&id).cloned().ok_or(Error::NotFound)
    }

    /// Resolve (`true`) or reopen (`false`) a report.
    pub fn resolve_report(
        &mut self,
        actor: u8,
        id: u64,
        resolved: bool,
        now_ms: u64,
    ) -> Result<()> {
        let mut rec = self.report_for_update(actor, id)?;
        if rec.action_taken_ms.is_some() == resolved {
            return Ok(());
        }
        rec.action_taken_ms = resolved.then_some(now_ms);
        rec.action_taken_by = resolved.then_some(actor);
        rec.updated_ms = now_ms;
        self.govern(Some(actor), now_ms)?;
        self.write_report(rec)
    }

    pub fn assign_report(&mut self, actor: u8, id: u64, assign: bool, now_ms: u64) -> Result<()> {
        let mut rec = self.report_for_update(actor, id)?;
        let assigned = assign.then_some(actor);
        if rec.assigned == assigned {
            return Ok(());
        }
        rec.assigned = assigned;
        rec.updated_ms = now_ms;
        self.govern(Some(actor), now_ms)?;
        self.write_report(rec)
    }

    pub fn recategorize_report(
        &mut self,
        actor: u8,
        id: u64,
        category: Option<ReportCategory>,
        rule_ids: Option<Vec<u8>>,
        now_ms: u64,
    ) -> Result<()> {
        let mut rec = self.report_for_update(actor, id)?;
        let before = rec.clone();
        if let Some(rules) = rule_ids {
            let count = self.state.server.rules.len();
            if rules.iter().any(|r| *r == 0 || *r as usize > count) {
                return invalid("Validation failed: Rules are invalid");
            }
            rec.rule_ids = rules;
        }
        if let Some(category) = category {
            rec.category = category;
        }
        if rec == before {
            return Ok(());
        }
        rec.updated_ms = now_ms;
        self.govern(Some(actor), now_ms)?;
        self.write_report(rec)
    }

    // --- Account moderation -------------------------------------------------

    fn write_moderation(&mut self, slot: u8, rec: AccountModRec) -> Result<()> {
        if self.state.moderation(slot) == rec {
            return Ok(());
        }
        if rec.is_clear() {
            self.erase(Ns::Mod, &keys::account_mod(slot))?;
            self.state.moderation[slot as usize] = None;
        } else {
            self.put(Ns::Mod, &keys::account_mod(slot), Kind::AccountMod, &rec)?;
            self.state.moderation[slot as usize] = Some(rec);
        }
        Ok(())
    }

    fn write_disabled(&mut self, slot: u8, disabled: bool) -> Result<()> {
        let mut rec = self.state.account(slot).ok_or(Error::NotFound)?.rec.clone();
        if rec.disabled == disabled {
            return Ok(());
        }
        rec.disabled = disabled;
        self.put(Ns::Acct, &keys::account(slot), Kind::Account, &rec)?;
        if let Some(a) = self.state.accounts[slot as usize].as_mut() {
            a.rec = rec;
        }
        Ok(())
    }

    /// Take (or undo) a moderation action. A `report_id` is resolved by it.
    /// Disabled and suspended accounts can't sign in and their tokens stop
    /// working; undoing the action brings their devices back.
    pub fn moderate(
        &mut self,
        actor: u8,
        target: u8,
        action: AdminAction,
        report_id: Option<u64>,
        now_ms: u64,
    ) -> Result<()> {
        self.require_power_over(actor, target)?;
        if let Some(id) = report_id {
            if self
                .state
                .reports
                .get(&id)
                .is_none_or(|r| r.target != target)
            {
                return Err(Error::NotFound);
            }
        }
        let mut m = self.state.moderation(target);
        m.account_id = self.state.account(target).ok_or(Error::NotFound)?.rec.id;
        self.govern(Some(actor), now_ms)?;
        match action {
            AdminAction::None => {}
            AdminAction::Disable => self.write_disabled(target, true)?,
            AdminAction::Enable => self.write_disabled(target, false)?,
            AdminAction::Sensitive | AdminAction::Unsensitive => {
                m.sensitized = action == AdminAction::Sensitive;
                self.write_moderation(target, m)?;
            }
            AdminAction::Silence | AdminAction::Unsilence => {
                m.silenced = action == AdminAction::Silence;
                self.write_moderation(target, m)?;
            }
            AdminAction::Suspend => {
                if m.suspended_ms.is_none() {
                    m.suspended_ms = Some(now_ms);
                    self.write_moderation(target, m)?;
                }
            }
            AdminAction::Unsuspend => {
                m.suspended_ms = None;
                self.write_moderation(target, m)?;
            }
        }
        if let Some(id) = report_id {
            if let Some(mut rec) = self.state.reports.get(&id).cloned() {
                if rec.action_taken_ms.is_none() {
                    rec.action_taken_ms = Some(now_ms);
                    rec.action_taken_by = Some(actor);
                    rec.updated_ms = now_ms;
                    self.write_report(rec)?;
                }
            }
        }
        Ok(())
    }

    /// The owner makes members admins, or admins members again.
    pub fn set_role(&mut self, actor: u8, target: u8, role: Role, now_ms: u64) -> Result<()> {
        if self.state.account(actor).map(|a| a.rec.role) != Some(Role::Owner) {
            return Err(Error::Forbidden("Only the owner can change roles".into()));
        }
        if role == Role::Owner {
            return Err(Error::Forbidden("There is only one owner".into()));
        }
        let mut rec = self
            .state
            .account(target)
            .ok_or(Error::NotFound)?
            .rec
            .clone();
        if rec.role == Role::Owner {
            return not_allowed();
        }
        if rec.role == role {
            return Ok(());
        }
        rec.role = role;
        self.govern(Some(actor), now_ms)?;
        self.put(Ns::Acct, &keys::account(target), Kind::Account, &rec)?;
        if let Some(a) = self.state.accounts[target as usize].as_mut() {
            a.rec = rec;
        }
        Ok(())
    }

    /// An admin deletes someone's post. Never a direct message the admin
    /// isn't part of: to them it doesn't exist (spec/open-questions #9).
    pub fn admin_delete_status(&mut self, actor: u8, id: u64, now_ms: u64) -> Result<Status> {
        self.require_admin(actor)?;
        let status = self.state.statuses.get(&id).ok_or(Error::NotFound)?;
        let author = status.rec.author;
        let party = author == actor || status.mentions & bit(actor) != 0;
        if status.rec.visibility == Visibility::Direct && !party {
            return Err(Error::NotFound);
        }
        if author != actor {
            self.require_power_over(actor, author)?;
        }
        let copy = self
            .state
            .statuses
            .get(&id)
            .cloned()
            .ok_or(Error::NotFound)?;
        self.govern(Some(actor), now_ms)?;
        self.remove_status(id)?;
        Ok(copy)
    }

    /// Delete an account and everything it made. The tombstone is the
    /// commit point; boot finishes the purge if power is lost (spec/02).
    /// Through the Mastodon admin API the account must be suspended first.
    pub fn delete_account(&mut self, actor: u8, target: u8, now_ms: u64) -> Result<()> {
        self.require_power_over(actor, target)?;
        self.govern(Some(actor), now_ms)?;
        let mut rec = self
            .state
            .account(target)
            .ok_or(Error::NotFound)?
            .rec
            .clone();
        rec.deleted = true;
        self.put(Ns::Acct, &keys::account(target), Kind::Account, &rec)?;
        let account_id = rec.id;
        if let Some(a) = self.state.accounts[target as usize].as_mut() {
            a.rec = rec;
        }
        self.purge_account(target, account_id)
    }

    fn purge_account(&mut self, slot: u8, account_id: u64) -> Result<()> {
        let statuses: Vec<u64> = self
            .state
            .statuses
            .values()
            .filter(|s| s.rec.author == slot)
            .map(|s| s.rec.id)
            .collect();
        for id in statuses {
            self.remove_status(id)?;
        }
        let boosts: Vec<u64> = self
            .state
            .boosts
            .values()
            .filter(|b| b.booster == slot)
            .map(|b| b.id)
            .collect();
        for id in boosts {
            self.remove_boost(id)?;
        }
        let reactions: Vec<(u64, Reaction)> = self
            .state
            .reactions
            .iter()
            .filter(|(_, r)| r.slot == slot)
            .map(|(id, r)| (*id, *r))
            .collect();
        for (rid, r) in reactions {
            self.erase(Ns::Rx, &keys::reaction(r.kind, r.status, r.slot))?;
            self.state.reactions.remove(&rid);
            self.state.reaction_ids.remove(&(r.kind, r.status, r.slot));
            if let Some(s) = self.state.statuses.get_mut(&r.status) {
                let mask = match r.kind {
                    ReactionKind::Favourite => &mut s.favourited,
                    ReactionKind::Bookmark => &mut s.bookmarked,
                    ReactionKind::Pin => &mut s.pinned,
                    ReactionKind::Mute => &mut s.muted,
                };
                *mask &= !bit(slot);
            }
        }
        let follows: Vec<(u8, u8)> = self
            .state
            .follows
            .keys()
            .filter(|(a, b)| *a == slot || *b == slot)
            .copied()
            .collect();
        for (a, b) in follows {
            self.erase(Ns::Rel, &keys::follow(a, b))?;
            self.state.follows.remove(&(a, b));
        }
        let blocks: Vec<(u8, u8)> = self
            .state
            .blocks
            .keys()
            .filter(|(a, b)| *a == slot || *b == slot)
            .copied()
            .collect();
        for (a, b) in blocks {
            self.erase(Ns::Rel, &keys::block(a, b))?;
            self.state.blocks.remove(&(a, b));
        }
        let mutes: Vec<(u8, u8)> = self
            .state
            .mutes
            .keys()
            .filter(|(a, b)| *a == slot || *b == slot)
            .copied()
            .collect();
        for (a, b) in mutes {
            self.erase(Ns::Rel, &keys::mute(a, b))?;
            self.state.mutes.remove(&(a, b));
        }
        if self.state.moderation[slot as usize].take().is_some() {
            self.erase(Ns::Mod, &keys::account_mod(slot))?;
        }
        let reports: Vec<u64> = self
            .state
            .reports
            .values()
            .filter(|r| r.reporter == slot || r.target == slot)
            .map(|r| r.id)
            .collect();
        for id in reports {
            self.erase(Ns::Mod, &keys::report(id))?;
            self.state.reports.remove(&id);
        }
        let reports: Vec<ReportRec> = self
            .state
            .reports
            .values()
            .filter(|r| r.assigned == Some(slot) || r.action_taken_by == Some(slot))
            .cloned()
            .collect();
        for mut rec in reports {
            if rec.assigned == Some(slot) {
                rec.assigned = None;
            }
            if rec.action_taken_by == Some(slot) {
                rec.action_taken_by = None;
            }
            self.write_report(rec)?;
        }
        self.purge_collections(slot, account_id)?;
        let tokens: Vec<[u8; 32]> = self
            .state
            .tokens
            .iter()
            .filter(|t| t.rec.slot == slot)
            .map(|t| t.rec.hash)
            .collect();
        for hash in tokens {
            self.forget_token(&hash)?;
        }
        if self.state.user_keys[slot as usize].take().is_some() {
            self.erase(Ns::Key, &keys::user_key(slot))?;
        }
        self.forget_votes(slot)?;
        let requests: Vec<(u8, u8)> = self
            .state
            .follow_requests
            .keys()
            .filter(|(a, b)| *a == slot || *b == slot)
            .copied()
            .collect();
        for (a, b) in requests {
            self.erase(Ns::Rel, &keys::follow_request(a, b))?;
            self.state.follow_requests.remove(&(a, b));
        }
        let lists: Vec<(u8, u8)> = self
            .state
            .lists
            .range((slot, 0)..=(slot, 15))
            .map(|(k, _)| *k)
            .collect();
        for (s, n) in lists {
            self.erase(Ns::List, &keys::list(s, n))?;
            self.state.lists.remove(&(s, n));
        }
        let filters: Vec<(u8, u8)> = self
            .state
            .filters
            .range((slot, 0)..=(slot, 15))
            .map(|(k, _)| *k)
            .collect();
        for (s, n) in filters {
            self.erase(Ns::Filt, &keys::filter(s, n))?;
            self.state.filters.remove(&(s, n));
        }
        self.state.conversations.retain(|(s, _), _| *s != slot);
        let codes: Vec<u64> = self
            .state
            .invites
            .values()
            .filter(|c| matches!(c.kind, CodeKind::Reset { slot: s, .. } if s == slot))
            .map(|c| c.id)
            .collect();
        for id in codes {
            self.erase(Ns::Inv, &keys::code(id))?;
            self.state.invites.remove(&id);
        }
        self.state
            .notifications
            .retain(|n| n.to != slot && n.from != slot);
        self.state.markers[slot as usize] = [None, None];
        self.erase(Ns::Acct, &keys::account(slot))?;
        self.state.accounts[slot as usize] = None;
        Ok(())
    }
}
