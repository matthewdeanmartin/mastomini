//! The pages a household sees in a browser at the board's address:
//!
//! * unprovisioned: `/` is the first-time setup form (household name, owner
//!   username and password);
//! * provisioned: `/` points to the household app (`/app/`, where members and
//!   admins do everything else) and explains how to connect a Mastodon app;
//! * `/setup/<code>`: redeem an invite (choose a username and password) or a
//!   reset code (choose a new password);
//! * `/setup/password`: change your own password.
//!
//! These stay server-rendered because JavaScript or a signed-in user can't be
//! assumed here (spec/06). No sessions and no JavaScript: every form carries
//! what it needs.

use super::html::{self, page};
use super::{Call, Reply};
use crate::domain::records::{CodeKind, Role};
use crate::domain::{Error, NewMember, Redemption};
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

/// [`field`] without `required`.
fn optional_field(label: &str, name: &str, value: &str, hint: &str) -> String {
    field(label, name, "text", value, hint, "off").replace(" required>", ">")
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
fn how_to_connect(address: &str, https: bool) -> String {
    let transport = if https {
        "<p class=\"hint\">iPhone and Mac apps need this device to trust the household certificate first: <a href=\"/trust\">Trust this server</a>.</p>"
    } else {
        "<p class=\"hint\">This server uses plain HTTP. Apps that insist on HTTPS cannot connect to it.</p>"
    };
    format!(
        "<h2>Connect a Mastodon app</h2><ol>\
<li>Install a Mastodon app: Ice Cubes or Ivory on iPhone, Tusky or Mastodon on Android, or Whalebird on a computer.</li>\
<li>When it asks for a server, enter {address}.</li>\
<li>Sign in with your username and password.</li></ol>{transport}"
    )
}

/// The landing page: where to go next. The admin work happens in the
/// household app; this page stays usable without JavaScript.
fn landing<S: Store>(c: &Call<'_, S>) -> Response {
    let server = &c.svc.state.server;
    let body = format!(
        "<p>{}</p><div class=\"buttons\"><a class=\"button\" href=\"/app/#/connect\">Set up my phone</a><a class=\"button\" href=\"/app/\">Household app</a></div><p class=\"hint\">The household app has your signed-in devices and password, and for admins members, invite links and settings. <a href=\"/trust\">Trust this server</a> · <a href=\"/about\">About</a></p>{}<p class=\"hint\"><a href=\"/setup/password\">Change your password</a></p>",
        escape(&server.description),
        how_to_connect(&server_address(c), c.ctx.tls.is_some()),
    );
    page(200, &server.title, &body)
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
<div class=\"buttons\"><a class=\"button\" href=\"/app/#/admin/members\">Invite your family</a></div>",
                escape(&username),
                how_to_connect(&server_address(c), c.ctx.tls.is_some())
            );
            Ok(page(200, "All set", &body))
        }
        Err(e) => Ok(setup_form(c, 422, Some(&friendly(&e)))),
    }
}

