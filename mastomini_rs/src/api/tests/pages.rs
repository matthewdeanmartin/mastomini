//! The plain post, profile and hashtag pages and their browser sign-in
//! (api/pages.rs).

use super::*;

fn text(r: &crate::http::Response) -> String {
    String::from_utf8_lossy(&r.body).into_owned()
}

fn header<'a>(r: &'a crate::http::Response, name: &str) -> Option<&'a str> {
    r.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn post(s: &mut Server, token: &str, text: &str, visibility: &str) -> String {
    let r = s.post_form(
        "/api/v1/statuses",
        Some(token),
        &[("status", text), ("visibility", visibility)],
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    r.json_body()["id"].as_str().unwrap().to_string()
}

fn with_cookie(req: Request, cookie: &str) -> Request {
    req.with_header("Cookie", &format!("theme=dark; mm_session={cookie}"))
}

/// Sign in on the page; the session cookie's value.
fn sign_in(s: &mut Server, username: &str, password: &str) -> String {
    let r = s.send(
        Request::new("POST", "/web/signin")
            .with_header("Host", "mastomini.test")
            .with_header("Origin", "http://mastomini.test")
            .with_body(
                "application/x-www-form-urlencoded",
                form(&[
                    ("username", username),
                    ("password", password),
                    ("return", "/@alice"),
                ]),
            ),
    );
    assert_eq!(r.status, 302, "{}", text(&r));
    assert_eq!(header(&r, "Location"), Some("/@alice"));
    let set = header(&r, "Set-Cookie").unwrap();
    assert!(set.contains("HttpOnly") && set.contains("SameSite=Lax"));
    set.strip_prefix("mm_session=")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

#[test]
fn the_urls_in_the_api_open_as_pages() {
    let mut s = Server::provisioned();
    let token = s.login("alice", "alicepw", "read write follow");
    let id = post(&mut s, &token, "Hello <b>world</b> #garden", "public");
    let status = s
        .get(&format!("/api/v1/statuses/{id}"), Some(&token))
        .json_body();
    for url in [
        status["url"].as_str().unwrap(),
        status["account"]["url"].as_str().unwrap(),
        status["tags"][0]["url"].as_str().unwrap(),
    ] {
        let path = url.split_once("mastomini.test").unwrap().1;
        let r = s.get(path, None);
        assert_eq!(r.status, 200, "{path}");
        let page = text(&r);
        assert!(
            page.contains("Hello &lt;b&gt;world&lt;/b&gt;"),
            "{path}: {page}"
        );
        assert!(!page.contains("<b>world"), "{path}");
        assert!(header(&r, "Content-Security-Policy")
            .unwrap()
            .contains("default-src 'none'"));
        assert_eq!(header(&r, "Cache-Control"), Some("private, no-store"));
    }
    // Clients also write the `@` encoded, or with the domain.
    for path in [format!("/%40alice/{id}"), "/@alice@mastomini.test".into()] {
        assert_eq!(s.get(&path, None).status, 200, "{path}");
    }
    // The post under another member's name is not that post.
    let r = s.get(&format!("/@nobody/{id}"), None);
    assert_eq!(r.status, 404);
    assert_eq!(s.get("/@nobody", None).status, 404);
}

#[test]
fn anyone_sees_public_posts_members_see_what_they_may() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    // bob follows alice, so he may see her followers-only posts.
    let alice_id = s
        .get("/api/v1/accounts/lookup?acct=alice", Some(&bob))
        .json_body()["id"]
        .as_str()
        .unwrap()
        .to_string();
    s.post_form(
        &format!("/api/v1/accounts/{alice_id}/follow"),
        Some(&bob),
        &[],
    );
    let public = post(&mut s, &alice, "public words", "public");
    let unlisted = post(&mut s, &alice, "unlisted words", "unlisted");
    let private = post(&mut s, &alice, "followers words", "private");

    let anon = text(&s.get("/@alice", None));
    assert!(anon.contains("public words") && anon.contains("unlisted words"));
    assert!(!anon.contains("followers words"));
    assert!(anon.contains("/web/signin?return=%2F%40alice"));
    assert_eq!(s.get(&format!("/@alice/{private}"), None).status, 404);
    assert_eq!(s.get(&format!("/@alice/{unlisted}"), None).status, 200);
    assert_eq!(s.get(&format!("/@alice/{public}"), None).status, 200);

    let cookie = sign_in(&mut s, "bob", "secret");
    let r = s.send(with_cookie(Request::new("GET", "/@alice"), &cookie));
    let page = text(&r);
    assert!(page.contains("followers words"), "{page}");
    assert!(page.contains("Signed in as <a href=\"/@bob\">@bob</a>"));
    let r = s.send(with_cookie(
        Request::new("GET", &format!("/@alice/{private}")),
        &cookie,
    ));
    assert_eq!(r.status, 200);
    // A bearer token works the same (an app fetching the page).
    assert_eq!(s.get(&format!("/@alice/{private}"), Some(&bob)).status, 200);
    // The session is a signed-in device the member can see and end.
    let devices = s
        .get("/api/mastomini/v1/me/devices", Some(&bob))
        .json_body();
    assert!(devices
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["app"]["name"] == "Web browser"));
}

