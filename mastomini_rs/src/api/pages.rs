//! Plain, read-only pages for the links Mastodon apps open "on the original
//! site": a post (`/@alice/<id>`), a profile (`/@alice`, `/@alice/tagged/x`)
//! and a hashtag (`/tags/x`), the `url`s in the API's JSON.
//!
//! Anyone on the network sees public and unlisted posts, as they would on
//! Mastodon. A member can sign in (`/web/signin`) to see what they may see
//! in their app: the session is a read-only token in an HttpOnly cookie
//! (`local_tokens.rs`), used by these pages only, never by the API, so a page
//! elsewhere cannot make the browser act for the member. Direct messages
//! stay sealed to the member's app devices. No JavaScript.

use super::{entities, html, time, Call, Reply};
use crate::domain::query::{AccountStatusesQuery, Entry, PageQuery};
use crate::domain::records::Visibility;
use crate::domain::{Account, Error, Status};
use crate::http::Response;
use crate::store::Store;
use crate::text::escape;
use serde_json::Value;

const COOKIE: &str = "mm_session";
const PAGE_SIZE: usize = 20;
/// Thirty days; the session also ends like any device (signed out, password
/// changed, evicted by newer sign-ins).
const COOKIE_MAX_AGE: u64 = 30 * 24 * 60 * 60;

const STYLE: &str = "main{max-width:600px}header.bar{display:flex;justify-content:space-between;align-items:center;gap:8px;font-size:.9em;margin-bottom:12px}\
header.bar form{display:inline}header.bar button{padding:4px 10px;font-size:.9em;background:#ddd;border:0;border-radius:6px;cursor:pointer}\
.profile{display:flex;gap:12px;align-items:center}.profile h1{margin:0}img.avatar{width:56px;height:56px;border-radius:10px}\
img.small{width:36px;height:36px;border-radius:8px}.post{border-top:1px solid #ddd;padding:12px 0;display:flex;gap:10px}\
.post.focus{background:#f0edff;border-radius:8px;padding:12px}.post .body{flex:1;min-width:0;overflow-wrap:anywhere}\
.meta,.muted{color:#666;font-size:.85em}.who a{color:inherit;text-decoration:none}.label{font-size:.75em;border:1px solid #999;border-radius:4px;padding:0 4px;margin-left:4px}\
summary{cursor:pointer;color:#563acc}.more{display:block;margin-top:16px}.content p{margin:.4em 0}\
@media (prefers-color-scheme:dark){.post{border-color:#333}.post.focus{background:#2a2540}.meta,.muted{color:#aaa}header.bar button{background:#444;color:#eee}}";

/// Who is looking: a bearer token (an app fetching the page) or the
/// session cookie. Read access is enough; a disabled member reads as anyone.
fn viewer<S: Store>(c: &mut Call<'_, S>) -> Option<u8> {
    if let Some(p) = &c.principal {
        return p.slot.filter(|_| p.allows("read") && !p.disabled);
    }
    let token = session_cookie(c)?.to_string();
    let p = c.svc.principal(&token, c.now)?;
    p.slot.filter(|_| p.allows("read") && !p.disabled)
}

fn session_cookie<'a, S: Store>(c: &'a Call<'_, S>) -> Option<&'a str> {
    c.req
        .header("Cookie")?
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE)
        .map(|(_, value)| value.trim())
        .filter(|v| !v.is_empty())
}

fn cookie<S: Store>(c: &Call<'_, S>, value: &str, max_age: u64) -> String {
    format!(
        "{COOKIE}={value}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Lax{}",
        if c.req.secure { "; Secure" } else { "" }
    )
}

/// A place to return to after signing in or out: a path on this server.
fn local_path(to: Option<&str>) -> String {
    match to {
        Some(p)
            if p.starts_with('/')
                && !p.starts_with("//")
                && p.len() <= 512
                && !p.contains('\\')
                && !p.chars().any(char::is_control) =>
        {
            p.to_string()
        }
        _ => "/about".to_string(),
    }
}

