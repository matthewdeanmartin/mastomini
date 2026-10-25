//! Feeds from other servers: once a night, fetch the RSS feed of every
//! account household members follow on other servers, and hand the new
//! posts (and small previews of their images) to mastomini
//! (spec/remote_follow_rss.md).
//!
//! Its "Mastodon server" is the household board and its "API key" is a
//! device key from mastomini's admin page (Remote accounts). It never posts.
//! Feeds are fetched one at a time; a run stops after its time budget and
//! the rest wait for the next night. Every reply from another server is
//! untrusted: sizes are bounded, only public `https://` addresses are
//! fetched, and only plain text and re-encoded JPEG previews go on.

use crate::bot::{Bot, BotInfo, Run, RunError};
use crate::mastodon::{HttpRequest, HttpResponse};
use crate::rss;
use crate::schedule::Schedule;
use crate::settings::{schedule_from, schedule_settings, Kind, Setting, Settings};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

pub struct Feeds;

/// A feed, or an image before it is shrunk, at most.
const FEED_LIMIT: usize = 256 * 1024;
const IMAGE_LIMIT: usize = 256 * 1024;
const JOBS_LIMIT: usize = 64 * 1024;
const USER_AGENT: &str = "mastomini feeds (household reader)";

impl Bot for Feeds {
    fn info(&self) -> BotInfo {
        BotInfo {
            id: "feeds",
            name: "Feeds from other servers",
            description: "Each night, fetches the posts of accounts household members follow on other servers (by RSS) and hands them to mastomini, where they appear in home timelines. Use mastomini's address as the server and a device key from its admin page (Remote accounts) as the API key.",
            schedule: Schedule::Manual,
            // A night's fetch is still useful in the early morning.
            grace_minutes: 180,
            default_instance: "https://mastomini.local",
            uses_llm: false,
            needs_mastodon: true,
        }
    }

    fn settings(&self) -> Vec<Setting> {
        let mut s = schedule_settings("daily", "03:00", "us_eastern", "1440");
        s.push(
            Setting::new("images", "Fetch images", Kind::Toggle, "yes")
                .help("Small previews, only when mastomini has an SD card. Posts marked adult never get images."),
        );
        s.push(Setting::new(
            "max_images",
            "Most images per night",
            Kind::Number { min: 0, max: 400 },
            "120",
        ));
        s.push(
            Setting::new(
                "minutes",
                "Longest run, in minutes",
                Kind::Number { min: 1, max: 120 },
                "30",
            )
            .help("Feeds not reached wait for the next night."),
        );
        s
    }

    fn schedule(&self, settings: &Settings) -> Schedule {
        schedule_from(settings)
    }

    fn check(&self, run: &mut Run<'_>) -> Result<String, RunError> {
        let jobs = jobs(run)?;
        let count = jobs["jobs"].as_array().map_or(0, Vec::len);
        let images = if jobs["images"] == true {
            "with images"
        } else {
            "without images (no SD card)"
        };
        Ok(format!(
            "The device key works: {count} feeds to fetch, {images}"
        ))
    }

    fn run(&self, run: &mut Run<'_>) -> Result<String, RunError> {
        let started = Instant::now();
        let budget = Duration::from_secs(run.settings.number("minutes").clamp(1, 120) as u64 * 60);
        let jobs = jobs(run)?;
        let images_on = jobs["images"] == true && run.settings.yes("images");
        let mut images_left = if images_on {
            run.settings.number("max_images").clamp(0, 400) as usize
        } else {
            0
        };
        let limits = Limits {
            max_items: jobs["max_items"].as_u64().unwrap_or(20) as usize,
            max_text: jobs["max_text"].as_u64().unwrap_or(2000) as usize,
            max_images: jobs["max_images"].as_u64().unwrap_or(4) as usize,
            max_px: jobs["image"]["max_px"].as_u64().unwrap_or(640).min(2048) as u16,
            max_bytes: jobs["image"]["max_bytes"].as_u64().unwrap_or(128 * 1024) as usize,
        };
        let list = jobs["jobs"].as_array().cloned().unwrap_or_default();
        // Start where the last run stopped, so every feed gets its turn.
        let start = run
            .state
            .get("next")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
            % list.len().max(1);
        let mut tally = Tally::default();
        for n in 0..list.len() {
            if started.elapsed() > budget {
                run.state
                    .insert("next".into(), ((start + n) % list.len()).to_string());
                tally.unreached = list.len() - n;
                break;
            }
            let job = &list[(start + n) % list.len()];
            match fetch_one(run, job, &limits, &mut images_left) {
                Ok(done) => {
                    tally.feeds += 1;
                    tally.posts += done.posts;
                    tally.images += done.images;
                }
                Err(e) if e.retryable => {
                    run.log(format!(
                        "{}: {}",
                        job["url"].as_str().unwrap_or("?"),
                        e.message
                    ));
                    tally.failed += 1;
                }
                // mastomini refused the key: no other feed will do better.
                Err(e) => return Err(e),
            }
        }
        if tally.unreached == 0 {
            run.state.remove("next");
        }
        Ok(tally.summary())
    }
}

