//! Read-side queries: visibility, timelines, threads, search and pagination.
//!
//! Timelines are plain reverse-chronological filters over RAM, nothing is
//! ranked or precomputed, and no read ever writes (spec/02 "No feed
//! algorithms").

use super::*;
use std::iter::Peekable;

pub const DEFAULT_LIMIT: usize = 20;
pub const MAX_LIMIT: usize = 40;
const MAX_ANCESTORS: usize = 40;
const MAX_DESCENDANTS: usize = 60;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct AccountStats {
    statuses: usize,
    last_ms: Option<u64>,
    followers: usize,
    following: usize,
}

/// Mastodon cursor parameters.
#[derive(Debug, Clone, Copy, Default)]
pub struct PageQuery {
    pub max_id: Option<u64>,
    pub since_id: Option<u64>,
    pub min_id: Option<u64>,
    pub limit: usize,
}

impl PageQuery {
    pub fn limit(&self) -> usize {
        if self.limit == 0 {
            DEFAULT_LIMIT
        } else {
            self.limit.min(MAX_LIMIT)
        }
    }
}

/// Exclusive id bounds for a range scan.
#[derive(Debug, Clone, Copy, Default)]
pub struct Bounds {
    pub lower: Option<u64>,
    pub upper: Option<u64>,
}

impl Bounds {
    fn range(self) -> (std::ops::Bound<u64>, std::ops::Bound<u64>) {
        use std::ops::Bound::*;
        (
            self.lower.map_or(Unbounded, Excluded),
            self.upper.map_or(Unbounded, Excluded),
        )
    }

    pub(crate) fn contains(self, id: u64) -> bool {
        self.lower.is_none_or(|l| id > l) && self.upper.is_none_or(|u| id < u)
    }
}

pub type Scan<'a, T> = Box<dyn Iterator<Item = (u64, T)> + 'a>;

/// Apply cursor semantics to a source that can scan a bounded id range in
/// either direction. Results are newest first. `min_id` returns the page
/// immediately above the cursor; `since_id` the newest page above it.
pub fn paginate<'a, T>(
    q: &PageQuery,
    source: impl Fn(Bounds, bool) -> Scan<'a, T>,
) -> Vec<(u64, T)> {
    let limit = q.limit();
    match q.min_id {
        Some(min) => {
            let bounds = Bounds {
                lower: Some(q.since_id.map_or(min, |s| s.max(min))),
                upper: q.max_id,
            };
            let mut items: Vec<(u64, T)> = source(bounds, true).take(limit).collect();
            items.reverse();
            items
        }
        None => source(
            Bounds {
                lower: q.since_id,
                upper: q.max_id,
            },
            false,
        )
        .take(limit)
        .collect(),
    }
}

/// A timeline row: an original status, or a boost of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Status(u64),
    Boost { id: u64, target: u64 },
}

impl Entry {
    pub fn status_id(self) -> u64 {
        match self {
            Entry::Status(id) => id,
            Entry::Boost { target, .. } => target,
        }
    }
}

/// Merge two id-ordered scans into one.
struct Merge<'a> {
    a: Peekable<Scan<'a, Entry>>,
    b: Peekable<Scan<'a, Entry>>,
    ascending: bool,
}

