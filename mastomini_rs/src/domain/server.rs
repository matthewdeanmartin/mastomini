//! Server settings an admin can change: name, description, rules and the
//! terms of service (spec/06 `GET/PUT /admin/server`).

use super::*;

pub const MAX_DESCRIPTION: usize = 280;
pub const MAX_RULES: usize = 8;
pub const MAX_RULE: usize = 140;
/// Fits the 4 KiB API body with room for the other fields.
pub const MAX_TERMS: usize = 3000;

/// `None` leaves a setting unchanged. `terms: Some("")` goes back to the
/// generated terms.
#[derive(Debug, Clone, Default)]
pub struct ServerUpdate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub rules: Option<Vec<String>>,
    pub terms: Option<String>,
}

impl<S: Store> Service<S> {
    pub fn update_server(&mut self, actor: u8, update: ServerUpdate, now_ms: u64) -> Result<()> {
        self.require_admin(actor)?;
        let mut server = self.state.server.clone();
        if let Some(title) = update.title {
            let title = title.trim().to_string();
            if title.is_empty() || title.len() > accounts::MAX_TITLE {
                return invalid("Validation failed: Title must be 1-40 bytes");
            }
            server.title = title;
        }
        if let Some(description) = update.description {
            let description = description.trim().to_string();
            if description.len() > MAX_DESCRIPTION {
                return invalid("Validation failed: Description is too long (280 bytes)");
            }
            server.description = description;
        }
        if let Some(rules) = update.rules {
            let rules: Vec<String> = rules
                .into_iter()
                .map(|r| r.trim().to_string())
                .filter(|r| !r.is_empty())
                .collect();
            if rules.len() > MAX_RULES || rules.iter().any(|r| r.len() > MAX_RULE) {
                return invalid("Validation failed: At most 8 rules of 140 bytes");
            }
            server.rules = rules;
        }
        let terms = match update.terms {
            Some(text) => {
                let text = text.replace("\r\n", "\n").trim().to_string();
                if text.len() > MAX_TERMS {
                    return invalid("Validation failed: Terms are too long (3000 bytes)");
                }
                let unchanged = match &self.state.terms {
                    Some(t) => t.text == text,
                    None => text.is_empty(),
                };
                (!unchanged).then(|| {
                    (!text.is_empty()).then_some(TermsRec {
                        text,
                        effective_ms: now_ms,
                    })
                })
            }
            None => None,
        };
        if server == self.state.server && terms.is_none() {
            return Ok(());
        }
        self.govern(Some(actor), now_ms)?;
        if server != self.state.server {
            self.put(Ns::Cfg, &keys::server(), Kind::Server, &server)?;
            self.state.server = server;
        }
        match terms {
            Some(Some(rec)) => {
                self.put(Ns::Cfg, &keys::terms(), Kind::Terms, &rec)?;
                self.state.terms = Some(rec);
            }
            Some(None) => {
                self.erase(Ns::Cfg, &keys::terms())?;
                self.state.terms = None;
            }
            None => {}
        }
        Ok(())
    }

    /// When the household was set up: the owner account's creation time.
    pub fn provisioned_ms(&self) -> u64 {
        self.state
            .active_accounts()
            .find(|a| a.rec.role == Role::Owner)
            .map_or(0, |a| ids::millis(a.rec.id))
    }

    /// The terms of service as plain text, and when they took effect.
    pub fn terms_of_service(&self) -> (String, u64) {
        if let Some(t) = &self.state.terms {
            return (t.text.clone(), t.effective_ms);
        }
        let server = &self.state.server;
        let mut text = format!(
            "{} is a private server for one household. It runs on a small \
             microcontroller at home and does not talk to any other server.\n\n\
             By using it you agree to the rules:",
            server.title
        );
        for (i, rule) in server.rules.iter().enumerate() {
            text.push_str(&format!("\n{}. {rule}", i + 1));
        }
        text.push_str(
            "\n\nThe household admins may remove posts or accounts that break \
             the rules. Old posts are deleted automatically when the server's \
             storage fills up, so keep copies of anything you want to keep.",
        );
        (text, self.provisioned_ms())
    }

    /// Generated privacy policy: what is kept, where, and for how long.
    pub fn privacy_policy(&self) -> String {
        format!(
            "Everything on {title} stays on the device in your home. Nothing is \
             sent to other servers, and there are no ads, trackers or analytics.\n\n\
             Stored: your account (username, display name, bio, profile fields, \
             a password hash), the apps you signed in with, and your posts, \
             favourites, bookmarks, boosts, follows, blocks, mutes, reports and \
             collections. Old posts are deleted automatically when storage fills \
             up.\n\n\
             Direct messages are encrypted. Only the people in the conversation \
             can read them, with their password or a device they signed in on. \
             The household admins cannot read them. Who wrote to whom, and when, \
             is not encrypted.\n\n\
             Not stored: notifications and read positions are kept in memory only \
             and are forgotten when the device restarts. IP addresses and e-mail \
             addresses are not collected.",
            title = self.state.server.title
        )
    }
}
