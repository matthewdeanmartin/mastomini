//! The pages a household sees in a browser at the board's address:
//!
//! * unprovisioned: `/` is the first-time setup form (household name, owner
//!   username and password);
//! * provisioned: `/` explains how to connect a Mastodon app and offers an
//!   "add a family member" form, authorised by an admin's password.
//!
//! No sessions and no JavaScript: every form carries what it needs.

use super::html::{self, page};
use super::{Call, Reply};
use crate::domain::records::Role;
use crate::domain::{Error, NewMember};
use crate::http::Response;
use crate::store::Store;
use crate::text::escape;

fn field(
    label: &str,
    name: &str,
    kind: &str,
    value: &str,
    hint: &str,
    autocomplete: &str,
) -> String {
    format!(
        "<label for=\"{name}\">{label}</label>\
<input type=\"{kind}\" id=\"{name}\" name=\"{name}\" value=\"{}\" autocomplete=\"{autocomplete}\" autocapitalize=\"none\" autocorrect=\"off\" required>\
{}",
        escape(value),
        if hint.is_empty() {
            String::new()
        } else {
            format!("<div class=\"hint\">{hint}</div>")
        }
    )
}

fn setup_form<S: Store>(c: &Call<'_, S>, status: u16, error: Option<&str>) -> Response {
    let p = &c.params;
    let body = format!(
        "<p>Welcome! This board is a private Mastodon server for your household. \
Create the household and your own account. You will be its owner.</p>{}\
<form method=\"post\" action=\"/setup\">\
{}{}{}{}\
<div class=\"buttons\"><button class=\"approve\">Create household</button></div></form>",
        html::error(error),
        field(
            "Household name",
            "title",
            "text",
            p.get("title").unwrap_or(""),
            "For example: The Martins",
            "off"
        ),
        field(
            "Your username",
            "username",
            "text",
            p.get("username").unwrap_or(""),
            "Lowercase letters, digits and _ (up to 20). It cannot be changed later.",
            "username"
        ),
        field(
            "Password",
            "password",
            "password",
            "",
            "At least 4 characters.",
            "new-password"
        ),
        field(
            "Password again",
            "password_confirm",
            "password",
            "",
            "",
            "new-password"
        ),
    );
    page(status, "Set up mastomini", &body)
}

/// The address to type into an app: the one this browser used (an IP works
/// on every phone), plus the configured name when it differs.
fn server_address<S: Store>(c: &Call<'_, S>) -> String {
    let configured = c.ctx.host();
    match c
        .req
        .header("Host")
        .map(str::trim)
        .filter(|h| !h.is_empty())
    {
        Some(visited) if !visited.eq_ignore_ascii_case(configured) => format!(
            "<code>{}</code> (or <code>{}</code>)",
            escape(visited),
            escape(configured)
        ),
        _ => format!("<code>{}</code>", escape(configured)),
    }
}

/// How to connect an app, shown after setup and on the landing page.
/// `address` is trusted HTML from [`server_address`].
fn how_to_connect(address: &str) -> String {
    format!(
        "<h2>Connect a Mastodon app</h2><ol>\
<li>Install a Mastodon app: Ice Cubes or Ivory on iPhone, Tusky or Mastodon on Android, or Whalebird on a computer.</li>\
<li>When it asks for a server, enter {address}.</li>\
<li>Sign in with your username and password.</li></ol>\
<p class=\"hint\">This server uses plain HTTP for now. Apps that insist on HTTPS cannot connect yet.</p>"
    )
}

fn member_form<S: Store>(c: &Call<'_, S>, notice: Option<&str>, error: Option<&str>) -> String {
    let p = &c.params;
    format!(
        "<h2>Add a family member</h2>{}{}\
<form method=\"post\" action=\"/setup/members\">\
{}{}\
<p class=\"hint\">To confirm, sign in as the owner or an admin:</p>\
{}{}\
<div class=\"buttons\"><button class=\"approve\">Add member</button></div></form>",
        notice.map_or(String::new(), |n| format!(
            "<p class=\"ok\">{}</p>",
            escape(n)
        )),
        html::error(error),
        field(
            "New member's username",
            "username",
            "text",
            p.get("username").unwrap_or(""),
            "Lowercase letters, digits and _ (up to 20).",
            "off"
        ),
        field(
            "New member's password",
            "password",
            "password",
            "",
            "Tell them this password; they can sign in right away.",
            "new-password"
        ),
        field(
            "Admin username",
            "admin_username",
            "text",
            p.get("admin_username").unwrap_or(""),
            "",
            "username"
        ),
        field(
            "Admin password",
            "admin_password",
            "password",
            "",
            "",
            "current-password"
        ),
    )
}