/// The path and query of this request, for "sign in and come back".
fn here<S: Store>(c: &Call<'_, S>) -> String {
    if c.req.query.is_empty() {
        c.req.path.clone()
    } else {
        format!("{}?{}", c.req.path, c.req.query)
    }
}

fn layout<S: Store>(c: &Call<'_, S>, viewer: Option<u8>, title: &str, body: &str) -> String {
    let server = escape(&c.svc.state.server.title);
    let back = crate::http::encode(&here(c));
    let who = match viewer.and_then(|v| c.svc.state.account(v)) {
        Some(a) => format!(
            "<span>Signed in as <a href=\"/@{u}\">@{u}</a> <form method=\"post\" action=\"/web/signout?return={back}\"><button type=\"submit\">Sign out</button></form></span>",
            u = escape(&a.rec.username),
        ),
        None => format!("<a href=\"/web/signin?return={back}\">Sign in</a>"),
    };
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{t} · {server}</title><style>{base}{STYLE}</style></head><body><main><header class=\"bar\"><a href=\"/about\">{server}</a>{who}</header>{body}</main></body></html>",
        t = escape(title),
        base = html::STYLE,
    )
}

/// Pages differ by viewer, and show member content: never cached, never
/// framed, no scripts, no third-party anything.
fn respond(status: u16, page: String) -> Response {
    Response::html(status, page)
        .with_header("Cache-Control", "private, no-store")
        .with_header(
            "Content-Security-Policy",
            "default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
        )
        .with_header("Referrer-Policy", "same-origin")
}

fn not_found<S: Store>(c: &Call<'_, S>, viewer: Option<u8>) -> Response {
    let hint = if viewer.is_none() {
        "<p class=\"muted\">It may be visible only to members: sign in to look again.</p>"
    } else {
        ""
    };
    let body =
        format!("<h1>Not found</h1><p>There is nothing here, or you can't see it.</p>{hint}");
    respond(404, layout(c, viewer, "Not found", &body))
}

fn str_of<'v>(v: &'v Value, key: &str) -> &'v str {
    v[key].as_str().unwrap_or("")
}

/// One post (with who boosted it, if it came in as a boost). `content` and
/// `note` are HTML the server rendered from escaped text.
fn post_html<S: Store>(
    c: &Call<'_, S>,
    viewer: Option<u8>,
    s: &Status,
    boosted_by: Option<&Account>,
    focus: bool,
) -> String {
    let v = entities::status(c.svc, c.ctx, s, viewer);
    let account = &v["account"];
    let name = match str_of(account, "display_name") {
        "" => str_of(account, "username"),
        n => n,
    };
    let content = str_of(&v, "content");
    let body = match str_of(&v, "spoiler_text") {
        "" => format!("<div class=\"content\">{content}</div>"),
        cw => format!(
            "<details><summary>{}</summary><div class=\"content\">{content}</div></details>",
            escape(cw)
        ),
    };
    let boost = boosted_by.map_or(String::new(), |b| {
        format!(
            "<div class=\"meta\">↻ <a href=\"/@{u}\">@{u}</a> boosted</div>",
            u = escape(&b.rec.username)
        )
    });
    let reply = s
        .rec
        .in_reply_to_id
        .filter(|_| !focus)
        .map_or(String::new(), |_| {
            "<span class=\"meta\"> · a reply</span>".into()
        });
    let poll = if v["poll"].is_null() {
        ""
    } else {
        "<p class=\"muted\">Poll: open it in your app to vote.</p>"
    };
    let visibility = match s.rec.visibility {
        Visibility::Public => "",
        Visibility::Unlisted => " · unlisted",
        Visibility::Private => " · followers only",
        Visibility::Direct => " · direct message",
    };
    let edited = if v["edited_at"].is_null() {
        ""
    } else {
        " · edited"
    };
    let when = str_of(&v, "created_at");
    let bot = if account["bot"] == true {
        "<span class=\"label\">bot</span>"
    } else {
        ""
    };
    format!(
        "<article class=\"post{focus}\"><img class=\"small\" src=\"{avatar}\" alt=\"\"><div class=\"body\">{boost}\
         <div class=\"who\"><a href=\"/@{user}\"><strong>{name}</strong> <span class=\"meta\">@{user}</span></a>{bot}</div>\
         {body}{poll}<div class=\"meta\"><a href=\"{url}\">{date}</a>{visibility}{edited}{reply} · {r} replies · {b} boosts · {f} favourites</div></div></article>",
        focus = if focus { " focus" } else { "" },
        avatar = escape(&format!("/avatars/{}.png", str_of(account, "id"))),
        user = escape(str_of(account, "username")),
        name = escape(name),
        url = escape(&format!("/@{}/{}", str_of(account, "username"), s.rec.id)),
        date = escape(&when.get(..16).unwrap_or(when).replace('T', " ")),
        r = v["replies_count"],
        b = v["reblogs_count"],
        f = v["favourites_count"],
    )
}