#[derive(Default)]
struct Tally {
    feeds: usize,
    posts: usize,
    images: usize,
    failed: usize,
    unreached: usize,
}

impl Tally {
    fn summary(&self) -> String {
        let mut s = format!(
            "{} feeds: {} new posts, {} images",
            self.feeds, self.posts, self.images
        );
        if self.failed > 0 {
            s.push_str(&format!("; {} failed", self.failed));
        }
        if self.unreached > 0 {
            s.push_str(&format!("; {} left for next time", self.unreached));
        }
        s
    }
}

struct Limits {
    max_items: usize,
    max_text: usize,
    max_images: usize,
    max_px: u16,
    max_bytes: usize,
}

struct Done {
    posts: usize,
    images: usize,
}

fn transport(message: String) -> RunError {
    RunError {
        message,
        retryable: true,
    }
}

/// A request to mastomini with the device key.
fn household(
    run: &mut Run<'_>,
    method: &'static str,
    path: &str,
    content_type: &str,
    body: Vec<u8>,
    limit: usize,
) -> Result<HttpResponse, RunError> {
    let mut headers = vec![
        ("Authorization".to_string(), format!("Bearer {}", run.token)),
        ("Accept".to_string(), "application/json".to_string()),
    ];
    if !body.is_empty() {
        headers.push(("Content-Type".to_string(), content_type.to_string()));
    }
    let req = HttpRequest {
        method,
        url: format!("{}{path}", run.instance),
        headers,
        body,
    };
    let response = run
        .http
        .send_bounded(&req, limit)
        .map_err(|e| transport(format!("mastomini did not answer: {e}")))?;
    match response.status {
        401 | 403 => Err(RunError::permanent(
            "mastomini refused the device key: make a new one on its admin page (Remote accounts)",
        )),
        _ => Ok(response),
    }
}

fn jobs(run: &mut Run<'_>) -> Result<Value, RunError> {
    let r = household(
        run,
        "GET",
        "/api/mastomini/v1/feeds/jobs",
        "",
        Vec::new(),
        JOBS_LIMIT,
    )?;
    if r.status == 404 {
        return Err(RunError::permanent(
            "This server has no feeds API: is it a mastomini with remote accounts?",
        ));
    }
    if r.status != 200 {
        return Err(transport(format!("mastomini answered {}", r.status)));
    }
    serde_json::from_slice(&r.body)
        .map_err(|_| transport("mastomini's answer was not understood".into()))
}

/// An address on a public server: `https://`, not a household name, not
/// an IP address. Feeds and images come only from those.
pub fn public_https(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host.split(':').next().unwrap_or("").to_ascii_lowercase();
    !host.is_empty()
        && host.contains('.')
        && !host.starts_with('[')
        && host.parse::<std::net::IpAddr>().is_err()
        && !crate::household_trust::household_host(&host)
        && !host.ends_with(".local")
        && !host.ends_with(".internal")
        && url.bytes().all(|b| b.is_ascii_graphic())
}

fn get(
    run: &mut Run<'_>,
    url: &str,
    accept: &str,
    extra: Vec<(String, String)>,
    limit: usize,
) -> Result<HttpResponse, String> {
    if !public_https(url) {
        return Err("not a public https address".into());
    }
    let mut headers = vec![
        ("Accept".into(), accept.into()),
        ("User-Agent".into(), USER_AGENT.into()),
    ];
    headers.extend(extra);
    let req = HttpRequest {
        method: "GET",
        url: url.to_string(),
        headers,
        body: Vec::new(),
    };
    run.http.send_bounded(&req, limit)
}

