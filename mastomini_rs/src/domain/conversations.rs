//! Direct-message conversations (`/api/v1/conversations`), derived from
//! `direct` statuses: nothing is stored (spec/02 "Derived at boot").
//!
//! A conversation is a thread of direct messages, named by its first
//! message. What a member has read or hidden lives in RAM, like markers.

use super::*;

/// Replies followed back to the first message, at most.
const MAX_THREAD_DEPTH: usize = 64;

/// One conversation as a member sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Conversation {
    /// The first message's id: the conversation's id.
    pub root: u64,
    /// The newest message the member can see.
    pub last: u64,
    /// Everyone in it (authors and mentions), as slots.
    pub participants: u16,
    pub unread: bool,
}

impl<S: Store> Service<S> {
    /// The first direct message of the thread `status` belongs to.
    fn conversation_root(&self, status: &Status) -> u64 {
        let mut root = status.rec.id;
        let mut parent = status.rec.in_reply_to_id;
        for _ in 0..MAX_THREAD_DEPTH {
            match parent.and_then(|p| self.state.statuses.get(&p)) {
                Some(s) if s.rec.visibility == Visibility::Direct => {
                    root = s.rec.id;
                    parent = s.rec.in_reply_to_id;
                }
                _ => break,
            }
        }
        root
    }

    /// The member's conversations, newest message first. Hidden ones stay
    /// hidden until a newer message arrives.
    pub fn conversations(&self, viewer: u8) -> Vec<Conversation> {
        let mut by_root: BTreeMap<u64, (u64, u16, u8)> = BTreeMap::new();
        for s in self.state.statuses.values() {
            if s.rec.visibility != Visibility::Direct
                || !self.can_see(Some(viewer), s)
                || self.state.hides(viewer, s.rec.author)
            {
                continue;
            }
            let root = self.conversation_root(s);
            let entry = by_root.entry(root).or_insert((0, 0, s.rec.author));
            if s.rec.id > entry.0 {
                entry.0 = s.rec.id;
                entry.2 = s.rec.author;
            }
            entry.1 |= bit(s.rec.author) | s.mentions;
        }
        let mut out: Vec<Conversation> = by_root
            .into_iter()
            .filter_map(|(root, (last, participants, last_author))| {
                let state = self
                    .state
                    .conversations
                    .get(&(viewer, root))
                    .copied()
                    .unwrap_or_default();
                (last > state.hidden_up_to).then_some(Conversation {
                    root,
                    last,
                    participants,
                    unread: last_author != viewer && last > state.read_up_to,
                })
            })
            .collect();
        out.sort_by_key(|c| std::cmp::Reverse(c.last));
        out
    }

    pub fn conversation(&self, viewer: u8, root: u64) -> Option<Conversation> {
        self.conversations(viewer)
            .into_iter()
            .find(|c| c.root == root)
    }

    /// Mark read (`hide == false`) or hide ("delete") a conversation. RAM
    /// only: no flash write.
    pub fn mark_conversation(&mut self, viewer: u8, root: u64, hide: bool) -> Result<()> {
        let last = self.conversation(viewer, root).ok_or(Error::NotFound)?.last;
        let state = self.state.conversations.entry((viewer, root)).or_default();
        state.read_up_to = last;
        if hide {
            state.hidden_up_to = last;
        }
        Ok(())
    }
}
