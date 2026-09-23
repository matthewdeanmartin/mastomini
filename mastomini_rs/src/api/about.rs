//! `/about`, `/terms-of-service` and `/privacy-policy`: the pages a Mastodon
//! app links to (from `/api/v2/instance` `configuration.urls`), rendered on
//! the server like the setup pages. No sign-in needed.

use super::{html, Call, Reply};
use crate::domain::records::Role;
use crate::store::Store;
use crate::text::{escape, render_plain};

fn nav(current: &str) -> String {
    let links = [
        ("/about", "About"),
        ("/terms-of-service", "Terms of service"),
        ("/privacy-policy", "Privacy policy"),
    ];
    let items: Vec<String> = links
        .iter()
        .map(|(href, label)| {
            if *href == current {
                format!("<strong>{label}</strong>")
            } else {
                format!("<a href=\"{href}\">{label}</a>")
            }
        })
        .collect();
    format!("<p class=\"hint\">{}</p>", items.join(" · "))
}

fn rules_html<S: Store>(c: &Call<'_, S>) -> String {
    let rules = &c.svc.state.server.rules;
    if rules.is_empty() {
        return "<p class=\"hint\">No rules yet.</p>".into();
    }
    let items: String = rules
        .iter()
        .map(|r| format!("<li>{}</li>", escape(r)))
        .collect();
    format!("<ol>{items}</ol>")
}

fn about<S: Store>(c: &Call<'_, S>) -> Reply {
    let server = &c.svc.state.server;
    let admins: Vec<String> = c
        .svc
        .state
        .active_accounts()
        .filter(|a| a.rec.role.is_admin())
        .map(|a| {
            let role = if a.rec.role == Role::Owner {
                "owner"
            } else {
                "admin"
            };
            format!("<li>@{} ({role})</li>", escape(&a.rec.username))
        })
        .collect();
    let members = c.svc.state.active_accounts().count();
    let body = format!(
        "{nav}{description}\
         <h2>Rules</h2>{rules}\
         <h2>Administered by</h2><ul>{admins}</ul>\
         <p class=\"hint\">{members} member{s}. Private: this server does not talk to other \
         Mastodon servers, and new accounts are made by an admin. \
         Server address: <code>{host}</code></p>",
        nav = nav("/about"),
        description = render_plain(&server.description),
        rules = rules_html(c),
        admins = admins.join(""),
        s = if members == 1 { "" } else { "s" },
        host = escape(c.ctx.host()),
    );
    Ok(html::page(200, &server.title, &body))
}

fn terms<S: Store>(c: &Call<'_, S>) -> Reply {
    let (text, effective_ms) = c.svc.terms_of_service();
    let body = format!(
        "{nav}<p class=\"hint\">Effective {date}</p>{text}",
        nav = nav("/terms-of-service"),
        date = super::time::date(effective_ms),
        text = render_plain(&text),
    );
    Ok(html::page(200, "Terms of service", &body))
}

fn privacy<S: Store>(c: &Call<'_, S>) -> Reply {
    let body = format!(
        "{}{}",
        nav("/privacy-policy"),
        render_plain(&c.svc.privacy_policy())
    );
    Ok(html::page(200, "Privacy policy", &body))
}

pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    Some(match (method, seg) {
        ("GET", ["about"]) => about(c),
        ("GET", ["terms-of-service" | "terms"]) => terms(c),
        ("GET", ["privacy-policy"]) => privacy(c),
        _ => return None,
    })
}
