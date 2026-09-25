//! Local social features. Each action commits one bounded record. References
//! carry never-reused public account IDs so slot reuse cannot inherit data.

use super::*;
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

pub const MAX_FOLLOWED_TAGS: usize = 32;
pub const MAX_FEATURED_TAGS: usize = 10;
pub const MAX_ANNOUNCEMENTS: usize = 16;
pub const MAX_ANNOUNCEMENT_BYTES: usize = 2048;
pub const MAX_NOTE_BYTES: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeaturedTag {
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocialRec {
    pub account_id: u64,
    pub followed: Vec<FeaturedTag>,
    pub featured: Vec<FeaturedTag>,
    pub dismissed: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteRec {
    pub owner_id: u64,
    pub target_id: u64,
    pub comment: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnouncementRec {
    pub id: u64,
    pub text: String,
    pub published: bool,
    pub all_day: bool,
    pub starts_ms: Option<u64>,
    pub ends_ms: Option<u64>,
    pub published_ms: Option<u64>,
    pub updated_ms: u64,
    pub read_by: Vec<u64>,
    pub reactions: BTreeMap<String, Vec<u64>>,
}

impl AnnouncementRec {
    pub fn visible(&self, now: u64) -> bool {
        self.published
            && self.starts_ms.is_none_or(|t| t <= now)
            && self.ends_ms.is_none_or(|t| t >= now)
    }
}

pub fn tag_name(text: &str) -> Result<String> {
    let name = text.trim().trim_start_matches('#').to_lowercase();
    if name.is_empty()
        || name.len() > crate::text::MAX_TAG_BYTES
        || !name.chars().all(|c| c.is_alphanumeric() || c == '_')
        || !name.chars().any(char::is_alphabetic)
    {
        return invalid("Validation failed: Invalid hashtag (maximum 40 UTF-8 bytes)");
    }
    Ok(name)
}

fn pref_key(slot: u8) -> Key {
    Key::new(&format!("p{slot:x}")).unwrap()
}
fn note_key(src: u8, dst: u8) -> Key {
    Key::new(&format!("n{src:x}{dst:x}")).unwrap()
}
fn ann_key(id: u64) -> Key {
    Key::new(&format!("a{}", ids::b32_str(id))).unwrap()
}

impl<S: Store> Service<S> {
    pub fn social(&self, slot: u8) -> SocialRec {
        self.state
            .social
            .get(&slot)
            .cloned()
            .unwrap_or_else(|| SocialRec {
                account_id: self.state.account(slot).map_or(0, |a| a.rec.id),
                ..Default::default()
            })
    }

    fn save_social(&mut self, slot: u8, rec: SocialRec, now: u64) -> Result<()> {
        if self.social(slot) == rec {
            return Ok(());
        }
        self.govern(Some(slot), now)?;
        self.put(Ns::Social, &pref_key(slot), Kind::Social, &rec)?;
        self.state.social.insert(slot, rec);
        Ok(())
    }

    pub fn set_tag(
        &mut self,
        slot: u8,
        name: &str,
        featured: bool,
        on: bool,
        now: u64,
    ) -> Result<Option<u64>> {
        let name = tag_name(name)?;
        self.state.account(slot).ok_or(Error::NotFound)?;
        let mut rec = self.social(slot);
        let tags = if featured {
            &mut rec.featured
        } else {
            &mut rec.followed
        };
        let existing = tags.iter().find(|t| t.name == name).map(|t| t.id);
        if on == existing.is_some() {
            return Ok(existing);
        }
        let id = if on {
            let limit = if featured {
                MAX_FEATURED_TAGS
            } else {
                MAX_FOLLOWED_TAGS
            };
            if tags.len() >= limit {
                return invalid("Validation failed: Too many tags");
            }
            let id = self.next_id(now);
            tags.push(FeaturedTag { id, name });
            Some(id)
        } else {
            tags.retain(|t| t.name != name);
            None
        };
        self.save_social(slot, rec, now)?;
        Ok(id)
    }

    pub fn follows_tag(&self, slot: u8, tags: &[String]) -> bool {
        self.state
            .social
            .get(&slot)
            .is_some_and(|p| p.followed.iter().any(|t| tags.contains(&t.name)))
    }

    pub fn account_note(&self, owner: u8, target: u8) -> &str {
        self.state
            .account_notes
            .get(&(owner, target))
            .map_or("", |n| n.comment.as_str())
    }

    pub fn set_account_note(
        &mut self,
        owner: u8,
        target: u8,
        comment: &str,
        now: u64,
    ) -> Result<()> {
        let owner_id = self.state.account(owner).ok_or(Error::NotFound)?.rec.id;
        let target_id = self.state.account(target).ok_or(Error::NotFound)?.rec.id;
        if comment.len() > MAX_NOTE_BYTES {
            return invalid("Validation failed: Note exceeds 2000 UTF-8 bytes");
        }
        if self.account_note(owner, target) == comment {
            return Ok(());
        }
        self.govern(Some(owner), now)?;
        if comment.is_empty() {
            self.erase(Ns::Social, &note_key(owner, target))?;
            self.state.account_notes.remove(&(owner, target));
        } else {
            let rec = NoteRec {
                owner_id,
                target_id,
                comment: comment.to_string(),
            };
            self.put(
                Ns::Social,
                &note_key(owner, target),
                Kind::AccountNote,
                &rec,
            )?;
            self.state.account_notes.insert((owner, target), rec);
        }
        Ok(())
    }

    pub fn dismiss_suggestion(&mut self, owner: u8, target: u8, now: u64) -> Result<()> {
        let id = self.state.account(target).ok_or(Error::NotFound)?.rec.id;
        let mut rec = self.social(owner);
        if !rec.dismissed.contains(&id) {
            rec.dismissed.push(id);
        }
        self.save_social(owner, rec, now)
    }

    pub fn save_announcement(
        &mut self,
        actor: u8,
        mut rec: AnnouncementRec,
        now: u64,
    ) -> Result<u64> {
        if !self
            .state
            .account(actor)
            .is_some_and(|a| a.rec.role.is_admin())
        {
            return Err(Error::Forbidden("Admin only".into()));
        }
        rec.text = rec.text.trim().to_string();
        if rec.text.is_empty() || rec.text.len() > MAX_ANNOUNCEMENT_BYTES {
            return invalid(
                "Validation failed: Announcement text is required (maximum 2048 UTF-8 bytes)",
            );
        }
        if rec.starts_ms.zip(rec.ends_ms).is_some_and(|(a, b)| a > b) {
            return invalid("Validation failed: End precedes start");
        }
        if rec.id == 0 {
            if self.state.announcements.len() >= MAX_ANNOUNCEMENTS {
                return invalid(
                    "Validation failed: At most 16 announcements; delete an old one first",
                );
            }
            rec.id = self.next_id(now);
        }
        if rec.published && rec.published_ms.is_none() {
            rec.published_ms = Some(now);
        }
        rec.updated_ms = now;
        self.govern(Some(actor), now)?;
        self.put(Ns::Social, &ann_key(rec.id), Kind::Announcement, &rec)?;
        let id = rec.id;
        self.state.announcements.insert(id, rec);
        Ok(id)
    }

    pub fn delete_announcement(&mut self, actor: u8, id: u64, now: u64) -> Result<()> {
        if !self
            .state
            .account(actor)
            .is_some_and(|a| a.rec.role.is_admin())
        {
            return Err(Error::Forbidden("Admin only".into()));
        }
        if !self.state.announcements.contains_key(&id) {
            return Err(Error::NotFound);
        }
        self.govern(Some(actor), now)?;
        self.erase(Ns::Social, &ann_key(id))?;
        self.state.announcements.remove(&id);
        Ok(())
    }

    pub fn react_announcement(
        &mut self,
        actor: u8,
        id: u64,
        reaction: Option<(&str, bool)>,
        now: u64,
    ) -> Result<()> {
        let account = self.state.account(actor).ok_or(Error::NotFound)?.rec.id;
        let mut rec = self
            .state
            .announcements
            .get(&id)
            .filter(|r| r.visible(now))
            .ok_or(Error::NotFound)?
            .clone();
        match reaction {
            None => {
                if !rec.read_by.contains(&account) {
                    rec.read_by.push(account);
                }
            }
            Some((name, on)) => {
                if name.is_empty()
                    || name.len() > 32
                    || name.graphemes(true).count() != 1
                    || !name.chars().any(|c| matches!(c as u32,
                        0x1f000..=0x1faff | 0x2300..=0x27ff | 0x00a9 | 0x00ae | 0x203c | 0x2049 | 0x2122 | 0x2139 | 0x20e3 | 0x3030 | 0x303d | 0x3297 | 0x3299))
                {
                    return invalid("Validation failed: A Unicode emoji is required");
                }
                if on {
                    if !rec.reactions.contains_key(name) && rec.reactions.len() >= 8 {
                        return invalid("Validation failed: At most 8 reaction types");
                    }
                    let users = rec.reactions.entry(name.to_string()).or_default();
                    if !users.contains(&account) {
                        users.push(account);
                    }
                } else if let Some(users) = rec.reactions.get_mut(name) {
                    users.retain(|u| *u != account);
                    if users.is_empty() {
                        rec.reactions.remove(name);
                    }
                }
            }
        }
        if self.state.announcements.get(&id) == Some(&rec) {
            return Ok(());
        }
        self.govern(Some(actor), now)?;
        self.put(Ns::Social, &ann_key(id), Kind::Announcement, &rec)?;
        self.state.announcements.insert(id, rec);
        Ok(())
    }

    pub(crate) fn purge_social(&mut self, slot: u8, id: u64) -> Result<()> {
        self.erase(Ns::Social, &pref_key(slot))?;
        self.state.social.remove(&slot);
        let notes: Vec<_> = self
            .state
            .account_notes
            .keys()
            .filter(|(a, b)| *a == slot || *b == slot)
            .copied()
            .collect();
        for (a, b) in notes {
            self.erase(Ns::Social, &note_key(a, b))?;
            self.state.account_notes.remove(&(a, b));
        }
        let prefs: Vec<_> = self
            .state
            .social
            .iter()
            .filter(|(_, r)| r.dismissed.contains(&id))
            .map(|(s, r)| (*s, r.clone()))
            .collect();
        for (s, mut rec) in prefs {
            rec.dismissed.retain(|a| *a != id);
            self.put(Ns::Social, &pref_key(s), Kind::Social, &rec)?;
            self.state.social.insert(s, rec);
        }
        self.clean_announcement_accounts()
            .map_err(|e| self.map_store_error(e))
    }

    fn clean_announcement_accounts(&mut self) -> core::result::Result<(), StoreError> {
        let live: Vec<_> = self.state.active_accounts().map(|a| a.rec.id).collect();
        for rec in self.state.announcements.values_mut() {
            let old = rec.clone();
            rec.read_by.retain(|id| live.contains(id));
            rec.reactions
                .values_mut()
                .for_each(|v| v.retain(|id| live.contains(id)));
            rec.reactions.retain(|_, v| !v.is_empty());
            if old != *rec {
                self.store.set(
                    Ns::Social,
                    &ann_key(rec.id),
                    &codec::encode(Kind::Announcement, rec)?,
                )?;
            }
        }
        Ok(())
    }

    pub(crate) fn load_social(&mut self) -> core::result::Result<(), StoreError> {
        for (key, bytes) in load(&mut self.store, Ns::Social)? {
            let k = key.as_str().as_bytes();
            let valid = match k {
                [b'p', s] => {
                    let slot = unhex(*s).ok_or_else(|| corrupt(&key, "bad slot"))?;
                    let mut rec: SocialRec = codec::decode(Kind::Social, &bytes)?;
                    if rec.followed.len() > MAX_FOLLOWED_TAGS
                        || rec.featured.len() > MAX_FEATURED_TAGS
                        || rec.dismissed.len() > MAX_ACCOUNTS
                    {
                        return Err(corrupt(&key, "social bounds"));
                    }
                    for tag in rec.followed.iter().chain(&rec.featured) {
                        if tag_name(&tag.name).ok().as_ref() != Some(&tag.name) {
                            return Err(corrupt(&key, "tag"));
                        }
                        self.ids.observe(tag.id);
                    }
                    if self
                        .state
                        .account(slot)
                        .is_some_and(|a| a.rec.id == rec.account_id)
                    {
                        let old = rec.clone();
                        rec.dismissed
                            .retain(|id| self.state.account_by_id(*id).is_some());
                        if rec != old {
                            self.store.set(
                                Ns::Social,
                                &key,
                                &codec::encode(Kind::Social, &rec)?,
                            )?;
                        }
                        self.state.social.insert(slot, rec);
                        true
                    } else {
                        false
                    }
                }
                [b'n', s, d] => {
                    let a = unhex(*s).ok_or_else(|| corrupt(&key, "bad slot"))?;
                    let b = unhex(*d).ok_or_else(|| corrupt(&key, "bad slot"))?;
                    let rec: NoteRec = codec::decode(Kind::AccountNote, &bytes)?;
                    if rec.comment.len() > MAX_NOTE_BYTES {
                        return Err(corrupt(&key, "note bound"));
                    }
                    if self
                        .state
                        .account(a)
                        .is_some_and(|x| x.rec.id == rec.owner_id)
                        && self
                            .state
                            .account(b)
                            .is_some_and(|x| x.rec.id == rec.target_id)
                    {
                        self.state.account_notes.insert((a, b), rec);
                        true
                    } else {
                        false
                    }
                }
                [b'a', ..] => {
                    let rec: AnnouncementRec = codec::decode(Kind::Announcement, &bytes)?;
                    if ann_key(rec.id) != key
                        || rec.text.len() > MAX_ANNOUNCEMENT_BYTES
                        || rec.read_by.len() > MAX_ACCOUNTS
                        || rec.reactions.len() > 8
                        || rec
                            .reactions
                            .iter()
                            .any(|(k, v)| k.len() > 32 || v.len() > MAX_ACCOUNTS)
                    {
                        return Err(corrupt(&key, "announcement bounds"));
                    }
                    self.ids.observe(rec.id);
                    self.state.announcements.insert(rec.id, rec);
                    true
                }
                _ => return Err(corrupt(&key, "unexpected key")),
            };
            if !valid {
                self.store.erase(Ns::Social, &key)?;
                self.state.repairs += 1;
            }
        }
        if self.state.announcements.len() > MAX_ANNOUNCEMENTS {
            return Err(StoreError::Corrupt("announcement capacity".into()));
        }
        self.clean_announcement_accounts()
    }
}
