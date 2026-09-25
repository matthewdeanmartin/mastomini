//! `/trust`, `/ca` and `/ca.pem`: how a device comes to trust the board's
//! HTTPS (docs/security/https.md). Public, served on HTTP and HTTPS alike,
//! and plain server-rendered HTML: a device that doesn't trust the board yet
//! can only reach it over HTTP, maybe without the household app.

use super::html::{self, page};
use super::{Call, Reply};
use crate::http::Response;
use crate::store::Store;
use crate::text::escape;
use crate::tls::Tls;

/// Per-platform steps: (label, steps). Kept in step with the household
/// app's Trust page (mastomini_ui/src/app/pages/trust.ts).
const STEPS: [(&str, &[&str]); 6] = [
    (
        "iPhone / iPad",
        &[
            "Open this page in Safari and tap <strong>Download the certificate</strong>, then <strong>Allow</strong>.",
            "Settings → General → VPN &amp; Device Management → the mastomini profile → <strong>Install</strong>.",
            "Settings → General → About → Certificate Trust Settings → turn on <strong>full trust</strong> for it. Installing alone isn't enough.",
        ],
    ),
    (
        "Android",
        &[
            "Download the certificate.",
            "Settings → Security → Encryption &amp; credentials → Install a certificate → <strong>CA certificate</strong> (names vary by phone).",
            "Chrome then trusts the board. Most Android apps (Tusky, the official app) ignore certificates you install, so they need the real-domain setup instead.",
        ],
    ),
    (
        "Mac",
        &[
            "Download the certificate and open it: Keychain Access adds it.",
            "Double-click it in Keychain Access → Trust → <strong>When using this certificate: Always Trust</strong>.",
        ],
    ),
    (
        "Windows",
        &[
            "Download the certificate and open it → Install Certificate → Current User.",
            "Choose <strong>Place all certificates in the following store</strong> → Trusted Root Certification Authorities.",
        ],
    ),
    (
        "Linux",
        &[
            "Download the <a href=\"/ca.pem\">PEM version</a>, copy it to <code>/usr/local/share/ca-certificates/mastomini.crt</code> and run <code>sudo update-ca-certificates</code>.",
        ],
    ),
    (
        "Firefox (any computer)",
        &[
            "Firefox keeps its own list: Settings → Privacy &amp; Security → Certificates → View Certificates → Authorities → Import, then tick <strong>Trust this CA to identify websites</strong>.",
        ],
    ),
];

/// Security headers for these pages: nothing but inline style, no framing.
fn harden(r: Response) -> Response {
    r.with_header("X-Content-Type-Options", "nosniff")
        .with_header("Referrer-Policy", "no-referrer")
        .with_header(
            "Content-Security-Policy",
            "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        )
}

fn download(body: Vec<u8>, mime: &str, file: &str) -> Response {
    Response::new(200, mime, body)
        .with_header("Cache-Control", "no-store")
        .with_header(
            "Content-Disposition",
            &format!("attachment; filename=\"{file}\""),
        )
}

fn no_https() -> Response {
    html::message(
        404,
        "No HTTPS on this server",
        "This server uses plain HTTP only, so there is no certificate to install. The owner turns HTTPS on by building the firmware with a household certificate (make certs).",
    )
}

fn trust_page(tls: &Tls, secure: bool) -> Response {
    let steps: String = STEPS
        .iter()
        .map(|(label, steps)| {
            let items: String = steps.iter().map(|s| format!("<li>{s}</li>")).collect();
            format!("<details><summary>{label}</summary><ol>{items}</ol></details>")
        })
        .collect();
    let scope = if tls.name_constrained() {
        "It can only vouch for household names (<code>.local</code>, <code>.lan</code>, <code>home.arpa</code>) and private addresses, never for public websites such as your bank."
    } else {
        "Only install it if you trust whoever runs this server: a device that trusts it believes any certificate it signs."
    };
    let here = if secure {
        "<p class=\"ok\">This page came over HTTPS, so this device already trusts the board.</p>"
            .to_string()
    } else {
        format!(
            "<p class=\"hint\">When you're done, use <a href=\"{url}/\">{url}</a>. If that shows a warning, the certificate isn't trusted yet: don't click past it.</p>",
            url = escape(&tls.https_url)
        )
    };
    let names = tls
        .names
        .iter()
        .map(|n| format!("<code>{}</code>", escape(n)))
        .collect::<Vec<_>>()
        .join(", ");
    let body = format!(
        "<p>Install the household certificate once on each device. Then browsers and apps can check they are talking to this board, and nobody else on the Wi-Fi can read or change what you send.</p>\
<div class=\"buttons\"><a class=\"button\" href=\"/ca\">Download the certificate</a></div>\
<h2>Check it's the right one</h2>\
<p class=\"hint\">Before trusting it, compare its SHA-256 fingerprint with the one the owner has (printed by <code>make certs</code>, and on the household app's Security page over HTTPS). A page fetched over plain HTTP can't prove this by itself.</p>\
<p><code>{fingerprint}</code></p>\
<p class=\"hint\">{ca}. {scope}</p>\
<h2>Install it</h2>{steps}{here}\
<p class=\"hint\">The board's certificate is valid for {names} until {until}.</p>",
        fingerprint = escape(&tls.ca_fingerprint),
        ca = escape(&tls.ca_name),
        until = escape(tls.not_after.get(..10).unwrap_or(&tls.not_after)),
    );
    harden(page(200, "Trust this server", &body))
}

pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    if method != "GET" && method != "HEAD" {
        return Some(Ok(Response::new(
            405,
            "text/plain; charset=utf-8",
            "Method not allowed",
        )
        .with_header("Allow", "GET")));
    }
    let Some(tls) = c.ctx.tls.as_ref() else {
        return Some(Ok(no_https()));
    };
    let mut reply = match seg {
        ["ca"] => download(
            tls.ca_der.clone(),
            "application/x-x509-ca-cert",
            "mastomini-household-ca.crt",
        ),
        ["ca.pem"] => download(
            tls.ca_pem().into_bytes(),
            "application/x-pem-file",
            "mastomini-household-ca.pem",
        ),
        _ => trust_page(tls, c.req.secure),
    };
    if method == "HEAD" {
        reply.body.clear();
    }
    Some(Ok(reply))
}