fn entries_html<S: Store>(c: &Call<'_, S>, viewer: Option<u8>, entries: &[(u64, Entry)]) -> String {
    let state = &c.svc.state;
    entries
        .iter()
        .filter_map(|(_, entry)| match *entry {
            Entry::Status(id) => state
                .statuses
                .get(&id)
                .map(|s| post_html(c, viewer, s, None, false)),
            Entry::Boost { id, target } => {
                let booster = state.boosts.get(&id).and_then(|b| state.account(b.booster));
                let s = c.svc.visible(viewer, target)?;
                Some(post_html(c, viewer, s, booster, false))
            }
        })
        .collect()
}

/// "Older posts" when the page is full.
fn older<S: Store>(c: &Call<'_, S>, entries: &[(u64, Entry)]) -> String {
    match entries.last() {
        Some((last, _)) if entries.len() >= PAGE_SIZE => format!(
            "<a class=\"more\" href=\"{}?max_id={last}\">Older posts</a>",
            escape(&c.req.path)
        ),
        _ => String::new(),
    }
}

fn page_query<S: Store>(c: &Call<'_, S>) -> PageQuery {
    PageQuery {
        max_id: c.params.u64("max_id"),
        limit: PAGE_SIZE,
        ..Default::default()
    }
}

fn decode(segment: &str) -> String {
    percent_encoding::percent_decode_str(segment)
        .decode_utf8_lossy()
        .into_owned()
}

/// `@alice`, `%40alice`, or `@alice@mastomini.local`: the username.
fn username(segment: &str) -> Option<String> {
    let text = decode(segment);
    let rest = text.strip_prefix('@')?;
    let name = rest.split('@').next().unwrap_or("");
    (!name.is_empty()).then(|| name.to_string())
}

