//! Collections (Mastodon 4.6): a member's curated list of other members,
//! like a starter pack. In a household every addition is accepted at once;
//! the featured member is notified and can revoke their inclusion.

use super::*;

pub const MAX_COLLECTION_NAME_CHARS: usize = 40;
pub const MAX_COLLECTION_DESCRIPTION_CHARS: usize = 100;
const MAX_NAME_BYTES: usize = 160;
const MAX_DESCRIPTION_BYTES: usize = 400;

#[derive(Debug, Clone, Default)]
pub struct NewCollection {
    pub name: String,
    pub description: String,
    pub discoverable: Option<bool>,
    pub sensitive: bool,
    pub language: Option<String>,
    pub tag: Option<String>,
    pub account_ids: Vec<u64>,
}

/// `PATCH /api/v1/collections/:id`. `None` leaves a field unchanged.
#[derive(Debug, Clone, Default)]
pub struct CollectionUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
    pub discoverable: Option<bool>,
    pub sensitive: Option<bool>,
    pub language: Option<Option<String>>,
    pub tag: Option<Option<String>>,
}

fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return invalid("Validation failed: Name can't be blank");
    }
    if name.chars().count() > MAX_COLLECTION_NAME_CHARS || name.len() > MAX_NAME_BYTES {
        return invalid("Validation failed: Name is too long (maximum is 40 characters)");
    }
    Ok(())
}

fn validate_description(text: &str) -> Result<()> {
    if text.chars().count() > MAX_COLLECTION_DESCRIPTION_CHARS || text.len() > MAX_DESCRIPTION_BYTES
    {
        return invalid("Validation failed: Description is too long (maximum is 100 characters)");
    }
    Ok(())
}

fn validate_language(language: &Option<String>) -> Result<()> {
    if language.as_ref().is_some_and(|l| l.len() > 8) {
        return invalid("Validation failed: Language is invalid");
    }
    Ok(())
}

/// `#Tag` or `tag` -> `tag`; empty means none.
fn normalize_tag(tag: Option<String>) -> Result<Option<String>> {
    let Some(tag) = tag else { return Ok(None) };
    let name = tag.trim().trim_start_matches('#').to_string();
    if name.is_empty() {
        return Ok(None);
    }
    let word = name.chars().all(|c| c.is_alphanumeric() || c == '_');
    if !word || name.len() > crate::text::MAX_TAG_BYTES || name.chars().all(|c| c.is_ascii_digit())
    {
        return invalid("Validation failed: Tag is invalid");
    }
    Ok(Some(name))
}

impl<S: Store> Service<S> {
    /// Can `viewer` see this collection at all (by id)?
    pub fn collection_visible(&self, viewer: u8, c: &CollectionRec) -> bool {
        self.state.account(c.owner).is_some()
            && !self.state.suspended(c.owner)
            && (viewer == c.owner || !self.state.blocked_either(viewer, c.owner))
    }

