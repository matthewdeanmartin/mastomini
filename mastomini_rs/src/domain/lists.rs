//! Lists (spec/04 "Lists"): up to 8 per member, each a named set of people
//! the member follows, with its own timeline. An exclusive list keeps its
//! members' posts off home.

use super::query::{paginate, Entry, PageQuery};
use super::*;

pub const MAX_LIST_TITLE: usize = 40;

#[derive(Debug, Clone, Default)]
pub struct ListUpdate {
    pub title: Option<String>,
    pub replies_policy: Option<RepliesPolicy>,
    pub exclusive: Option<bool>,
}

fn check_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() || title.len() > MAX_LIST_TITLE {
        return invalid("Validation failed: Title must be 1-40 bytes");
    }
    Ok(title.to_string())
}

impl<S: Store> Service<S> {
    pub fn lists_of(&self, slot: u8) -> impl Iterator<Item = &ListRec> {
        self.state
            .lists
            .range((slot, 0)..(slot, MAX_LISTS_PER_ACCOUNT))
            .map(|(_, l)| l)
    }

    fn list_index(&self, slot: u8, id: u64) -> Result<u8> {
        self.state
            .lists
            .range((slot, 0)..(slot, MAX_LISTS_PER_ACCOUNT))
            .find(|(_, l)| l.id == id)
            .map(|((_, n), _)| *n)
            .ok_or(Error::NotFound)
    }

    pub fn list(&self, slot: u8, id: u64) -> Result<&ListRec> {
        let n = self.list_index(slot, id)?;
        Ok(&self.state.lists[&(slot, n)])
    }

    fn write_list(&mut self, slot: u8, n: u8, rec: ListRec) -> Result<()> {
        if self.state.lists.get(&(slot, n)) == Some(&rec) {
            return Ok(());
        }
        self.put(Ns::List, &keys::list(slot, n), Kind::List, &rec)?;
        self.state.lists.insert((slot, n), rec);
        Ok(())
    }

    pub fn create_list(
        &mut self,
        slot: u8,
        title: &str,
        replies_policy: RepliesPolicy,
        exclusive: bool,
        now_ms: u64,
    ) -> Result<u64> {
        let title = check_title(title)?;
        let n = (0..MAX_LISTS_PER_ACCOUNT)
            .find(|n| !self.state.lists.contains_key(&(slot, *n)))
            .ok_or_else(|| {
                Error::Invalid("Validation failed: You can have at most 8 lists".into())
            })?;
        self.govern(Some(slot), now_ms)?;
        let rec = ListRec {
            id: self.next_id(now_ms),
            title,
            replies_policy,
            exclusive,
            members: Vec::new(),
        };
        let id = rec.id;
        self.write_list(slot, n, rec)?;
        Ok(id)
    }

    pub fn update_list(
        &mut self,
        slot: u8,
        id: u64,
        update: ListUpdate,
        now_ms: u64,
    ) -> Result<()> {
        let n = self.list_index(slot, id)?;
        let mut rec = self.state.lists[&(slot, n)].clone();
        if let Some(title) = update.title {
            rec.title = check_title(&title)?;
        }
        if let Some(policy) = update.replies_policy {
            rec.replies_policy = policy;
        }
        if let Some(exclusive) = update.exclusive {
            rec.exclusive = exclusive;
        }
        self.govern(Some(slot), now_ms)?;
        self.write_list(slot, n, rec)
    }

    pub fn delete_list(&mut self, slot: u8, id: u64, now_ms: u64) -> Result<()> {
        let n = self.list_index(slot, id)?;
        self.govern(Some(slot), now_ms)?;
        self.erase(Ns::List, &keys::list(slot, n))?;
        self.state.lists.remove(&(slot, n));
        Ok(())
    }