fn profile<S: Store>(c: &mut Call<'_, S>, user: &str, tagged: Option<String>) -> Reply {
    let viewer = viewer(c);
    let Some(account) = c.svc.state.account_by_username(user).cloned() else {
        return Ok(not_found(c, viewer));
    };
    let a = entities::account(c.svc, c.ctx, &account);
    let name = match str_of(&a, "display_name") {
        "" => str_of(&a, "username"),
        n => n,
    };
    let bot = if account.rec.bot {
        "<span class=\"label\">bot</span>"
    } else {
        ""
    };
    let header = format!(
        "<div class=\"profile\"><img class=\"avatar\" src=\"/avatars/{id}.png\" alt=\"\"><div><h1>{name}{bot}</h1><div class=\"meta\">@{user}@{host}</div></div></div>\
         <div class=\"content\">{note}</div><p class=\"meta\">{posts} posts · {following} following · {followers} followers · joined {joined}</p>",
        id = account.rec.id,
        name = escape(name),
        user = escape(&account.rec.username),
        host = escape(c.ctx.host()),
        note = str_of(&a, "note"),
        posts = a["statuses_count"],
        following = a["following_count"],
        followers = a["followers_count"],
        joined = escape(&time::date(crate::ids::millis(account.rec.id))),
    );
    let title = format!("{name} (@{})", account.rec.username);
    if c.svc.state.suspended(account.slot) {
        let body = format!("{header}<p class=\"muted\">This account is suspended.</p>");
        return Ok(respond(200, layout(c, viewer, &title, &body)));
    }
    let heading = tagged.as_ref().map_or(String::new(), |t| {
        format!(
            "<p><strong>Posts tagged #{}</strong> · <a href=\"/@{}\">all posts</a></p>",
            escape(t),
            escape(&account.rec.username)
        )
    });
    let opts = AccountStatusesQuery {
        tagged,
        ..Default::default()
    };
    let entries = c
        .svc
        .account_statuses(viewer, account.slot, &opts, &page_query(c));
    let posts = entries_html(c, viewer, &entries);
    let posts = if posts.is_empty() {
        "<p class=\"muted\">No posts to show.</p>".to_string()
    } else {
        posts
    };
    let body = format!("{header}{heading}{posts}{}", older(c, &entries));
    Ok(respond(200, layout(c, viewer, &title, &body)))
}

fn post<S: Store>(c: &mut Call<'_, S>, user: &str, id: &str) -> Reply {
    let viewer = viewer(c);
    let found = crate::ids::parse(id)
        .and_then(|id| c.svc.visible(viewer, id))
        .filter(|s| {
            c.svc
                .state
                .account(s.rec.author)
                .is_some_and(|a| a.rec.username.eq_ignore_ascii_case(user))
        })
        .map(|s| s.rec.id);
    let Some(id) = found else {
        return Ok(not_found(c, viewer));
    };
    let (ancestors, descendants) = c.svc.context(viewer, id).unwrap_or_default();
    let state = &c.svc.state;
    let render = |ids: &[u64]| -> String {
        ids.iter()
            .filter_map(|i| state.statuses.get(i))
            .map(|s| post_html(c, viewer, s, None, false))
            .collect()
    };
    let status = &state.statuses[&id];
    let author = state
        .account(status.rec.author)
        .map_or(String::new(), |a| a.rec.username.clone());
    let body = format!(
        "{}{}{}",
        render(&ancestors),
        post_html(c, viewer, status, None, true),
        render(&descendants)
    );
    Ok(respond(
        200,
        layout(c, viewer, &format!("Post by @{author}"), &body),
    ))
}

fn tag<S: Store>(c: &mut Call<'_, S>, name: &str) -> Reply {
    let viewer = viewer(c);
    let name = decode(name).to_lowercase();
    let q = page_query(c);
    // As Mastodon: hashtag pages list public posts (and, for a member,
    // what their public timeline would).
    let entries = match viewer {
        Some(v) => c.svc.tag_timeline(v, &name, &q),
        None => c.svc.status_page(&q, |s| {
            s.tags.contains(&name)
                && s.rec.visibility == Visibility::Public
                && c.svc.can_see(None, s)
        }),
    };
    let posts = entries_html(c, viewer, &entries);
    let posts = if posts.is_empty() {
        "<p class=\"muted\">No posts to show.</p>".to_string()
    } else {
        posts
    };
    let body = format!("<h1>#{}</h1>{posts}{}", escape(&name), older(c, &entries));
    Ok(respond(200, layout(c, viewer, &format!("#{name}"), &body)))
}