fn code_form<S: Store>(c: &Call<'_, S>, code: &str, status: u16, error: Option<&str>) -> Response {
    let Some(kind) = c.svc.find_code(code, c.now).map(|r| r.kind) else {
        return html::message(
            404,
            "Link not valid",
            "This link has expired or has been used already. Ask an admin for a new one.",
        );
    };
    let p = &c.params;
    let passwords = format!(
        "{}{}",
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
    let (title, intro, fields) = match kind {
        CodeKind::Invite => (
            format!("Join {}", c.svc.state.server.title),
            "You have been invited to the household's Mastodon server. Choose your username and password."
                .to_string(),
            format!(
                "{}{}{passwords}",
                field(
                    "Username",
                    "username",
                    "text",
                    p.get("username").unwrap_or(""),
                    "Lowercase letters, digits and _ (up to 20). It cannot be changed later.",
                    "username"
                ),
                optional_field(
                    "Display name (optional)",
                    "display_name",
                    p.get("display_name").unwrap_or(""),
                    ""
                ),
            ),
        ),
        CodeKind::Reset { slot, .. } => (
            "Choose a new password".to_string(),
            format!(
                "Choose a new password for <code>{}</code>. Every device you are signed in on will be \
signed out, and your earlier direct messages can no longer be read.",
                escape(
                    &c.svc
                        .state
                        .account(slot)
                        .map_or(String::new(), |a| a.rec.username.clone())
                )
            ),
            passwords,
        ),
    };
    let body = format!(
        "<p>{intro}</p>{}<form method=\"post\" action=\"/setup/{}\">{fields}\
<div class=\"buttons\"><button class=\"approve\">Save</button></div></form>",
        html::error(error),
        escape(code),
    );
    page(status, &title, &body)
}

fn redeem<S: Store>(c: &mut Call<'_, S>, code: &str) -> Reply {
    let p = &c.params;
    let password = p.get("password").unwrap_or("").to_string();
    if password != p.get("password_confirm").unwrap_or("") {
        return Ok(code_form(
            c,
            code,
            422,
            Some("The two passwords are different."),
        ));
    }
    let redemption = Redemption {
        password,
        username: p.get("username").unwrap_or("").to_string(),
        display_name: p.get("display_name").unwrap_or("").to_string(),
    };
    let invite = c
        .svc
        .find_code(code, c.now)
        .is_some_and(|r| r.kind == CodeKind::Invite);
    match c.svc.redeem_code(code, redemption, c.now) {
        Ok(slot) => {
            let username = c
                .svc
                .state
                .account(slot)
                .map_or(String::new(), |a| a.rec.username.clone());
            let done = if invite {
                format!(
                    "Welcome! Your account <code>{}</code> is ready.",
                    escape(&username)
                )
            } else {
                format!(
                    "The password for <code>{}</code> is changed. Sign in again on each of your devices.",
                    escape(&username)
                )
            };
            let body = format!(
                "<p class=\"ok\">{done}</p>{}",
                how_to_connect(&server_address(c), c.ctx.tls.is_some())
            );
            Ok(page(200, "All set", &body))
        }
        Err(e) => Ok(code_form(c, code, 422, Some(&friendly(&e)))),
    }
}

fn password_form<S: Store>(c: &Call<'_, S>, status: u16, error: Option<&str>) -> Response {
    let body = format!(
        "<p>Every device you are signed in on will be signed out; sign in again with the new password.</p>{}\
<form method=\"post\" action=\"/setup/password\">{}{}{}{}\
<div class=\"buttons\"><button class=\"approve\">Change password</button></div></form>\
<p class=\"hint\">Forgot it? Ask an admin for a reset link.</p>",
        html::error(error),
        field(
            "Username",
            "username",
            "text",
            c.params.get("username").unwrap_or(""),
            "",
            "username"
        ),
        field(
            "Current password",
            "current",
            "password",
            "",
            "",
            "current-password"
        ),
        field(
            "New password",
            "password",
            "password",
            "",
            "At least 4 characters.",
            "new-password"
        ),
        field(
            "New password again",
            "password_confirm",
            "password",
            "",
            "",
            "new-password"
        ),
    );
    page(status, "Change your password", &body)
}

fn change_password<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let p = &c.params;
    let username = p.get("username").unwrap_or("").to_string();
    let current = p.get("current").unwrap_or("").to_string();
    let new = p.get("password").unwrap_or("").to_string();
    if new != p.get("password_confirm").unwrap_or("") {
        return Ok(password_form(
            c,
            422,
            Some("The two new passwords are different."),
        ));
    }
    // The sign-in lockout applies here too.
    let slot = match c.svc.check_password(&username, &current, c.now) {
        Ok(slot) => slot,
        Err(Error::Unauthorized) => {
            return Ok(password_form(c, 401, Some("Wrong username or password.")))
        }
        Err(e) => return Ok(password_form(c, 422, Some(&friendly(&e)))),
    };
    match c.svc.change_password(slot, &current, &new, c.now) {
        Ok(()) => Ok(html::message(
            200,
            "Password changed",
            "Your password is changed. Sign in again on each of your devices.",
        )),
        Err(e) => Ok(password_form(c, 422, Some(&friendly(&e)))),
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
        ("GET", []) => Ok(landing(c)),
        ("GET", ["setup"]) => Ok(Response::redirect("/")),
        ("POST", ["setup"]) => create_household(c),
        (_, ["setup", ..]) if !c.svc.state.provisioned => Ok(Response::redirect("/")),
        ("GET", ["setup", "password"]) => Ok(password_form(c, 200, None)),
        ("POST", ["setup", "password"]) => change_password(c),
        ("GET", ["setup", code]) => Ok(code_form(c, code, 200, None)),
        ("POST", ["setup", code]) => redeem(c, code),
        _ => return None,
    })
}
