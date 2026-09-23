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

    fn contains(self, id: u64) -> bool {
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
    pub fn can_see(&self, viewer: Option<u8>, status: &Status) -> bool {
        let author = status.rec.author;
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
        if !self.entry_visible(viewer, entry) {
            return false;
        }
        match entry {
            Entry::Status(id) => {
                let Some(s) = self.state.statuses.get(&id) else {
                    return false;
                };
                let author = s.rec.author;
                if author != viewer && !self.state.follows(viewer, author) {
                    return s.rec.visibility == Visibility::Direct && s.mentions & bit(viewer) != 0;
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
        paginate(q, |bounds, asc| {
            Box::new(
                self.entries(bounds, asc)
                    .filter(move |(_, e)| self.in_home(viewer, *e)),
            )
        })
    }

    /// Public (local) timeline: public posts only, no boosts.
    pub fn public(&self, q: &PageQuery) -> Vec<(u64, Entry)> {
        self.status_page(q, |s| s.rec.visibility == Visibility::Public)
    }

    pub fn tag_timeline(&self, tag: &str, q: &PageQuery) -> Vec<(u64, Entry)> {
        let tag = tag.to_lowercase();
        self.status_page(q, |s| {
            s.rec.visibility == Visibility::Public && s.tags.contains(&tag)
        })
    }

    fn status_page(
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
        let mut ancestors = Vec::new();
        let mut parent = status.rec.in_reply_to_id;
        while let Some(p) = parent {
            let Some(s) = self.visible(viewer, p) else {
                break;
            };
            ancestors.push(p);
            if ancestors.len() >= MAX_ANCESTORS {
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
                descendants.push(*sid);
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
            .filter(|s| self.can_see(Some(viewer), s) && s.rec.text.to_lowercase().contains(&q))
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
        self.state
            .statuses
            .values()
            .filter(|s| s.rec.author == slot && s.rec.visibility != Visibility::Direct)
            .count()
    }

    pub fn last_status_ms(&self, slot: u8) -> Option<u64> {
        self.state
            .statuses
            .values()
            .rev()
            .find(|s| s.rec.author == slot)
            .map(|s| ids::millis(s.rec.id))
    }

    pub fn follower_counts(&self, slot: u8) -> (usize, usize) {
        let followers = self
            .state
            .follows
            .keys()
            .filter(|(_, d)| *d == slot)
            .count();
        let following = self
            .state
            .follows
            .keys()
            .filter(|(s, _)| *s == slot)
            .count();
        (followers, following)
    }
}