/// A form post must come from these pages. Browsers send `Origin` on POST;
/// SameSite=Lax already keeps the cookie off cross-site posts.
fn same_origin<S: Store>(c: &Call<'_, S>) -> bool {
    match c.req.header("Origin") {
        None => true,
        Some(origin) => {
            let host = origin.split_once("://").map_or("", |(_, h)| h);
            c.req
                .header("Host")
                .is_some_and(|h| h.eq_ignore_ascii_case(host))
        }
    }
}

fn signin_form<S: Store>(c: &Call<'_, S>, back: &str, error: Option<&str>) -> Response {
    let body = format!(
        "<h1>Sign in</h1>{err}<form method=\"post\" action=\"/web/signin\">\
         <input type=\"hidden\" name=\"return\" value=\"{back}\">\
         <label for=\"u\">Username</label><input id=\"u\" type=\"text\" name=\"username\" autocomplete=\"username\" autocapitalize=\"none\" required>\
         <label for=\"p\">Password</label><input id=\"p\" type=\"password\" name=\"password\" autocomplete=\"current-password\" required>\
         <div class=\"buttons\"><button class=\"approve\" type=\"submit\">Sign in</button></div></form>\
         <p class=\"hint\">To read posts only members can see, in this browser. It appears under \
         <em>Signed-in devices</em> in the household app, where you can sign it out.</p>",
        err = html::error(error),
        back = escape(back),
    );
    let status = if error.is_some() { 401 } else { 200 };
    respond(status, layout(c, None, "Sign in", &body))
}

fn signin_post<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let back = local_path(c.params.get("return"));
    if !same_origin(c) {
        return Ok(signin_form(
            c,
            &back,
            Some("Sign in from this server's own page."),
        ));
    }
    let username = c.params.get("username").unwrap_or("").trim().to_string();
    let password = c.params.get("password").unwrap_or("").to_string();
    match c.svc.browser_session(&username, &password, c.now) {
        Ok(token) => {
            // Replace any session this browser already had.
            if let Some(old) = session_cookie(c).map(str::to_string) {
                let _ = end_browser_session(c, &old);
            }
            Ok(Response::redirect(&back)
                .with_header("Set-Cookie", &cookie(c, &token, COOKIE_MAX_AGE))
                .with_header("Cache-Control", "no-store"))
        }
        Err(Error::Unauthorized) => Ok(signin_form(c, &back, Some("Wrong username or password."))),
        Err(e) => Ok(signin_form(c, &back, Some(&e.to_string()))),
    }
}

fn signout<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let back = local_path(c.params.get("return"));
    if !same_origin(c) {
        return Ok(Response::redirect(&back));
    }
    if let Some(token) = session_cookie(c).map(str::to_string) {
        end_browser_session(c, &token).map_err(super::fail)?;
    }
    Ok(Response::redirect(&back).with_header("Set-Cookie", &cookie(c, "", 0)))
}

/// Revoke `token` if it is a browser session: whatever a cookie holds, it
/// can't sign out an app or an API key.
fn end_browser_session<S: Store>(c: &mut Call<'_, S>, token: &str) -> Result<(), Error> {
    let hash = crate::auth::sha256(token);
    let browser = c.svc.state.tokens.iter().any(|t| {
        t.rec.hash == hash
            && c.svc.state.local_token(t.rec.id).map(|l| l.kind)
                == Some(crate::domain::LocalKind::Browser)
    });
    if browser {
        c.svc.revoke(token)?;
    }
    Ok(())
}

pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    Some(match (method, seg) {
        ("GET", ["web", "signin"]) => {
            let back = local_path(c.params.get("return"));
            Ok(signin_form(c, &back, None))
        }
        ("POST", ["web", "signin"]) => signin_post(c),
        ("POST", ["web", "signout"]) => signout(c),
        ("GET", ["tags", name]) => tag(c, name),
        ("GET", [user]) => profile(c, &username(user)?, None),
        ("GET", [user, "tagged", name]) => profile(c, &username(user)?, Some(decode(name))),
        ("GET", [user, id]) => post(c, &username(user)?, id),
        _ => return None,
    })
}
