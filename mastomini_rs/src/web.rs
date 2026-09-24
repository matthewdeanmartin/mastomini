//! The household app (`mastomini_ui`), embedded in the binary and served at
//! `/app/` (spec/06).
//!
//! `scripts/build-web.sh` builds the Angular app and generates the asset table
//! that `--features bundled-web` includes. Only gzip copies are stored, to
//! save flash. Lookups are exact matches against that table: no filesystem, no
//! percent-decoding, nothing to traverse. The app uses hash routing, so
//! `/app/` is its only page.

use crate::api::html;
use crate::http::{Request, Response};

pub struct Asset {
    pub path: &'static str,
    pub mime: &'static str,
    pub gzip: &'static [u8],
    pub etag: &'static str,
    /// Content-hashed file names can be cached for good.
    pub immutable: bool,
}

#[cfg(feature = "bundled-web")]
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/.embuild/web/assets.rs"
));
#[cfg(not(feature = "bundled-web"))]
static ASSETS: &[Asset] = &[];

/// Is this build carrying the household app?
pub fn bundled() -> bool {
    !ASSETS.is_empty()
}

/// Does the client accept gzip? No header means any coding is acceptable
/// (RFC 9110 §12.5.3); otherwise a `gzip` or `*` token without `q=0`.
fn accepts_gzip(header: Option<&str>) -> bool {
    let Some(header) = header else {
        return true;
    };
    header.split(',').any(|part| {
        let mut fields = part.split(';').map(str::trim);
        let name = fields.next().unwrap_or("");
        let refused = fields.any(|f| {
            f.strip_prefix("q=")
                .and_then(|q| q.trim().parse::<f32>().ok())
                .is_some_and(|q| q <= 0.0)
        });
        (name.eq_ignore_ascii_case("gzip") || name == "*") && !refused
    })
}

/// `GET /app` and everything below it.
pub fn serve(req: &Request) -> Response {
    serve_from(ASSETS, req)
}

fn serve_from(assets: &[Asset], req: &Request) -> Response {
    if req.method != "GET" && req.method != "HEAD" {
        return Response::new(405, "text/plain; charset=utf-8", "Method not allowed")
            .with_header("Allow", "GET");
    }
    let path = match req.path.as_str() {
        "/app" => return Response::redirect("/app/"),
        "/app/" => "/app/index.html",
        other => other,
    };
    let Some(asset) = assets.iter().find(|a| a.path == path) else {
        if assets.is_empty() && path == "/app/index.html" {
            return html::message(
                404,
                "Household app not included",
                "This build doesn't include the household app. Build it with make web (desktop: make run-web).",
            );
        }
        return Response::new(404, "text/plain; charset=utf-8", "Not found");
    };
    let cache = if asset.immutable {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let common = |r: Response| {
        r.with_header("ETag", asset.etag)
            .with_header("Cache-Control", cache)
            .with_header("Vary", "Accept-Encoding")
            .with_header("X-Content-Type-Options", "nosniff")
            .with_header("Referrer-Policy", "no-referrer")
    };
    if req.header("If-None-Match") == Some(asset.etag) {
        let mut r = common(Response::new(304, asset.mime, Vec::new()));
        r.headers.retain(|(k, _)| k != "Content-Type");
        return r;
    }
    if !accepts_gzip(req.header("Accept-Encoding")) {
        return Response::new(
            406,
            "text/plain; charset=utf-8",
            "The household app needs a browser that accepts gzip",
        );
    }
    let body = if req.method == "HEAD" {
        Vec::new()
    } else {
        asset.gzip.to_vec()
    };
    common(Response::new(200, asset.mime, body)).with_header("Content-Encoding", "gzip")
}

#[cfg(test)]
mod tests {
    use super::*;

    static FAKE: &[Asset] = &[
        Asset {
            path: "/app/index.html",
            mime: "text/html; charset=utf-8",
            gzip: b"<index>",
            etag: "\"idx\"",
            immutable: false,
        },
        Asset {
            path: "/app/main-ABC.js",
            mime: "text/javascript; charset=utf-8",
            gzip: b"<js>",
            etag: "\"js\"",
            immutable: true,
        },
    ];

    fn get(path: &str, accept: &str) -> Response {
        let req = Request::new("GET", path).with_header("Accept-Encoding", accept);
        serve_from(FAKE, &req)
    }

    #[test]
    fn serves_index_and_hashed_assets_gzipped() {
        assert_eq!(get("/app", "gzip").header("Location"), Some("/app/"));
        let r = get("/app/", "gzip, deflate, br");
        assert_eq!(r.status, 200);
        assert_eq!(r.body, b"<index>");
        assert_eq!(r.header("Content-Encoding"), Some("gzip"));
        assert_eq!(r.header("Cache-Control"), Some("no-cache"));
        let r = get("/app/main-ABC.js", "*");
        assert_eq!(r.body, b"<js>");
        assert!(r.header("Cache-Control").unwrap().contains("immutable"));
    }

    #[test]
    fn exact_lookups_only() {
        for path in [
            "/app/missing.js",
            "/app/../Cargo.toml",
            "/app/%2e%2e/x",
            "/app/index.html/",
        ] {
            assert_eq!(get(path, "gzip").status, 404, "{path}");
        }
        let r = serve_from(FAKE, &Request::new("POST", "/app/"));
        assert_eq!(r.status, 405);
    }

    #[test]
    fn revalidation_and_encoding() {
        let req = Request::new("GET", "/app/")
            .with_header("If-None-Match", "\"idx\"")
            .with_header("Accept-Encoding", "gzip");
        let r = serve_from(FAKE, &req);
        assert_eq!(r.status, 304);
        assert!(r.body.is_empty());
        assert_eq!(get("/app/", "identity").status, 406);
        assert_eq!(get("/app/", "gzip;q=0").status, 406);
        assert_eq!(get("/app/", "").status, 406);
        // Firefox, and a client that sends no Accept-Encoding at all.
        assert_eq!(get("/app/", "gzip, deflate, br, zstd").status, 200);
        assert_eq!(serve_from(FAKE, &Request::new("GET", "/app/")).status, 200);
    }

    #[test]
    fn a_build_without_the_app_says_so() {
        let r = serve_from(&[], &Request::new("GET", "/app/"));
        assert_eq!(r.status, 404);
        assert!(String::from_utf8_lossy(&r.body).contains("make web"));
    }
}