fn landing<S: Store>(
    c: &Call<'_, S>,
    status: u16,
    notice: Option<&str>,
    error: Option<&str>,
) -> Response {
    let server = &c.svc.state.server;
    let members: Vec<String> = c
        .svc
        .state
        .active_accounts()
        .map(|a| format!("<code>{}</code>", escape(&a.rec.username)))
        .collect();
    let body = format!(
        "<p>{}</p>{}<h2>Members</h2><p>{}</p>{}",
        escape(&server.description),
        how_to_connect(&server_address(c)),
        members.join(", "),
        member_form(c, notice, error),
    );
    page(status, &server.title, &body)
}

fn create_household<S: Store>(c: &mut Call<'_, S>) -> Reply {
    if c.svc.state.provisioned {
        return Ok(Response::redirect("/"));
    }
    let p = &c.params;
    let password = p.get("password").unwrap_or("").to_string();
    if password != p.get("password_confirm").unwrap_or("") {
        return Ok(setup_form(c, 422, Some("The two passwords are different.")));
    }
    let owner = NewMember {
        username: p.get("username").unwrap_or("").trim().to_ascii_lowercase(),
        password,
        display_name: String::new(),
        role: Role::Owner,
    };
    let title = p
        .get("title")
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    let username = owner.username.clone();
    match c.svc.provision(owner, title, c.now) {
        Ok(_) => {
            let body = format!(
                "<p class=\"ok\">Your household is ready and <code>{}</code> is its owner.</p>{}\
<div class=\"buttons\"><a class=\"button\" href=\"/\">Add family members</a></div>",
                escape(&username),
                how_to_connect(&server_address(c))
            );
            Ok(page(200, "All set", &body))
        }
        Err(e) => Ok(setup_form(c, 422, Some(&friendly(&e)))),
    }
}

fn add_member<S: Store>(c: &mut Call<'_, S>) -> Reply {
    if !c.svc.state.provisioned {
        return Ok(Response::redirect("/"));
    }
    let p = &c.params;
    let admin = p.get("admin_username").unwrap_or("").to_string();
    let admin_password = p.get("admin_password").unwrap_or("").to_string();
    let member = NewMember {
        username: p.get("username").unwrap_or("").trim().to_ascii_lowercase(),
        password: p.get("password").unwrap_or("").to_string(),
        display_name: String::new(),
        role: Role::Member,
    };
    let actor = match c.svc.check_password(&admin, &admin_password, c.now) {
        Ok(slot) => slot,
        Err(Error::Unauthorized) => {
            return Ok(landing(
                c,
                401,
                None,
                Some("Wrong admin username or password."),
            ))
        }
        Err(e) => return Ok(landing(c, 422, None, Some(&friendly(&e)))),
    };
    let username = member.username.clone();
    match c.svc.create_member(actor, member, c.now) {
        Ok(_) => {
            let notice = format!("Added {username}. They can sign in now.");
            // A fresh form, not the one just submitted.
            c.params = Default::default();
            Ok(landing(c, 200, Some(&notice), None))
        }
        Err(e) => Ok(landing(c, 422, None, Some(&friendly(&e)))),
    }
}

/// Domain errors read like API messages; trim the API prefix for people.
fn friendly(e: &Error) -> String {
    let text = e.to_string();
    text.strip_prefix("Validation failed: ")
        .unwrap_or(&text)
        .to_string()
}

/// Shown instead of a setup form while the clock is not set (writes need it).
pub(crate) fn clock_not_set() -> Response {
    html::message(
        503,
        "One moment",
        "The board is still setting its clock from the internet. Wait a few seconds and try again.",
    )
    .with_header("Retry-After", "5")
}

pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    Some(match (method, seg) {
        ("GET", [] | ["setup"]) if !c.svc.state.provisioned => Ok(setup_form(c, 200, None)),
        ("GET", []) => Ok(landing(c, 200, None, None)),
        ("GET", ["setup"]) => Ok(Response::redirect("/")),
        ("POST", ["setup"]) => create_household(c),
        ("POST", ["setup", "members"]) => add_member(c),
        _ => return None,
    })
}