/// Stands in for an ETag when a server sends no validators: an unchanged
/// body is then still not parsed or handed in again (but is downloaded).
const HASH_PREFIX: &str = "sha:";

fn body_hash(body: &[u8]) -> String {
    let hex: String = Sha256::digest(body)[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("{HASH_PREFIX}{hex}")
}

/// A conditional GET from what the last fetch saw: the server's own ETag
/// and Last-Modified (a body hash is ours, never sent).
fn validators(job: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    match job["etag"].as_str() {
        Some(etag) if !etag.is_empty() && !etag.starts_with(HASH_PREFIX) => {
            out.push(("If-None-Match".into(), etag.into()))
        }
        _ => {}
    }
    if let Some(at) = job["last_modified"].as_str().filter(|v| !v.is_empty()) {
        out.push(("If-Modified-Since".into(), at.into()));
    }
    out
}

fn clip(text: &str, max: usize) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if out.len() + c.len_utf8() > max {
            break;
        }
        out.push(c);
    }
    out
}

/// Mastodon serves a small version of every attachment next to the
/// original: try it first, it is usually already preview-sized.
fn image_candidates(url: &str) -> Vec<String> {
    let mut out = Vec::new();
    if url.contains("/media_attachments/") || url.contains("/accounts/avatars/") {
        if let Some(small) = url
            .contains("/original/")
            .then(|| url.replacen("/original/", "/small/", 1))
        {
            out.push(small);
        }
    }
    out.push(url.to_string());
    out
}

/// Fetch an image and make its preview; `None` when it cannot be done.
fn preview_of(run: &mut Run<'_>, url: &str, limits: &Limits) -> Option<Vec<u8>> {
    for candidate in image_candidates(url) {
        let Ok(r) = get(
            run,
            &candidate,
            "image/jpeg, image/png;q=0.9",
            Vec::new(),
            IMAGE_LIMIT,
        ) else {
            continue;
        };
        if r.status != 200 {
            continue;
        }
        // A stated type that is not one we can shrink: no need to try.
        if r.header("content-type")
            .is_some_and(|t| !t.starts_with("image/jpeg") && !t.starts_with("image/png"))
        {
            return None;
        }
        match crate::preview::preview(&r.body, limits.max_px, limits.max_bytes) {
            Ok(jpeg) => return Some(jpeg),
            Err(e) => {
                run.log(format!("image skipped: {e}"));
                return None;
            }
        }
    }
    None
}

fn upload(run: &mut Run<'_>, account: &str, jpeg: Vec<u8>) -> Result<Option<String>, RunError> {
    let r = household(
        run,
        "POST",
        &format!("/api/mastomini/v1/feeds/{account}/media"),
        "image/jpeg",
        jpeg,
        4096,
    )?;
    if r.status != 200 {
        return Ok(None);
    }
    Ok(serde_json::from_slice::<Value>(&r.body)
        .ok()
        .and_then(|v| v["id"].as_str().map(str::to_string)))
}

