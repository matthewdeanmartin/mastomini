//! OAuth endpoints as Mastodon clients use them (spec/05 "OAuth").
//!
//! The authorize page is plain server-rendered HTML with no JavaScript and no
//! cookies, so it works in any in-app browser.

use super::html::STYLE;
use super::{fail, Call, Reply};
use crate::domain::{parse_scopes, AuthCodeGrant, Error, OOB};
use crate::http::{encode, Response};
use crate::store::Store;
use crate::text::escape;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::json;

fn oauth_error(status: u16, error: &str, description: &str) -> Response {
    Response::json(
        status,
        &json!({ "error": error, "error_description": description }),
    )
}

fn redirect_uris<S: Store>(c: &Call<'_, S>) -> Vec<String> {
    c.params
        .all("redirect_uris")
        .iter()
        .flat_map(|v| v.split(['\n', ' ']))
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(str::to_string)
        .collect()
}

fn create_app<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let name = c.params.get("client_name").unwrap_or("");
    let uris = redirect_uris(c);
    let scopes = c.params.get("scopes").unwrap_or("read");
    let website = c.params.get("website").map(str::to_string);
    let (app, secret) = c
        .svc
        .register_app(name, website.as_deref(), &uris, scopes, c.now)
        .map_err(fail)?;
    Ok(Response::ok(json!({
        "id": app.id.to_string(),
        "name": app.name,
        "website": app.website,
        "scopes": app.scopes,
        "redirect_uri": app.redirect_uris.join("\n"),
        "redirect_uris": app.redirect_uris,
        "client_id": app.client_id,
        "client_secret": secret,
        "client_secret_expires_at": 0,
        "vapid_key": "",
    })))
}

fn verify_app<S: Store>(c: &Call<'_, S>) -> Reply {
    let principal = c
        .principal
        .as_ref()
        .ok_or_else(|| fail(Error::Unauthorized))?;
    let app = c
        .svc
        .state
        .apps
        .get(&principal.app_id)
        .ok_or_else(|| fail(Error::Unauthorized))?;
    Ok(Response::ok(json!({
        "id": app.id.to_string(),
        "name": app.name,
        "website": app.website,
        "scopes": app.scopes,
        "redirect_uri": app.redirect_uris.join("\n"),
        "redirect_uris": app.redirect_uris,
        "vapid_key": "",
    })))
}

fn grant<S: Store>(c: &Call<'_, S>) -> Result<(AuthCodeGrant, String), Response> {
    let p = &c.params;
    if p.get("response_type").unwrap_or("code") != "code" {
        return Err(page(
            400,
            "Unsupported response type",
            "Only response_type=code is supported.",
        ));
    }
    let challenge = p.text("code_challenge").map(str::to_string);
    if challenge.is_some() && p.get("code_challenge_method").unwrap_or("plain") != "S256" {
        return Err(page(
            400,
            "Unsupported PKCE method",
            "Only code_challenge_method=S256 is supported.",
        ));
    }
    let scopes = parse_scopes(p.get("scope").unwrap_or("read"))
        .map_err(|e| page(400, "Invalid scope", &e.to_string()))?;
    let grant = AuthCodeGrant {
        client_id: p.get("client_id").unwrap_or("").to_string(),
        redirect_uri: p.get("redirect_uri").unwrap_or("").to_string(),
        scopes,
        code_challenge: challenge,
    };
    Ok((grant, p.get("state").unwrap_or("").to_string()))
}

fn page(status: u16, title: &str, message: &str) -> Response {
    Response::html(
        status,
        format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{t}</title><style>{STYLE}</style></head><body><main><h1>{t}</h1><p>{m}</p></main></body></html>",
            t = escape(title),
            m = escape(message)
        ),
    )
}

fn authorize_page<S: Store>(
    c: &Call<'_, S>,
    grant: &AuthCodeGrant,
    state: &str,
    error: Option<&str>,
) -> Response {
    let app_name = c
        .svc
        .app_by_client_id(&grant.client_id)
        .map_or("an app", |a| a.name.as_str())
        .to_string();
    let server = &c.svc.state.server.title;
    let hidden = |name: &str, value: &str| {
        format!(
            "<input type=\"hidden\" name=\"{name}\" value=\"{}\">",
            escape(value)
        )
    };
    let mut fields = String::new();
    fields.push_str(&hidden("response_type", "code"));
    fields.push_str(&hidden("client_id", &grant.client_id));
    fields.push_str(&hidden("redirect_uri", &grant.redirect_uri));
    fields.push_str(&hidden("scope", &grant.scopes.join(" ")));
    fields.push_str(&hidden("state", state));
    if let Some(challenge) = &grant.code_challenge {
        fields.push_str(&hidden("code_challenge", challenge));
        fields.push_str(&hidden("code_challenge_method", "S256"));
    }
    let username = c.params.get("username").unwrap_or("");
    let error = error.map_or(String::new(), |e| {
        format!("<p class=\"error\">{}</p>", escape(e))
    });
    Response::html(
        if error.is_empty() { 200 } else { 401 },
        format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Sign in to {server}</title><style>{STYLE}</style></head><body><main>\
<h1>Sign in to {server}</h1><p><strong>{app}</strong> would like to use your account.</p>\
<p class=\"scopes\">Permissions: {scopes}</p>{error}\
<form method=\"post\" action=\"/oauth/authorize\">{fields}\
<label for=\"username\">Username</label><input type=\"text\" id=\"username\" name=\"username\" value=\"{username}\" autocomplete=\"username\" autocapitalize=\"none\" autocorrect=\"off\" required>\
<label for=\"password\">Password</label><input type=\"password\" id=\"password\" name=\"password\" autocomplete=\"current-password\" required>\
<div class=\"buttons\"><button class=\"deny\" name=\"decision\" value=\"deny\" formnovalidate>Deny</button><button class=\"approve\" name=\"decision\" value=\"approve\">Authorize</button></div>\
</form></main></body></html>",
            server = escape(server),
            app = escape(&app_name),
            scopes = escape(&grant.scopes.join(", ")),
            username = escape(username),
        ),
    )
}