impl Iterator for Merge<'_> {
    type Item = (u64, Entry);
    fn next(&mut self) -> Option<Self::Item> {
        let take_a = match (self.a.peek(), self.b.peek()) {
            (Some(x), Some(y)) => (x.0 < y.0) == self.ascending,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => return None,
        };
        if take_a {
            self.a.next()
        } else {
            self.b.next()
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct AccountStatusesQuery {
    pub exclude_replies: bool,
    pub exclude_reblogs: bool,
    pub pinned: bool,
    pub only_media: bool,
    pub tagged: Option<String>,
}

impl<S: Store> Service<S> {
    /// Who may see a status (spec/04 "Visibility inside a household").
    /// Nobody sees a suspended account's posts, and an author's block hides
    /// their posts from the blocked account.
    pub fn can_see(&self, viewer: Option<u8>, status: &Status) -> bool {
        let author = status.rec.author;
        if self.state.suspended(author) {
            return false;
        }
        if viewer.is_some_and(|v| v != author && self.state.blocks(author, v)) {
            return false;
        }
        match (viewer, status.rec.visibility) {
            (_, Visibility::Public | Visibility::Unlisted) => true,
            (None, _) => false,
            (Some(v), _) if v == author => true,
            (Some(v), Visibility::Private) => {
                self.state.follows(v, author) || status.mentions & bit(v) != 0
            }
            (Some(v), Visibility::Direct) => status.mentions & bit(v) != 0,
        }
    }

    pub fn visible(&self, viewer: Option<u8>, id: u64) -> Option<&Status> {
        self.state
            .statuses
            .get(&id)
            .filter(|s| self.can_see(viewer, s))
    }

    /// Statuses and boosts merged in id order.
    pub fn entries(&self, bounds: Bounds, ascending: bool) -> Scan<'_, Entry> {
        let statuses = self
            .state
            .statuses
            .range(bounds.range())
            .map(|(id, _)| (*id, Entry::Status(*id)));
        let boosts = self.state.boosts.range(bounds.range()).map(|(id, b)| {
            (
                *id,
                Entry::Boost {
                    id: *id,
                    target: b.target,
                },
            )
        });
        let (a, b): (Scan<'_, Entry>, Scan<'_, Entry>) = if ascending {
            (Box::new(statuses), Box::new(boosts))
        } else {
            (Box::new(statuses.rev()), Box::new(boosts.rev()))
        };
        Box::new(Merge {
            a: a.peekable(),
            b: b.peekable(),
            ascending,
        })
    }

    fn entry_visible(&self, viewer: u8, entry: Entry) -> bool {
        self.visible(Some(viewer), entry.status_id()).is_some()
    }

    /// Mastodon's home rule: your posts and those of people you follow;
    /// replies only when you also follow (or are) the person replied to.
    pub fn in_home(&self, viewer: u8, entry: Entry) -> bool {
        self.in_home_masked(viewer, entry, self.state.hidden_mask(viewer))
    }

    /// Timeline filter for blocks and mutes: the author, the booster, or
    /// anyone mentioned is in `hidden` (Mastodon's feed filter).
    fn hidden_entry(&self, entry: Entry, hidden: u16) -> bool {
        if hidden == 0 {
            return false;
        }
        let Some(s) = self.state.statuses.get(&entry.status_id()) else {
            return true;
        };
        let booster = match entry {
            Entry::Boost { id, .. } => self.state.boosts.get(&id).map_or(0, |b| bit(b.booster)),
            Entry::Status(_) => 0,
        };
        (bit(s.rec.author) | booster | s.mentions) & hidden != 0
    }

    fn in_home_masked(&self, viewer: u8, entry: Entry, hidden: u16) -> bool {
        if !self.entry_visible(viewer, entry) {
            return false;
        }
        // Your own posts may mention someone you muted; keep them.
        let own = match entry {
            Entry::Status(id) => self
                .state
                .statuses
                .get(&id)
                .is_some_and(|s| s.rec.author == viewer),
            Entry::Boost { .. } => false,
        };
        if !own && self.hidden_entry(entry, hidden & !bit(viewer)) {
            return false;
        }
        match entry {
            Entry::Status(id) => {
                let Some(s) = self.state.statuses.get(&id) else {
                    return false;
                };
                let author = s.rec.author;
                if author != viewer && !self.state.follows(viewer, author) {
                    return (s.rec.visibility == Visibility::Direct
                        && s.mentions & bit(viewer) != 0)
                        || (self.follows_tag(viewer, &s.tags)
                            && self.on_public(viewer, s, hidden));
                }
                match s
                    .rec
                    .in_reply_to_account_id
                    .and_then(|a| self.state.slot_of(a))
                {
                    Some(replied) if replied != author && replied != viewer => {
                        author == viewer || self.state.follows(viewer, replied)
                    }
                    _ => true,
                }
            }
            Entry::Boost { id, .. } => {
                let Some(boost) = self.state.boosts.get(&id) else {
                    return false;
                };
                boost.booster == viewer
                    || self
                        .state
                        .follows
                        .get(&(viewer, boost.booster))
                        .is_some_and(|f| f.reblogs)
            }
        }
    }

    pub fn home(&self, viewer: u8, q: &PageQuery) -> Vec<(u64, Entry)> {
        let hidden = self.state.hidden_mask(viewer);
        // Members of exclusive lists appear only in those lists.
        let exclusive = self.exclusive_mask(viewer) & !bit(viewer);
        let off_home = move |e: Entry| -> bool {
            let who = match e {
                Entry::Status(id) => self.state.statuses.get(&id).map(|s| s.rec.author),
                Entry::Boost { id, .. } => self.state.boosts.get(&id).map(|b| b.booster),
            };
            who.is_some_and(|s| bit(s) & exclusive != 0)
        };
        paginate(q, |bounds, asc| {
            Box::new(
                self.entries(bounds, asc)
                    .filter(move |(_, e)| self.in_home_masked(viewer, *e, hidden) && !off_home(*e)),
            )
        })
    }

    /// Public and tag timelines leave out the viewer's blocks and mutes,
    /// and silenced ("limited") accounts the viewer doesn't follow.
    fn on_public(&self, viewer: u8, s: &Status, hidden: u16) -> bool {
        let author = s.rec.author;
        s.rec.visibility == Visibility::Public
            && self.can_see(Some(viewer), s)
            && (author == viewer || s.mentions & hidden & !bit(viewer) == 0)
            && bit(author) & hidden == 0
            && (author == viewer
                || !self.state.silenced(author)
                || self.state.follows(viewer, author))
    }

    /// Public (local) timeline: public posts only, no boosts.
    pub fn public(&self, viewer: u8, q: &PageQuery) -> Vec<(u64, Entry)> {
        let hidden = self.state.hidden_mask(viewer);
        self.status_page(q, move |s| self.on_public(viewer, s, hidden))
    }

    pub fn tag_timeline(&self, viewer: u8, tag: &str, q: &PageQuery) -> Vec<(u64, Entry)> {
        let tag = tag.to_lowercase();
        let hidden = self.state.hidden_mask(viewer);
        let tag = &tag;
        self.status_page(q, move |s| {
            s.tags.contains(tag) && self.on_public(viewer, s, hidden)
        })
    }

    pub(crate) fn status_page(
        &self,
        q: &PageQuery,
        keep: impl Fn(&Status) -> bool + Copy,
    ) -> Vec<(u64, Entry)> {
        paginate(q, |bounds, asc| {
            let range = self.state.statuses.range(bounds.range());
            let it: Scan<'_, Entry> = if asc {
                Box::new(
                    range
                        .filter(move |(_, s)| keep(s))
                        .map(|(id, _)| (*id, Entry::Status(*id))),
                )
            } else {
                Box::new(
                    range
                        .rev()
                        .filter(move |(_, s)| keep(s))
                        .map(|(id, _)| (*id, Entry::Status(*id))),
                )
            };
            it
        })
    }

    pub fn account_statuses(
        &self,
        viewer: Option<u8>,
        author: u8,
        opts: &AccountStatusesQuery,
        q: &PageQuery,
    ) -> Vec<(u64, Entry)> {
        if opts.only_media {
            return Vec::new();
        }
        let tagged = opts.tagged.as_ref().map(|t| t.to_lowercase());
        let keep = move |entry: Entry| -> bool {
            match entry {
                Entry::Status(id) => {
                    let Some(s) = self.visible(viewer, id) else {
                        return false;
                    };
                    s.rec.author == author
                        && (!opts.pinned || s.pinned & bit(author) != 0)
                        && !(opts.exclude_replies
                            && s.rec.in_reply_to_id.is_some()
                            && s.rec.in_reply_to_account_id
                                != self.state.account(author).map(|a| a.rec.id))
                        && tagged.as_ref().is_none_or(|t| s.tags.contains(t))
                }
                Entry::Boost { id, target } => {
                    !opts.exclude_reblogs
                        && !opts.pinned
                        && tagged.is_none()
                        && self
                            .state
                            .boosts
                            .get(&id)
                            .is_some_and(|b| b.booster == author)
                        && self.visible(viewer, target).is_some()
                }
            }
        };
        paginate(q, |bounds, asc| {
            let keep = keep.clone();
            Box::new(self.entries(bounds, asc).filter(move |(_, e)| keep(*e)))
        })
    }

    /// Favourites / bookmarks, paged by reaction record id.
    pub fn reactions_of(&self, viewer: u8, kind: ReactionKind, q: &PageQuery) -> Vec<(u64, u64)> {
        paginate(q, |bounds, asc| {
            let range = self.state.reactions.range(bounds.range());
            let keep = move |(_, r): &(&u64, &Reaction)| {
                r.kind == kind && r.slot == viewer && self.visible(Some(viewer), r.status).is_some()
            };
            let it: Scan<'_, u64> = if asc {
                Box::new(range.filter(keep).map(|(id, r)| (*id, r.status)))
            } else {
                Box::new(range.rev().filter(keep).map(|(id, r)| (*id, r.status)))
            };
            it
        })
    }

    /// Accounts that reacted (or boosted), in slot order.
    pub fn reactors(&self, mask: u16) -> Vec<u8> {
        (0..MAX_ACCOUNTS as u8)
            .filter(|s| mask & bit(*s) != 0 && self.state.account(*s).is_some())
            .collect()
    }

    pub fn notifications(
        &self,
        viewer: u8,
        include: &[String],
        exclude: &[String],
        from: Option<u8>,
        q: &PageQuery,
    ) -> Vec<(u64, Notification)> {
        let keep = move |n: &&Notification| {
            n.to == viewer
                && (include.is_empty() || include.iter().any(|t| t == n.kind.as_str()))
                && !exclude.iter().any(|t| t == n.kind.as_str())
                && from.is_none_or(|f| n.from == f)
                && self.state.account(n.from).is_some()
                && (n.kind == NotificationKind::AdminReport
                    || (n.kind == NotificationKind::Poll && n.from == viewer)
                    || self.wants_notification(viewer, n.from, n.status))
                && n.status
                    .is_none_or(|s| self.visible(Some(viewer), s).is_some())
        };
        paginate(q, |bounds, asc| {
            let items = self.state.notifications.iter();
            let it: Scan<'_, Notification> = if asc {
                Box::new(
                    items
                        .filter(keep)
                        .filter(move |n| bounds.contains(n.id))
                        .map(|n| (n.id, *n)),
                )
            } else {
                Box::new(
                    items
                        .rev()
                        .filter(keep)
                        .filter(move |n| bounds.contains(n.id))
                        .map(|n| (n.id, *n)),
                )
            };
            it
        })
    }

    /// Thread around a status: visible ancestors (oldest first) and
    /// descendants in reply order.
    pub fn context(&self, viewer: Option<u8>, id: u64) -> Option<(Vec<u64>, Vec<u64>)> {
        let status = self.visible(viewer, id)?;
        let hides = |s: &Status| viewer.is_some_and(|v| self.state.hides(v, s.rec.author));
        let mut ancestors = Vec::new();
        let mut parent = status.rec.in_reply_to_id;
        let mut depth = 0;
        while let Some(p) = parent {
            let Some(s) = self.visible(viewer, p) else {
                break;
            };
            if !hides(s) {
                ancestors.push(p);
            }
            depth += 1;
            if depth >= MAX_ANCESTORS {
                break;
            }
            parent = s.rec.in_reply_to_id;
        }
        ancestors.reverse();
        let mut thread = vec![id];
        let mut descendants = Vec::new();
        for (sid, s) in self.state.statuses.range(id + 1..) {
            if descendants.len() >= MAX_DESCENDANTS {
                break;
            }
            if s.rec.in_reply_to_id.is_some_and(|p| thread.contains(&p)) && self.can_see(viewer, s)
            {
                thread.push(*sid);
                if !hides(s) {
                    descendants.push(*sid);
                }
            }
        }
        Some((ancestors, descendants))
    }

    pub fn search_statuses(&self, viewer: u8, query: &str, limit: usize) -> Vec<u64> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        self.state
            .statuses
            .values()
            .rev()
            .filter(|s| {
                self.can_see(Some(viewer), s)
                    && !self.state.hides(viewer, s.rec.author)
                    && s.rec.text.to_lowercase().contains(&q)
            })
            .map(|s| s.rec.id)
            .take(limit)
            .collect()
    }

    pub fn search_tags(&self, query: &str, limit: usize) -> Vec<String> {
        let q = query.trim().trim_start_matches('#').to_lowercase();
        let mut out: Vec<String> = Vec::new();
        for s in self.state.statuses.values().rev() {
            for tag in &s.tags {
                if tag.contains(&q) && !out.contains(tag) {
                    out.push(tag.clone());
                    if out.len() >= limit {
                        return out;
                    }
                }
            }
        }
        out
    }

    /// Accounts following / followed by `slot`, paged by follow record id.
    pub fn follow_list(&self, slot: u8, followers: bool, q: &PageQuery) -> Vec<(u64, u8)> {
        let mut rows: Vec<(u64, u8)> = self
            .state
            .follows
            .iter()
            .filter_map(|((src, dst), rec)| match followers {
                true if *dst == slot => Some((rec.id, *src)),
                false if *src == slot => Some((rec.id, *dst)),
                _ => None,
            })
            .collect();
        rows.sort_unstable();
        paginate(q, |bounds, asc| {
            let rows = rows.clone();
            let it: Scan<'_, u8> = if asc {
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

    pub fn statuses_count(&self, slot: u8) -> usize {
        self.account_stats(slot).statuses
    }

    pub fn last_status_ms(&self, slot: u8) -> Option<u64> {
        self.account_stats(slot).last_ms
    }

    pub fn follower_counts(&self, slot: u8) -> (usize, usize) {
        let stats = self.account_stats(slot);
        (stats.followers, stats.following)
    }

    fn account_stats(&self, slot: u8) -> AccountStats {
        let mut cache = self.state.account_stats.borrow_mut();
        let stats = cache.get_or_insert_with(|| {
            let mut stats = [AccountStats::default(); MAX_ACCOUNTS];
            for s in self.state.statuses.values() {
                let a = &mut stats[s.rec.author as usize];
                a.statuses += usize::from(s.rec.visibility != Visibility::Direct);
                a.last_ms = Some(ids::millis(s.rec.id));
            }
            for (src, dst) in self.state.follows.keys() {
                stats[*src as usize].following += 1;
                stats[*dst as usize].followers += 1;
            }
            stats
        });
        stats.get(slot as usize).copied().unwrap_or_default()
    }
}