fn fetch_one(
    run: &mut Run<'_>,
    job: &Value,
    limits: &Limits,
    images_left: &mut usize,
) -> Result<Done, RunError> {
    let account = job["account"].as_str().unwrap_or("").to_string();
    let url = job["url"].as_str().unwrap_or("").to_string();
    let newest = job["newest_ms"].as_u64().unwrap_or(0);
    let mut done = Done {
        posts: 0,
        images: 0,
    };
    let report = |run: &mut Run<'_>, body: Value| -> Result<Option<u64>, RunError> {
        let r = household(
            run,
            "POST",
            &format!("/api/mastomini/v1/feeds/{account}/result"),
            "application/json",
            body.to_string().into_bytes(),
            4096,
        )?;
        match r.status {
            200 => Ok(serde_json::from_slice::<Value>(&r.body)
                .ok()
                .and_then(|v| v["added"].as_u64())),
            // Unfollowed meanwhile.
            410 => Ok(None),
            status => Err(transport(format!("mastomini answered {status}"))),
        }
    };
    let response = match get(
        run,
        &url,
        "application/rss+xml, application/atom+xml, application/xml;q=0.9, */*;q=0.1",
        validators(job),
        FEED_LIMIT,
    ) {
        Ok(r) => r,
        Err(e) => {
            report(
                run,
                json!({"outcome": "failed", "error": "No answer from the server"}),
            )?;
            return Err(transport(e));
        }
    };
    match response.status {
        200 => {}
        // Nothing new since last night: the server said so itself.
        304 => {
            report(run, json!({"outcome": "not_modified"}))?;
            return Ok(done);
        }
        404 | 410 => {
            report(run, json!({"outcome": "gone"}))?;
            return Ok(done);
        }
        status => {
            report(
                run,
                json!({"outcome": "failed", "error": format!("The server answered {status}")}),
            )?;
            return Err(transport(format!("{url} answered {status}")));
        }
    }
    // The server's validators, else a hash of the body.
    let etag = response
        .header("etag")
        .map_or_else(|| body_hash(&response.body), str::to_string);
    let last_modified = response.header("last-modified").unwrap_or("").to_string();
    if job["etag"].as_str() == Some(body_hash(&response.body).as_str()) {
        report(run, json!({"outcome": "not_modified"}))?;
        return Ok(done);
    }
    let feed = match rss::parse(&response.body) {
        Ok(feed) => feed,
        Err(e) => {
            report(
                run,
                json!({"outcome": "failed", "error": "The feed could not be read"}),
            )?;
            return Err(transport(format!("{url}: {e}")));
        }
    };
    drop(response);

    let mut avatar = None;
    if job["wants_avatar"] == true && *images_left > 0 {
        if let Some(image) = feed.image.clone().filter(|u| public_https(u)) {
            if let Some(jpeg) = preview_of(run, &image, limits) {
                avatar = upload(run, &account, jpeg)?;
                *images_left -= 1;
            }
        }
    }
    let mut items = Vec::new();
    for item in feed.items.iter().take(limits.max_items) {
        let guid = if item.guid.is_empty() {
            item.link.clone()
        } else {
            item.guid.clone()
        };
        let link = [&item.link, &item.guid]
            .into_iter()
            .find(|u| public_https(u))
            .cloned();
        let (Some(link), false) = (link, guid.is_empty()) else {
            continue;
        };
        let (mut text, cw) = rss::post_text(&item.html);
        if text.is_empty() && !item.title.is_empty() {
            text = rss::html_to_text(&item.title);
        }
        let fresh = item.published_ms == 0 || item.published_ms > newest;
        let mut media = Vec::new();
        if fresh && !item.adult {
            for image in item.images.iter().take(limits.max_images) {
                if *images_left == 0 {
                    break;
                }
                let Some(jpeg) = preview_of(run, &image.url, limits) else {
                    continue;
                };
                *images_left -= 1;
                if let Some(id) = upload(run, &account, jpeg)? {
                    done.images += 1;
                    media.push(json!({"id": id, "description": clip(&image.description, 400)}));
                }
            }
        }
        items.push(json!({
            "guid": clip(&guid, 512),
            "published_ms": item.published_ms,
            "url": link,
            "text": clip(&text, limits.max_text),
            "cw": clip(&cw, 200),
            "sensitive": item.adult || !cw.is_empty(),
            "media": media,
        }));
    }
    let note = rss::html_to_text(&feed.description);
    let added = report(
        run,
        json!({
            "outcome": "ok",
            "etag": etag,
            "last_modified": last_modified,
            "title": clip(&rss::html_to_text(&feed.title), 100),
            // Mastodon's feed description is a stock line, not the bio.
            "note": (!note.starts_with("Public posts from")).then(|| clip(&note, 500)),
            "avatar": avatar,
            "items": items,
        }),
    )?;
    done.posts = added.unwrap_or(0) as usize;
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_https_addresses() {
        assert!(public_https("https://files.example.org/a.jpg"));
        for bad in [
            "http://example.org/a",
            "https://mastomini.local/a",
            "https://192.168.1.10/a",
            "https://[::1]/a",
            "https://localhost/a",
            "https://router.lan/x",
            "https://example.org/a b",
            "file:///etc/passwd",
        ] {
            assert!(!public_https(bad), "{bad}");
        }
        assert_eq!(
            image_candidates("https://f.example/media_attachments/files/1/original/a.jpg"),
            [
                "https://f.example/media_attachments/files/1/small/a.jpg",
                "https://f.example/media_attachments/files/1/original/a.jpg"
            ]
        );
        assert_eq!(image_candidates("https://blog.example/a.jpg").len(), 1);
    }

    /// A night against a real mastomini, in process: `--features e2e`.
    #[cfg(feature = "e2e")]
    mod night {
        use super::*;
        use crate::mastodon::HttpClient;
        use mastomini::domain::remote::RemoteProfile;
        use mastomini::domain::{Config, Service};
        use mastomini::store::mem::MemStore;

        const NOW: u64 = 1_793_000_000_000;
        const ETAG: &str = r#"W/"f1""#;

        fn jpeg(w: u16, h: u16) -> Vec<u8> {
            let rgb = vec![120u8; usize::from(w) * usize::from(h) * 3];
            let mut out = Vec::new();
            jpeg_encoder::Encoder::new(&mut out, 90)
                .encode(&rgb, w, h, jpeg_encoder::ColorType::Rgb)
                .unwrap();
            out
        }

        const FEED: &str = r#"<?xml version="1.0"?>
    <rss version="2.0" xmlns:media="http://search.yahoo.com/mrss/"><channel>
    <title>Bob B</title><description>Public posts from @bob@example.org</description>
    <image><url>https://files.example.org/accounts/avatars/1/original/a.png</url></image>
    <item><guid>https://example.org/@bob/2</guid><link>https://example.org/@bob/2</link>
    <pubDate>Wed, 21 Oct 2026 10:00:00 +0000</pubDate><description>&lt;p&gt;A cat&lt;/p&gt;</description>
    <media:content url="https://files.example.org/media_attachments/files/1/original/c.jpg" type="image/jpeg" medium="image">
    <media:description type="plain">Grey cat</media:description></media:content></item>
    <item><guid>https://example.org/@bob/1</guid><link>https://example.org/@bob/1</link>
    <pubDate>Tue, 20 Oct 2026 10:00:00 +0000</pubDate><description>&lt;p&gt;Grown-up&lt;/p&gt;</description>
    <media:content url="https://files.example.org/media_attachments/files/2/original/d.jpg" type="image/jpeg" medium="image">
    <media:rating scheme="urn:simple">adult</media:rating></media:content></item>
    <item><guid>local</guid><link>http://192.168.1.1/x</link><description>no public link</description></item>
    </channel></rss>"#;

        /// mastomini in-process at https://mm.test; example.org serves the feed
        /// and its images. Anything else is an error, so a stray fetch shows.
        struct World {
            mm: Service<MemStore>,
            fetched: Vec<String>,
            /// If-None-Match on each feed request.
            conditions: Vec<Option<String>>,
        }

        impl HttpClient for World {
            fn send(&mut self, req: &HttpRequest) -> Result<HttpResponse, String> {
                self.fetched.push(req.url.clone());
                if let Some(path) = req.url.strip_prefix("https://mm.test") {
                    let content_type = req
                        .headers
                        .iter()
                        .find(|(k, _)| k == "Content-Type")
                        .map_or("application/json", |(_, v)| v.as_str());
                    let mut request = mastomini::http::Request::new(req.method, path)
                        .with_body(content_type, req.body.clone());
                    request.headers = req.headers.clone();
                    let r = mastomini::api::handle(
                        &mut self.mm,
                        &mastomini::api::Ctx::new("https://mm.test"),
                        &request,
                        Some(NOW),
                    );
                    return Ok(HttpResponse {
                        headers: Vec::new(),
                        status: r.status,
                        body: r.body,
                    });
                }
                if req.url == "https://example.org/@bob.rss" {
                    // A server with validators: 304 when the ETag matches.
                    let condition = req
                        .headers
                        .iter()
                        .find(|(k, _)| k == "If-None-Match")
                        .map(|(_, v)| v.clone());
                    self.conditions.push(condition.clone());
                    let mut r = if condition.as_deref() == Some(ETAG) {
                        HttpResponse::new(304, Vec::new())
                    } else {
                        HttpResponse::new(200, FEED.as_bytes().to_vec())
                    };
                    r.keep_header("ETag", ETAG);
                    r.keep_header("Last-Modified", "Wed, 21 Oct 2026 10:00:00 GMT");
                    return Ok(r);
                }
                let (status, body) = match req.url.as_str() {
                    "https://files.example.org/media_attachments/files/1/small/c.jpg" => {
                        (200, jpeg(1200, 800))
                    }
                    "https://files.example.org/accounts/avatars/1/small/a.png" => (404, Vec::new()),
                    "https://files.example.org/accounts/avatars/1/original/a.png" => {
                        (200, jpeg(400, 400))
                    }
                    other => return Err(format!("unexpected fetch {other}")),
                };
                Ok(HttpResponse::new(status, body))
            }
        }

        #[test]
        fn a_night_fetch_reaches_the_household_board() {
            let mut mm = Service::open(MemStore::default(), Config::default()).unwrap();
            let provision = mastomini::http::Request::new("POST", "/api/mastomini/v1/provision")
                .with_body(
                    "application/x-www-form-urlencoded",
                    "username=alice&password=alicepw&title=Home",
                );
            let ctx = mastomini::api::Ctx::new("https://mm.test");
            assert_eq!(
                mastomini::api::handle(&mut mm, &ctx, &provision, Some(NOW)).status,
                200
            );
            let alice = mm.state.account_by_username("alice").unwrap().slot;
            let bob = mm
                .remote_seen(
                    RemoteProfile {
                        acct: "bob@example.org".into(),
                        display_name: String::new(),
                        note: String::new(),
                    },
                    NOW,
                )
                .unwrap();
            mm.remote_follow(alice, bob, true, NOW).unwrap();
            let (_, key) = mm.create_device_key(alice, "Bots board", NOW).unwrap();
            let media = tempfile::tempdir().unwrap();
            mm.set_media(Some(
                mastomini::media::MediaDir::open(media.path()).unwrap(),
            ));

            let mut world = World {
                mm,
                fetched: Vec::new(),
                conditions: Vec::new(),
            };
            let settings = Settings::new(Feeds.settings(), Default::default());
            let summary = {
                let mut run = Run::new(
                    "feeds",
                    NOW,
                    NOW,
                    &settings,
                    &mut world,
                    "https://mm.test",
                    &key,
                    None,
                );
                Feeds.run(&mut run).unwrap()
            };
            assert_eq!(summary, "1 feeds: 2 new posts, 1 images");
            let remote = &world.mm.remote;
            let account = &remote.accounts[&bob];
            assert_eq!(account.display_name, "Bob B");
            assert!(
                account.avatar.is_some(),
                "the avatar fell back to the original"
            );
            let items: Vec<_> = remote.items.values().collect();
            assert_eq!(items.len(), 2);
            let cat = items.iter().find(|i| i.text == "A cat").unwrap();
            assert_eq!(cat.media.len(), 1);
            assert_eq!(cat.media[0].description, "Grey cat");
            assert!(cat.media[0].width <= 640 && cat.media[0].height <= 640);
            let adult = items.iter().find(|i| i.text == "Grown-up").unwrap();
            assert!(
                adult.sensitive && adult.media.is_empty(),
                "adult posts get no images"
            );
            assert!(!world.fetched.iter().any(|u| u.contains("192.168")));
            assert!(
                !world.fetched.iter().any(|u| u.contains("/original/c.jpg")),
                "the small image was enough"
            );

            // The next night the feed is unchanged: nothing new, nothing fetched twice.
            world.fetched.clear();
            let summary = {
                let mut run = Run::new(
                    "feeds",
                    NOW,
                    NOW,
                    &settings,
                    &mut world,
                    "https://mm.test",
                    &key,
                    None,
                );
                Feeds.run(&mut run).unwrap()
            };
            assert_eq!(summary, "1 feeds: 0 new posts, 0 images");
            assert_eq!(world.mm.remote.items.len(), 2);
            // The second night asked with the server's ETag and got a 304.
            assert_eq!(world.conditions, [None, Some(ETAG.to_string())]);
            let feed = &world.mm.remote.accounts[&bob].feed;
            assert_eq!(feed.last_modified, "Wed, 21 Oct 2026 10:00:00 GMT");
            assert!(feed.last_ok_ms.is_some());
            assert!(!world
                .fetched
                .iter()
                .any(|u| u.contains("files.example.org")));

            // A revoked key stops the run with a clear, permanent error.
            let id = *world.mm.remote.devices.keys().next().unwrap();
            world.mm.revoke_device_key(alice, id, NOW).unwrap();
            let mut run = Run::new(
                "feeds",
                NOW,
                NOW,
                &settings,
                &mut world,
                "https://mm.test",
                &key,
                None,
            );
            let e = Feeds.run(&mut run).unwrap_err();
            assert!(
                !e.retryable && e.message.contains("device key"),
                "{}",
                e.message
            );
        }
    }
}
