//! The admin app (`mastomini_bots_ui`), embedded and served at `/app/`.
//! `scripts/build-web.sh` builds it and generates the gzip-only asset table
//! that `--features bundled-web` includes. Exact-match lookups only.

use crate::http::{Request, Response};

pub struct Asset {
    pub path: &'static str,
    pub mime: &'static str,
    pub gzip: &'static [u8],
    pub etag: &'static str,
    /// Content-hashed names can be cached for good.
    pub immutable: bool,
}

#[cfg(feature = "bundled-web")]
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/.embuild/web/assets.rs"
));
#[cfg(not(feature = "bundled-web"))]
static ASSETS: &[Asset] = &[];

pub fn bundled() -> bool {
    !ASSETS.is_empty()
}

pub fn serve(req: &Request) -> Response {
    serve_from(ASSETS, req)
}

fn serve_from(assets: &[Asset], req: &Request) -> Response {
    let path = match req.path.as_str() {
        "/app" => return Response::redirect("/app/"),
        "/app/" => "/app/index.html",
        other => other,
    };
    // The app routes with the hash, so every page is index.html.
    let Some(asset) = assets.iter().find(|a| a.path == path) else {
        if assets.is_empty() {
            return Response::new(
                404,
                "text/plain; charset=utf-8",
                "This build does not include the admin app: run make web, then make run-web.",
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

    static FAKE: &[Asset] = &[Asset {
        path: "/app/index.html",
        mime: "text/html; charset=utf-8",
        gzip: b"<index>",
        etag: "\"idx\"",
        immutable: false,
    }];

    #[test]
    fn serves_exact_paths() {
        let r = serve_from(FAKE, &Request::new("GET", "/app/"));
        assert_eq!((r.status, r.body.as_slice()), (200, b"<index>".as_slice()));
        assert_eq!(
            serve_from(FAKE, &Request::new("GET", "/app/../Cargo.toml")).status,
            404
        );
        let r = serve_from(
            FAKE,
            &Request::new("GET", "/app/").with_header("If-None-Match", "\"idx\""),
        );
        assert_eq!(r.status, 304);
        assert_eq!(serve_from(&[], &Request::new("GET", "/app/")).status, 404);
    }
}