#[test]
fn the_session_cookie_is_for_pages_only_and_read_only() {
    let mut s = Server::provisioned();
    let cookie = sign_in(&mut s, "alice", "alicepw");
    // The API ignores cookies: a page elsewhere can't use them.
    let r = s.send(with_cookie(
        Request::new("GET", "/api/v1/accounts/verify_credentials"),
        &cookie,
    ));
    assert_eq!(r.status, 401);
    // The token is read-only even if someone lifted it into a header.
    let r = s.post_form("/api/v1/statuses", Some(&cookie), &[("status", "x")]);
    assert_eq!(r.status, 403);
}

#[test]
fn signing_in_checks_the_password_and_the_origin() {
    let mut s = Server::provisioned();
    let form_body = |password: &str| {
        form(&[
            ("username", "alice"),
            ("password", password),
            ("return", "//evil.example"),
        ])
    };
    let r = s.send(
        Request::new("POST", "/web/signin")
            .with_body("application/x-www-form-urlencoded", form_body("wrong")),
    );
    assert_eq!(r.status, 401);
    assert!(text(&r).contains("Wrong username or password."));
    assert!(header(&r, "Set-Cookie").is_none());
    // From another site's form: refused.
    let r = s.send(
        Request::new("POST", "/web/signin")
            .with_header("Host", "mastomini.test")
            .with_header("Origin", "https://evil.example")
            .with_body("application/x-www-form-urlencoded", form_body("alicepw")),
    );
    assert!(header(&r, "Set-Cookie").is_none());
    // Returning anywhere but this server goes to /about instead.
    let r = s.send(
        Request::new("POST", "/web/signin")
            .with_body("application/x-www-form-urlencoded", form_body("alicepw")),
    );
    assert_eq!(r.status, 302);
    assert_eq!(header(&r, "Location"), Some("/about"));
    // The form itself can't be framed.
    let r = s.get("/web/signin?return=/@alice", None);
    assert!(header(&r, "Content-Security-Policy")
        .unwrap()
        .contains("frame-ancestors 'none'"));
    assert!(text(&r).contains("value=\"/@alice\""));
}

#[test]
fn signing_out_ends_the_browser_session_only() {
    let mut s = Server::provisioned();
    let app = s.login("alice", "alicepw", "read write follow");
    let cookie = sign_in(&mut s, "alice", "alicepw");
    let r = s.send(with_cookie(
        Request::new("POST", "/web/signout?return=/@alice"),
        &cookie,
    ));
    assert_eq!(r.status, 302);
    assert!(header(&r, "Set-Cookie").unwrap().contains("Max-Age=0"));
    // The cookie no longer signs anyone in.
    let page = text(&s.send(with_cookie(Request::new("GET", "/@alice"), &cookie)));
    assert!(page.contains("Sign in</a>"));
    // An app token planted in the cookie is not signed out by it.
    s.send(with_cookie(Request::new("POST", "/web/signout"), &app));
    assert_eq!(
        s.get("/api/v1/accounts/verify_credentials", Some(&app))
            .status,
        200
    );
}

#[test]
fn direct_messages_stay_sealed_on_pages() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    s.add_member(&alice, "bob");
    let id = post(&mut s, &alice, "@bob secret plans", "direct");
    let cookie = sign_in(&mut s, "alice", "alicepw");
    let r = s.send(with_cookie(
        Request::new("GET", &format!("/@alice/{id}")),
        &cookie,
    ));
    assert_eq!(r.status, 200);
    let page = text(&r);
    assert!(!page.contains("secret plans"), "{page}");
    assert!(page.contains("Encrypted direct message"));
    assert_eq!(s.get(&format!("/@alice/{id}"), None).status, 404);
}

#[test]
fn hashtag_pages_list_public_posts() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    post(&mut s, &alice, "tomatoes #Garden", "public");
    post(&mut s, &alice, "quiet #garden", "unlisted");
    let page = text(&s.get("/tags/garden", None));
    assert!(page.contains("tomatoes"));
    assert!(!page.contains("quiet"));
    let page = text(&s.get("/@alice/tagged/garden", None));
    assert!(page.contains("tomatoes") && page.contains("quiet"));
}
