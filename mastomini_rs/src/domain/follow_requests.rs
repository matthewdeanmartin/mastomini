//! Follow requests for locked accounts (spec/04 "`locked` accounts").
//!
//! A request is a `Q` edge in `mm_rel`, the same shape as a follow.
//! Authorizing writes the follow (the commit point) and then erases the
//! request; boot drops a request whose follow already exists.

use super::*;

impl<S: Store> Service<S> {
    /// Ask to follow a locked account. Asking again updates the options.
    pub(crate) fn request_follow(
        &mut self,
        src: u8,
        dst: u8,
        reblogs: Option<bool>,
        notify: Option<bool>,
        now_ms: u64,
    ) -> Result<()> {
        let existing = self.state.follow_requests.get(&(src, dst)).copied();
        let rec = FollowRec {
            id: existing.map_or_else(|| self.next_id(now_ms), |e| e.id),
            reblogs: reblogs.unwrap_or(existing.is_none_or(|e| e.reblogs)),
            notify: notify.unwrap_or(existing.is_some_and(|e| e.notify)),
        };
        if existing == Some(rec) {
            return Ok(());
        }
        self.govern(Some(src), now_ms)?;
        self.put(
            Ns::Rel,
            &keys::follow_request(src, dst),
            Kind::FollowRequest,
            &rec,
        )?;
        self.state.follow_requests.insert((src, dst), rec);
        if existing.is_none() {
            self.notify(NotificationKind::FollowRequest, dst, src, None, now_ms);
        }
        Ok(())
    }

    /// Withdraw, reject or finish a request (the edge only).
    pub(crate) fn drop_follow_request(&mut self, src: u8, dst: u8) -> Result<()> {
        if self.state.follow_requests.remove(&(src, dst)).is_some() {
            self.erase(Ns::Rel, &keys::follow_request(src, dst))?;
        }
        self.state.notifications.retain(|n| {
            !(n.kind == NotificationKind::FollowRequest && n.to == dst && n.from == src)
        });
        Ok(())
    }

    /// `slot` accepts `requester`: the follow is written, then the request
    /// erased.
    pub fn authorize_follow(&mut self, slot: u8, requester: u8, now_ms: u64) -> Result<()> {
        let rec = *self
            .state
            .follow_requests
            .get(&(requester, slot))
            .ok_or(Error::NotFound)?;
        self.govern(Some(slot), now_ms)?;
        self.put(Ns::Rel, &keys::follow(requester, slot), Kind::Follow, &rec)?;
        self.state.follows.insert((requester, slot), rec);
        self.drop_follow_request(requester, slot)
    }

    pub fn reject_follow(&mut self, slot: u8, requester: u8, now_ms: u64) -> Result<()> {
        if !self.state.follow_requests.contains_key(&(requester, slot)) {
            return Err(Error::NotFound);
        }
        self.govern(Some(slot), now_ms)?;
        self.drop_follow_request(requester, slot)
    }

    /// When an account unlocks, everyone waiting gets in (as on Mastodon).
    pub(crate) fn authorize_all(&mut self, slot: u8) -> Result<()> {
        let waiting: Vec<u8> = self
            .state
            .follow_requests
            .keys()
            .filter(|(_, dst)| *dst == slot)
            .map(|(src, _)| *src)
            .collect();
        for src in waiting {
            let rec = self.state.follow_requests[&(src, slot)];
            self.put(Ns::Rel, &keys::follow(src, slot), Kind::Follow, &rec)?;
            self.state.follows.insert((src, slot), rec);
            self.drop_follow_request(src, slot)?;
        }
        Ok(())
    }

    /// Accounts waiting for `slot`'s answer, paged by request id.
    pub fn follow_requests_to(&self, slot: u8, q: &query::PageQuery) -> Vec<(u64, u8)> {
        let mut rows: Vec<(u64, u8)> = self
            .state
            .follow_requests
            .iter()
            .filter(|((_, dst), _)| *dst == slot)
            .map(|((src, _), rec)| (rec.id, *src))
            .collect();
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
}