    /// Items `viewer` may see: the curator sees everything; everyone else
    /// sees accepted items of live, unsuspended accounts they aren't
    /// blocked from.
    pub fn visible_items<'a>(
        &'a self,
        viewer: u8,
        c: &'a CollectionRec,
    ) -> impl Iterator<Item = &'a CollectionItemRec> + 'a {
        c.items.iter().filter(move |item| {
            let Some(slot) = self.state.slot_of(item.account_id) else {
                return false;
            };
            viewer == c.owner
                || (!item.revoked
                    && !self.state.suspended(slot)
                    && !self.state.blocked_either(viewer, slot))
        })
    }

    pub fn accepted_count(&self, c: &CollectionRec) -> usize {
        c.items
            .iter()
            .filter(|i| !i.revoked && self.state.slot_of(i.account_id).is_some())
            .count()
    }

    fn owned_collection(&self, actor: u8, id: u64) -> Result<CollectionRec> {
        let c = self.state.collections.get(&id).ok_or(Error::NotFound)?;
        if !self.collection_visible(actor, c) {
            return Err(Error::NotFound);
        }
        if c.owner != actor {
            return Err(Error::Forbidden("This action is not allowed".into()));
        }
        Ok(c.clone())
    }

    fn write_collection(&mut self, rec: CollectionRec) -> Result<()> {
        self.put(Ns::Coll, &keys::collection(rec.id), Kind::Collection, &rec)?;
        self.state.collections.insert(rec.id, rec);
        Ok(())
    }

    /// Check that `account_id` may be featured by `owner` in `rec`, and
    /// return its slot.
    fn featurable(&self, owner: u8, rec: &CollectionRec, account_id: u64) -> Result<u8> {
        let account = self
            .state
            .account_by_id(account_id)
            .ok_or_else(|| Error::Invalid("Validation failed: Account is invalid".into()))?;
        let slot = account.slot;
        if slot == owner {
            return invalid("Validation failed: You can't feature yourself");
        }
        if self.state.suspended(slot) || self.state.blocked_either(owner, slot) {
            return invalid("Validation failed: Account is invalid");
        }
        if !account.rec.discoverable {
            return invalid("Validation failed: This account does not want to be featured");
        }
        match rec.items.iter().find(|i| i.account_id == account_id) {
            Some(item) if item.revoked => {
                invalid("Validation failed: This account removed itself from the collection")
            }
            Some(_) => invalid("Validation failed: Account is already in this collection"),
            None => Ok(slot),
        }
    }

    pub fn create_collection(&mut self, owner: u8, new: NewCollection, now_ms: u64) -> Result<u64> {
        self.writable()?;
        let name = new.name.trim().to_string();
        let description = new.description.trim().to_string();
        validate_name(&name)?;
        validate_description(&description)?;
        validate_language(&new.language)?;
        let tag = normalize_tag(new.tag)?;
        let owned = self
            .state
            .collections
            .values()
            .filter(|c| c.owner == owner)
            .count();
        if owned >= MAX_COLLECTIONS_PER_ACCOUNT {
            return invalid("Validation failed: You can have at most 8 collections");
        }
        let mut rec = CollectionRec {
            id: 0,
            owner,
            name,
            description,
            discoverable: new.discoverable.unwrap_or(true),
            sensitive: new.sensitive,
            language: new.language,
            tag,
            items: Vec::new(),
            updated_ms: now_ms,
        };
        let mut added = Vec::new();
        for account_id in new.account_ids {
            let slot = self.featurable(owner, &rec, account_id)?;
            rec.items.push(CollectionItemRec {
                id: 0,
                account_id,
                revoked: false,
            });
            added.push(slot);
        }
        self.govern(Some(owner), now_ms)?;
        rec.id = self.next_id(now_ms);
        for item in &mut rec.items {
            item.id = self.ids.next(now_ms);
        }
        let id = rec.id;
        self.write_collection(rec)?;
        for slot in added {
            self.notify_about(
                NotificationKind::AddedToCollection,
                slot,
                owner,
                None,
                Some(id),
                now_ms,
            );
        }
        Ok(id)
    }

    pub fn update_collection(
        &mut self,
        actor: u8,
        id: u64,
        update: CollectionUpdate,
        now_ms: u64,
    ) -> Result<()> {
        let mut rec = self.owned_collection(actor, id)?;
        let before = rec.clone();
        if let Some(name) = update.name {
            let name = name.trim().to_string();
            validate_name(&name)?;
            rec.name = name;
        }
        if let Some(description) = update.description {
            let description = description.trim().to_string();
            validate_description(&description)?;
            rec.description = description;
        }
        if let Some(v) = update.discoverable {
            rec.discoverable = v;
        }
        if let Some(v) = update.sensitive {
            rec.sensitive = v;
        }
        if let Some(language) = update.language {
            validate_language(&language)?;
            rec.language = language;
        }
        if let Some(tag) = update.tag {
            rec.tag = normalize_tag(tag)?;
        }
        if rec == before {
            return Ok(());
        }
        rec.updated_ms = now_ms;
        self.govern(Some(actor), now_ms)?;
        self.write_collection(rec)
    }

    pub fn delete_collection(&mut self, actor: u8, id: u64, now_ms: u64) -> Result<()> {
        self.owned_collection(actor, id)?;
        self.govern(Some(actor), now_ms)?;
        self.erase(Ns::Coll, &keys::collection(id))?;
        self.state.collections.remove(&id);
        self.state
            .notifications
            .retain(|n| !(n.kind == NotificationKind::AddedToCollection && n.object == Some(id)));
        Ok(())
    }

    /// Returns the new item's id.
    pub fn add_collection_item(
        &mut self,
        actor: u8,
        id: u64,
        account_id: u64,
        now_ms: u64,
    ) -> Result<u64> {
        let mut rec = self.owned_collection(actor, id)?;
        let slot = self.featurable(actor, &rec, account_id)?;
        self.govern(Some(actor), now_ms)?;
        let item_id = self.next_id(now_ms);
        rec.items.push(CollectionItemRec {
            id: item_id,
            account_id,
            revoked: false,
        });
        rec.updated_ms = now_ms;
        self.write_collection(rec)?;
        self.notify_about(
            NotificationKind::AddedToCollection,
            slot,
            actor,
            None,
            Some(id),
            now_ms,
        );
        Ok(item_id)
    }

    /// The curator removes an item. A revoked item stays (as revoked) so the
    /// account can't be added back without its consent.
    pub fn remove_collection_item(
        &mut self,
        actor: u8,
        id: u64,
        item_id: u64,
        now_ms: u64,
    ) -> Result<()> {
        let mut rec = self.owned_collection(actor, id)?;
        let item = *rec
            .items
            .iter()
            .find(|i| i.id == item_id)
            .ok_or(Error::NotFound)?;
        if item.revoked {
            return Ok(());
        }
        rec.items.retain(|i| i.id != item_id);
        rec.updated_ms = now_ms;
        self.govern(Some(actor), now_ms)?;
        self.write_collection(rec)?;
        if let Some(slot) = self.state.slot_of(item.account_id) {
            self.state.notifications.retain(|n| {
                !(n.kind == NotificationKind::AddedToCollection
                    && n.object == Some(id)
                    && n.to == slot)
            });
        }
        Ok(())
    }

    /// The featured account removes itself from someone's collection.
    pub fn revoke_collection_item(
        &mut self,
        actor: u8,
        id: u64,
        item_id: u64,
        now_ms: u64,
    ) -> Result<()> {
        let c = self.state.collections.get(&id).ok_or(Error::NotFound)?;
        if !self.collection_visible(actor, c) {
            return Err(Error::NotFound);
        }
        let item = *c
            .items
            .iter()
            .find(|i| i.id == item_id)
            .ok_or(Error::NotFound)?;
        let me = self.state.account(actor).map(|a| a.rec.id);
        if Some(item.account_id) != me {
            return Err(Error::Forbidden("This action is not allowed".into()));
        }
        if item.revoked {
            return Ok(());
        }
        let mut rec = c.clone();
        for i in &mut rec.items {
            if i.id == item_id {
                i.revoked = true;
            }
        }
        rec.updated_ms = now_ms;
        self.govern(Some(actor), now_ms)?;
        self.write_collection(rec)
    }

    /// Collections `owner` curates, as `viewer` may list them: all of them
    /// for the curator, discoverable ones for everyone else. Oldest first.
    pub fn collections_of(&self, viewer: u8, owner: u8) -> Vec<u64> {
        self.state
            .collections
            .values()
            .filter(|c| c.owner == owner && self.collection_visible(viewer, c))
            .filter(|c| viewer == owner || c.discoverable)
            .map(|c| c.id)
            .collect()
    }

    /// Collections that feature `account` (accepted items only).
    pub fn collections_featuring(&self, viewer: u8, account: u8) -> Vec<u64> {
        let Some(account_id) = self.state.account(account).map(|a| a.rec.id) else {
            return Vec::new();
        };
        self.state
            .collections
            .values()
            .filter(|c| self.collection_visible(viewer, c))
            .filter(|c| viewer == account || viewer == c.owner || c.discoverable)
            .filter(|c| {
                c.items
                    .iter()
                    .any(|i| i.account_id == account_id && !i.revoked)
            })
            .map(|c| c.id)
            .collect()
    }

    /// Account deletion: drop the account's collections, and its items in
    /// everyone else's.
    pub(crate) fn purge_collections(&mut self, slot: u8, account_id: u64) -> Result<()> {
        let owned: Vec<u64> = self
            .state
            .collections
            .values()
            .filter(|c| c.owner == slot)
            .map(|c| c.id)
            .collect();
        for id in owned {
            self.erase(Ns::Coll, &keys::collection(id))?;
            self.state.collections.remove(&id);
        }
        let featuring: Vec<CollectionRec> = self
            .state
            .collections
            .values()
            .filter(|c| c.items.iter().any(|i| i.account_id == account_id))
            .cloned()
            .collect();
        for mut rec in featuring {
            rec.items.retain(|i| i.account_id != account_id);
            self.write_collection(rec)?;
        }
        Ok(())
    }
}