    /// Add people the member follows (as on Mastodon).
    pub fn add_to_list(
        &mut self,
        slot: u8,
        id: u64,
        account_ids: &[u64],
        now_ms: u64,
    ) -> Result<()> {
        let n = self.list_index(slot, id)?;
        let mut rec = self.state.lists[&(slot, n)].clone();
        for account_id in account_ids {
            let member = self.state.slot_of(*account_id).ok_or(Error::NotFound)?;
            if !self.state.follows(slot, member) {
                return invalid("Validation failed: You must follow someone to add them to a list");
            }
            if !rec.members.contains(account_id) {
                rec.members.push(*account_id);
            }
        }
        self.govern(Some(slot), now_ms)?;
        self.write_list(slot, n, rec)
    }

    pub fn remove_from_list(
        &mut self,
        slot: u8,
        id: u64,
        account_ids: &[u64],
        now_ms: u64,
    ) -> Result<()> {
        let n = self.list_index(slot, id)?;
        let mut rec = self.state.lists[&(slot, n)].clone();
        rec.members.retain(|m| !account_ids.contains(m));
        self.govern(Some(slot), now_ms)?;
        self.write_list(slot, n, rec)
    }

    /// `owner` stopped following `member`: take them out of `owner`'s lists.
    pub(crate) fn leave_lists(&mut self, owner: u8, member: u8) -> Result<()> {
        let Some(member_id) = self.state.account(member).map(|a| a.rec.id) else {
            return Ok(());
        };
        let affected: Vec<(u8, ListRec)> = self
            .state
            .lists
            .range((owner, 0)..(owner, MAX_LISTS_PER_ACCOUNT))
            .filter(|(_, l)| l.members.contains(&member_id))
            .map(|((_, n), l)| (*n, l.clone()))
            .collect();
        for (n, mut rec) in affected {
            rec.members.retain(|m| *m != member_id);
            self.write_list(owner, n, rec)?;
        }
        Ok(())
    }

    /// Slots of a list's members that still exist.
    pub fn list_members(&self, list: &ListRec) -> u16 {
        list.members
            .iter()
            .filter_map(|id| self.state.slot_of(*id))
            .fold(0, |m, s| m | bit(s))
    }

    /// Members of the viewer's exclusive lists: kept off home.
    pub fn exclusive_mask(&self, viewer: u8) -> u16 {
        self.lists_of(viewer)
            .filter(|l| l.exclusive)
            .fold(0, |m, l| m | self.list_members(l))
    }

    /// `GET /api/v1/timelines/list/:id`: members' posts and boosts, with
    /// replies as the list's policy says.
    pub fn list_timeline(&self, viewer: u8, id: u64, q: &PageQuery) -> Result<Vec<(u64, Entry)>> {
        let list = self.list(viewer, id)?;
        let members = self.list_members(list);
        let policy = list.replies_policy;
        let hidden = self.state.hidden_mask(viewer);
        let keep = move |entry: Entry| -> bool {
            let Some(s) = self.visible(Some(viewer), entry.status_id()) else {
                return false;
            };
            match entry {
                Entry::Boost { id, .. } => {
                    let booster = self.state.boosts.get(&id).map_or(0, |b| bit(b.booster));
                    booster & members != 0 && (booster | bit(s.rec.author)) & hidden == 0
                }
                Entry::Status(_) => {
                    let author = s.rec.author;
                    if bit(author) & members == 0 || s.mentions & hidden & !bit(viewer) != 0 {
                        return false;
                    }
                    match s
                        .rec
                        .in_reply_to_account_id
                        .and_then(|a| self.state.slot_of(a))
                    {
                        Some(replied) if replied != author => match policy {
                            RepliesPolicy::Followed => {
                                replied == viewer || self.state.follows(viewer, replied)
                            }
                            RepliesPolicy::List => replied == viewer || bit(replied) & members != 0,
                            RepliesPolicy::None => false,
                        },
                        _ => true,
                    }
                }
            }
        };
        Ok(paginate(q, |bounds, asc| {
            Box::new(self.entries(bounds, asc).filter(move |(_, e)| keep(*e)))
        }))
    }
}
