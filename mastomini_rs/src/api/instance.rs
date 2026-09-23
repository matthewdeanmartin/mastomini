//! Instance metadata, nodeinfo and OAuth server metadata.

use super::{entities, Call, Reply};
use crate::domain::{KNOWN_SCOPES, MAX_PINS};
use crate::http::Response;
use crate::store::Store;
use crate::text::{MAX_CHARS, URL_CHARS};
use serde_json::{json, Value};

pub const VERSION: &str = concat!(
    "4.3.0 (compatible; mastomini ",
    env!("CARGO_PKG_VERSION"),
    ")"
);

fn statuses_config() -> Value {
    json!({
        "max_characters": MAX_CHARS,
        "max_media_attachments": 0,
        "characters_reserved_per_url": URL_CHARS,
    })
}

fn media_config() -> Value {
    json!({
        "supported_mime_types": [],
        "image_size_limit": 0,
        "image_matrix_limit": 0,
        "video_size_limit": 0,
        "video_frame_rate_limit": 0,
        "video_matrix_limit": 0,
    })
}

fn polls_config() -> Value {
    json!({
        "max_options": 0,
        "max_characters_per_option": 0,
        "min_expiration": 300,
        "max_expiration": 604800,
    })
}

fn accounts_config() -> Value {
    json!({ "max_featured_tags": 0, "max_pinned_statuses": MAX_PINS })
}

fn rules<S: Store>(c: &Call<'_, S>) -> Value {
    Value::Array(
        c.svc
            .state
            .server
            .rules
            .iter()
            .enumerate()
            .map(|(i, text)| json!({ "id": (i + 1).to_string(), "text": text, "hint": "" }))
            .collect(),
    )
}

fn contact<S: Store>(c: &Call<'_, S>) -> Value {
    c.svc
        .state
        .active_accounts()
        .find(|a| a.rec.role == crate::domain::records::Role::Owner)
        .map_or(Value::Null, |a| entities::account(c.svc, c.ctx, a))
}

fn v1<S: Store>(c: &Call<'_, S>) -> Value {
    let server = &c.svc.state.server;
    json!({
        "uri": c.ctx.host(),
        "title": server.title,
        "short_description": server.description,
        "description": server.description,
        "email": "",
        "version": VERSION,
        "urls": { "streaming_api": null },
        "stats": {
            "user_count": c.svc.state.active_accounts().count(),
            "status_count": c.svc.state.statuses.len(),
            "domain_count": 0,
        },
        "thumbnail": null,
        "languages": ["en"],
        "registrations": false,
        "approval_required": false,
        "invites_enabled": false,
        "configuration": {
            "accounts": accounts_config(),
            "statuses": statuses_config(),
            "media_attachments": media_config(),
            "polls": polls_config(),
        },
        "contact_account": contact(c),
        "rules": rules(c),
    })
}

fn v2<S: Store>(c: &Call<'_, S>) -> Value {
    let server = &c.svc.state.server;
    json!({
        "domain": c.ctx.host(),
        "title": server.title,
        "version": VERSION,
        "source_url": "https://github.com/matthewdeanmartin/mastomini",
        "description": server.description,
        "usage": { "users": { "active_month": c.svc.state.active_accounts().count() } },
        "thumbnail": { "url": entities::header_url(c.ctx) },
        "icon": [],
        "languages": ["en"],
        "configuration": {
            "urls": { "streaming": null, "status": null },
            "vapid": { "public_key": "" },
            "accounts": accounts_config(),
            "statuses": statuses_config(),
            "media_attachments": media_config(),
            "polls": polls_config(),
            "translation": { "enabled": false },
        },
        "registrations": { "enabled": false, "approval_required": false, "message": null, "url": null },
        "api_versions": { "mastodon": 2 },
        "contact": { "email": "", "account": contact(c) },
        "rules": rules(c),
    })
}

fn oauth_metadata<S: Store>(c: &Call<'_, S>) -> Value {
    let base = &c.ctx.base_url;
    json!({
        "issuer": format!("{base}/"),
        "service_documentation": "https://docs.joinmastodon.org/",
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "revocation_endpoint": format!("{base}/oauth/revoke"),
        "app_registration_endpoint": format!("{base}/api/v1/apps"),
        "scopes_supported": KNOWN_SCOPES,
        "response_types_supported": ["code"],
        "response_modes_supported": ["query", "fragment", "form_post"],
        "code_challenge_methods_supported": ["S256"],
        "grant_types_supported": ["authorization_code", "client_credentials"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post", "none"],
    })
}

pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    if method != "GET" {
        return None;
    }
    let body = match seg {
        ["api", "v1", "instance"] => v1(c),
        ["api", "v2", "instance"] => v2(c),
        ["api", "v1", "instance", "rules"] => rules(c),
        ["api", "v1", "instance", "peers" | "activity" | "domain_blocks" | "languages"]
        | ["api", "v1", "custom_emojis"] => json!([]),
        ["api", "v1", "instance", "extended_description"] => json!({
            "updated_at": null,
            "content": crate::text::render_plain(&c.svc.state.server.description),
        }),
        [".well-known", "oauth-authorization-server"] => oauth_metadata(c),
        [".well-known", "nodeinfo"] => json!({
            "links": [{
                "rel": "http://nodeinfo.diaspora.software/ns/schema/2.0",
                "href": format!("{}/nodeinfo/2.0", c.ctx.base_url),
            }]
        }),
        ["nodeinfo", "2.0"] => json!({
            "version": "2.0",
            "software": { "name": "mastodon", "version": VERSION },
            "protocols": ["activitypub"],
            "services": { "outbound": [], "inbound": [] },
            "usage": {
                "users": { "total": c.svc.state.active_accounts().count() },
                "localPosts": c.svc.state.statuses.len(),
            },
            "openRegistrations": false,
            "metadata": {},
        }),
        _ => return None,
    };
    Some(Ok(Response::ok(body)))
}