fn with_query(uri: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = uri.to_string();
    for (k, v) in pairs.iter().filter(|(_, v)| !v.is_empty()) {
        out.push(if out.contains('?') { '&' } else { '?' });
        out.push_str(k);
        out.push('=');
        out.push_str(&encode(v));
    }
    out
}

fn authorize_get<S: Store>(c: &Call<'_, S>) -> Reply {
    let (grant, state) = grant(c)?;
    c.svc
        .check_authorize_request(&grant)
        .map_err(|e| page(400, "Authorization failed", &e.to_string()))?;
    Ok(authorize_page(c, &grant, &state, None))
}

fn authorize_post<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let (grant, state) = grant(c)?;
    c.svc
        .check_authorize_request(&grant)
        .map_err(|e| page(400, "Authorization failed", &e.to_string()))?;
    if c.params.get("decision") == Some("deny") {
        if grant.redirect_uri == OOB {
            return Ok(page(
                200,
                "Authorization denied",
                "You can close this window.",
            ));
        }
        let to = with_query(
            &grant.redirect_uri,
            &[("error", "access_denied"), ("state", &state)],
        );
        return Ok(Response::redirect(&to));
    }
    let username = c.params.get("username").unwrap_or("").to_string();
    let password = c.params.get("password").unwrap_or("").to_string();
    let code = match c.svc.authorize(&grant, &username, &password, c.now) {
        Ok(code) => code,
        Err(Error::Unauthorized) => {
            return Ok(authorize_page(
                c,
                &grant,
                &state,
                Some("Wrong username or password."),
            ))
        }
        Err(e) => return Ok(authorize_page(c, &grant, &state, Some(&e.to_string()))),
    };
    if grant.redirect_uri == OOB {
        return Ok(Response::html(
            200,
            format!(
                "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Authorization code</title><style>{STYLE}</style></head><body><main><h1>Authorization code</h1><p>Copy this code into the app:</p><p><code id=\"code\">{}</code></p></main></body></html>",
                escape(&code)
            ),
        ));
    }
    Ok(Response::redirect(&with_query(
        &grant.redirect_uri,
        &[("code", &code), ("state", &state)],
    )))
}

/// Client credentials from the body, or from HTTP Basic auth.
fn client<S: Store>(c: &Call<'_, S>) -> (String, Option<String>) {
    let basic = c
        .req
        .header("Authorization")
        .and_then(|h| {
            h.strip_prefix("Basic ")
                .or_else(|| h.strip_prefix("basic "))
        })
        .and_then(|b| STANDARD.decode(b.trim()).ok())
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| {
            s.split_once(':')
                .map(|(id, secret)| (id.to_string(), secret.to_string()))
        });
    if let Some((id, secret)) = basic {
        return (id, Some(secret));
    }
    (
        c.params.get("client_id").unwrap_or("").to_string(),
        c.params.text("client_secret").map(str::to_string),
    )
}

fn token<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let (client_id, client_secret) = client(c);
    let issued = match c.params.get("grant_type") {
        Some("authorization_code") => {
            let code = c.params.get("code").unwrap_or("").to_string();
            let redirect_uri = c.params.get("redirect_uri").unwrap_or("").to_string();
            let verifier = c.params.text("code_verifier").map(str::to_string);
            c.svc.exchange_code(
                &client_id,
                client_secret.as_deref(),
                &code,
                &redirect_uri,
                verifier.as_deref(),
                c.now,
            )
        }
        Some("client_credentials") => {
            let scopes = c.params.get("scope").unwrap_or("read").to_string();
            c.svc
                .app_token(&client_id, client_secret.as_deref().unwrap_or(""), &scopes)
        }
        Some(other) => {
            return Err(oauth_error(
                400,
                "unsupported_grant_type",
                &format!("The authorization grant type {other} is not supported."),
            ))
        }
        None => {
            return Err(oauth_error(
                400,
                "invalid_request",
                "grant_type is required.",
            ))
        }
    };
    match issued {
        Ok((token, scopes)) => Ok(Response::ok(json!({
            "access_token": token,
            "token_type": "Bearer",
            "scope": scopes.join(" "),
            "created_at": c.now / 1000,
        }))),
        Err(Error::Unauthorized) => Err(oauth_error(
            401,
            "invalid_client",
            "Client authentication failed due to unknown client, no client authentication included, or unsupported authentication method.",
        )),
        Err(Error::Invalid(_)) => Err(oauth_error(
            400,
            "invalid_grant",
            "The provided authorization grant is invalid, expired, revoked, does not match the redirection URI used in the authorization request, or was issued to another client.",
        )),
        Err(e) => Err(fail(e)),
    }
}

fn revoke<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let token = c.params.get("token").unwrap_or("").to_string();
    c.svc.revoke(&token).map_err(fail)?;
    Ok(Response::ok(json!({})))
}

pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    Some(match (method, seg) {
        ("POST", ["api", "v1", "apps"]) => create_app(c),
        ("GET", ["api", "v1", "apps", "verify_credentials"]) => verify_app(c),
        ("GET", ["oauth", "authorize"]) => authorize_get(c),
        ("POST", ["oauth", "authorize"]) => authorize_post(c),
        ("POST", ["oauth", "token"]) => token(c),
        ("POST", ["oauth", "revoke"]) => revoke(c),
        _ => return None,
    })
}
