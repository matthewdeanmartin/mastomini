//! The small server-rendered pages (sign-in, setup, Wi-Fi onboarding) share
//! one look. No JavaScript is required anywhere, so they work in captive
//! portal browsers and in-app sign-in sheets.

use crate::http::Response;
use crate::text::escape;

pub const STYLE: &str = "body{font-family:system-ui,sans-serif;background:#f4f4f8;color:#222;margin:0;padding:24px}\
main{max-width:380px;margin:0 auto;background:#fff;border-radius:12px;padding:24px;box-shadow:0 1px 4px #0002}\
h1{font-size:1.3em;margin-top:0}h2{font-size:1.05em;margin:20px 0 4px}label{display:block;margin:12px 0 4px}\
input[type=text],input[type=password],select{width:100%;box-sizing:border-box;padding:10px;font-size:1em;border:1px solid #bbb;border-radius:6px}\
.scopes,.hint{color:#555;font-size:.9em}.error{color:#b00020}.ok{color:#11772d}\
.buttons{display:flex;gap:8px;margin-top:20px}button,a.button{flex:1;padding:10px;font-size:1em;border-radius:6px;border:0;cursor:pointer;text-align:center;text-decoration:none}\
button.approve,a.button{background:#563acc;color:#fff}button.deny{background:#ddd}code{font-size:1.1em;word-break:break-all}\
ol{padding-left:1.2em}li{margin:6px 0}\
@media (prefers-color-scheme:dark){body{background:#17171c;color:#eee}main{background:#24242c}.scopes,.hint{color:#aaa}input,select{background:#1b1b22;color:#eee}}";

/// A complete page. `title` is escaped; `body` is trusted HTML.
pub fn document(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{t}</title><style>{STYLE}</style></head><body><main><h1>{t}</h1>{body}</main></body></html>",
        t = escape(title)
    )
}

pub fn page(status: u16, title: &str, body: &str) -> Response {
    Response::html(status, document(title, body)).with_header("Cache-Control", "no-store")
}

/// A page with one escaped paragraph.
pub fn message(status: u16, title: &str, text: &str) -> Response {
    page(status, title, &format!("<p>{}</p>", escape(text)))
}

/// `<p class="error">` for an optional message.
pub fn error(message: Option<&str>) -> String {
    message.map_or(String::new(), |m| {
        format!("<p class=\"error\">{}</p>", escape(m))
    })
}
