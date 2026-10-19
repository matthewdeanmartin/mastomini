//! Durable bounded event history. Existing status/account records are untouched.
use super::*;

/// Account IDs prevent a recycled slot from inheriting another member's events.
#[derive(serde::Serialize, serde::Deserialize)]
struct NotificationRec {
    event: Notification,
    recipient_id: u64,
    sender_id: u64,
}

fn key(id: u64) -> Key {
    Key::from_parts(&[b"n", &ids::b32(id)]).expect("notification key")
}

impl<S: Store> Service<S> {
    pub(super) fn load_notifications(&mut self) -> core::result::Result<(), StoreError> {
        // Stream records into one bounded allocation (PSRAM on the S3), avoiding
        // a second copy of every blob and hundreds of small tree allocations.
        let mut events = Vec::with_capacity(MAX_NOTIFICATIONS);
        let mut repairs = Vec::new();
        let state = &self.state;
        self.store.for_each(Ns::Notif, &mut |stored_key, bytes| {
            let rec: NotificationRec = codec::decode(Kind::Notification, bytes)?;
            let n = rec.event;
            if *stored_key != key(n.id) || n.id == 0 {
                return Err(StoreError::Corrupt("invalid notification key".into()));
            }
            self.ids.observe(n.id);
            let valid = state.account(n.to).map(|a| a.rec.id) == Some(rec.recipient_id)
                && state.account(n.from).map(|a| a.rec.id) == Some(rec.sender_id)
                && !state.blocked_either(n.to, n.from)
                && n.status.is_none_or(|id| state.statuses.contains_key(&id))
                && match n.kind {
                    NotificationKind::Favourite => n.status.is_some_and(|id| {
                        state
                            .reaction_ids
                            .contains_key(&(ReactionKind::Favourite, id, n.from))
                    }),
                    NotificationKind::Reblog => n.status.is_some_and(|id| {
                        state
                            .boosts
                            .values()
                            .any(|b| b.target == id && b.booster == n.from)
                    }),
                    NotificationKind::FollowRequest => {
                        state.follow_requests.contains_key(&(n.from, n.to))
                    }
                    NotificationKind::AdminReport => {
                        n.object.is_some_and(|id| state.reports.contains_key(&id))
                    }
                    NotificationKind::AddedToCollection => n
                        .object
                        .and_then(|id| state.collections.get(&id))
                        .is_some_and(|c| {
                            c.items
                                .iter()
                                .any(|i| i.account_id == rec.recipient_id && !i.revoked)
                        }),
                    _ => true,
                };
            if events.len() + repairs.len() >= MAX_NOTIFICATIONS {
                return Err(StoreError::Corrupt("notification capacity exceeded".into()));
            }
            if valid {
                events.push(n);
            } else {
                repairs.push(*stored_key);
            }
            Ok(())
        })?;
        for stored_key in repairs {
            self.store.erase(Ns::Notif, &stored_key)?;
            self.state.repairs += 1;
        }
        events.sort_unstable_by_key(|n| n.id);
        self.state.notifications = events.into();
        Ok(())
    }

    pub(super) fn persist_notification(&mut self, event: Notification) -> Result<()> {
        self.writable()?;
        let rec = NotificationRec {
            event,
            recipient_id: self.state.account(event.to).ok_or(Error::NotFound)?.rec.id,
            sender_id: self
                .state
                .account(event.from)
                .ok_or(Error::NotFound)?
                .rec
                .id,
        };
        if self.state.notifications.len() >= MAX_NOTIFICATIONS {
            let oldest = self.state.notifications.front().expect("full history").id;
            self.erase(Ns::Notif, &key(oldest))?;
            self.state.notifications.pop_front();
        }
        self.put(Ns::Notif, &key(event.id), Kind::Notification, &rec)?;
        self.state.notifications.push_back(event);
        Ok(())
    }

    /// Erase before changing RAM. On partial failure the operation reports an
    /// error; already erased events stay erased after reboot. Never rewrites posts.
    pub(crate) fn retain_notifications(
        &mut self,
        keep: impl Fn(&Notification) -> bool,
    ) -> Result<()> {
        self.writable()?;
        let ids: Vec<_> = self
            .state
            .notifications
            .iter()
            .filter(|n| !keep(n))
            .map(|n| n.id)
            .collect();
        for id in ids {
            self.erase(Ns::Notif, &key(id))?;
            self.state.notifications.retain(|n| n.id != id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_flash_budget_includes_nvs_overhead() {
        let rec = NotificationRec {
            event: Notification {
                id: u64::MAX,
                kind: NotificationKind::AddedToCollection,
                to: 31,
                from: 30,
                status: Some(u64::MAX),
                object: Some(u64::MAX),
            },
            recipient_id: u64::MAX,
            sender_id: u64::MAX,
        };
        let bytes = codec::encode(Kind::Notification, &rec).unwrap();
        let flash = crate::store::entries_for(bytes.len()) * crate::store::ENTRY_SIZE;
        assert_eq!(flash, 128);
        assert_eq!(flash * MAX_NOTIFICATIONS, 64 * 1024);
    }
}
