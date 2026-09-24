//! Content filters (spec/04 "Filters"), Mastodon's v2 model: up to 8 per
//! member, each with up to 4 keywords and 4 specific statuses. The server
//! annotates matching statuses (`Status.filtered`); clients warn, blur or
//! hide them, as with Mastodon.

use super::*;

pub const MAX_FILTER_TITLE: usize = 40;
pub const MAX_FILTER_KEYWORD: usize = 40;

/// The bit for a context name (`home`, `notifications`, ...).
pub fn filter_context_bit(name: &str) -> Option<u8> {
    FILTER_CONTEXTS
        .iter()
        .position(|c| *c == name)
        .map(|i| 1 << i)
}

/// Does `keyword` occur in `text` (both lowercased)? `whole_word` needs
/// non-alphanumeric characters (or the ends) on both sides.
fn contains_keyword(text: &str, keyword: &str, whole_word: bool) -> bool {
    if keyword.is_empty() {
        return false;
    }
    if !whole_word {
        return text.contains(keyword);
    }
    let edge = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
    text.match_indices(keyword).any(|(at, _)| {
        edge(text[..at].chars().next_back()) && edge(text[at + keyword.len()..].chars().next())
    })
}

/// A filter that matched a status: which keywords and which status entries.
#[derive(Debug, Clone)]
pub struct FilterMatch<'a> {
    pub filter: &'a FilterRec,
    pub keywords: Vec<&'a str>,
    pub statuses: Vec<u64>,
}

impl<S: Store> Service<S> {
    pub fn filters_of(&self, slot: u8) -> impl Iterator<Item = &FilterRec> {
        self.state
            .filters
            .range((slot, 0)..(slot, MAX_FILTERS_PER_ACCOUNT))
            .map(|(_, f)| f)
    }

    fn filter_index(&self, slot: u8, id: u64) -> Result<u8> {
        self.state
            .filters
            .range((slot, 0)..(slot, MAX_FILTERS_PER_ACCOUNT))
            .find(|(_, f)| f.id == id)
            .map(|((_, n), _)| *n)
            .ok_or(Error::NotFound)
    }

    pub fn filter(&self, slot: u8, id: u64) -> Result<&FilterRec> {
        let n = self.filter_index(slot, id)?;
        Ok(&self.state.filters[&(slot, n)])
    }

    /// The member's filter holding keyword (or status entry) `id`.
    pub fn filter_with_part(&self, slot: u8, id: u64) -> Result<&FilterRec> {
        self.filters_of(slot)
            .find(|f| {
                f.keywords.iter().any(|k| k.id == id) || f.statuses.iter().any(|s| s.id == id)
            })
            .ok_or(Error::NotFound)
    }

    /// Create (`rec.id == 0`) or replace a filter. New keywords and status
    /// entries (`id == 0`) get ids. Returns the filter's id.
    pub fn save_filter(&mut self, slot: u8, mut rec: FilterRec, now_ms: u64) -> Result<u64> {
        rec.title = rec.title.trim().to_string();
        if rec.title.is_empty() || rec.title.len() > MAX_FILTER_TITLE {
            return invalid("Validation failed: Title must be 1-40 bytes");
        }
        if rec.context == 0 {
            return invalid("Validation failed: Choose where the filter applies");
        }
        if rec.keywords.len() > MAX_FILTER_KEYWORDS {
            return invalid("Validation failed: A filter has at most 4 keywords");
        }
        if rec.statuses.len() > MAX_FILTER_STATUSES {
            return invalid("Validation failed: A filter has at most 4 posts");
        }
        for k in &mut rec.keywords {
            k.keyword = k.keyword.trim().to_string();
            if k.keyword.is_empty() || k.keyword.len() > MAX_FILTER_KEYWORD {
                return invalid("Validation failed: Keywords must be 1-40 bytes");
            }
        }
        let n = if rec.id == 0 {
            (0..MAX_FILTERS_PER_ACCOUNT)
                .find(|n| !self.state.filters.contains_key(&(slot, *n)))
                .ok_or_else(|| {
                    Error::Invalid("Validation failed: You can have at most 8 filters".into())
                })?
        } else {
            self.filter_index(slot, rec.id)?
        };
        if self.state.filters.get(&(slot, n)) == Some(&rec) {
            return Ok(rec.id);
        }
        self.govern(Some(slot), now_ms)?;
        if rec.id == 0 {
            rec.id = self.next_id(now_ms);
        }
        for k in &mut rec.keywords {
            if k.id == 0 {
                k.id = self.next_id(now_ms);
            }
        }
        for s in &mut rec.statuses {
            if s.id == 0 {
                s.id = self.next_id(now_ms);
            }
        }
        let id = rec.id;
        self.put(Ns::Filt, &keys::filter(slot, n), Kind::Filter, &rec)?;
        self.state.filters.insert((slot, n), rec);
        Ok(id)
    }

    pub fn delete_filter(&mut self, slot: u8, id: u64, now_ms: u64) -> Result<()> {
        let n = self.filter_index(slot, id)?;
        self.govern(Some(slot), now_ms)?;
        self.erase(Ns::Filt, &keys::filter(slot, n))?;
        self.state.filters.remove(&(slot, n));
        Ok(())
    }

    /// The viewer's live filters for `context` that match a status: its
    /// text as the viewer reads it (direct messages decrypted for them
    /// only), or the status itself.
    pub fn filter_matches(&self, viewer: u8, context: u8, status: &Status) -> Vec<FilterMatch<'_>> {
        let now = self.state.now_ms;
        let mut text: Option<String> = None;
        self.filters_of(viewer)
            .filter(|f| f.context & context != 0 && f.expires_ms.is_none_or(|e| e > now))
            .filter_map(|f| {
                let statuses: Vec<u64> = f
                    .statuses
                    .iter()
                    .filter(|s| s.status_id == status.rec.id)
                    .map(|s| s.status_id)
                    .collect();
                let keywords: Vec<&str> = if f.keywords.is_empty() {
                    Vec::new()
                } else {
                    let text = text.get_or_insert_with(|| {
                        let (body, spoiler) = self.readable_text(Some(viewer), status);
                        format!("{spoiler}\n{body}").to_lowercase()
                    });
                    f.keywords
                        .iter()
                        .filter(|k| contains_keyword(text, &k.keyword.to_lowercase(), k.whole_word))
                        .map(|k| k.keyword.as_str())
                        .collect()
                };
                (!keywords.is_empty() || !statuses.is_empty()).then_some(FilterMatch {
                    filter: f,
                    keywords,
                    statuses,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::contains_keyword;

    #[test]
    fn whole_words_and_substrings() {
        assert!(contains_keyword("a dragon appears", "dragon", true));
        assert!(!contains_keyword("snapdragons", "dragon", true));
        assert!(contains_keyword("snapdragons", "dragon", false));
        assert!(contains_keyword("dragon!", "dragon", true));
        assert!(contains_keyword("über drache", "über", true));
        assert!(!contains_keyword("text", "", false));
    }
}
