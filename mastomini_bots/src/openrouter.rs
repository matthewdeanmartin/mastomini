//! OpenRouter chat completions, the way mawkingbird calls it
//! (`providers/openrouter/openrouter-chat.ts`): `POST /chat/completions`,
//! a bounded `max_tokens` as a guard against a runaway bill, errors turned
//! into something an admin can act on, and the reply treated as untrusted
//! plain text ([`crate::text::clean_completion`]).
//!
//! One key and default model for the whole device, set on the admin site's
//! Device page; a bot may choose another model.

use crate::bot::RunError;
use crate::mastodon::{HttpClient, HttpRequest};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const BASE_URL: &str = "https://openrouter.ai/api/v1";
/// Cheap, capable, and what mawkingbird defaults to.
pub const DEFAULT_MODEL: &str = "google/gemma-4-31b-it";
/// A bill guard, not a length limit: a post is a few hundred characters.
pub const MAX_TOKENS_LIMIT: u32 = 2000;

/// Stored under `i.openrouter`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Never sent to the browser.
    pub key: String,
    pub model: String,
}

impl Config {
    pub fn model(&self) -> &str {
        if self.model.is_empty() {
            DEFAULT_MODEL
        } else {
            &self.model
        }
    }
}

#[derive(Debug, Clone)]
pub struct Prompt<'a> {
    pub system: &'a str,
    pub user: &'a str,
    pub model: &'a str,
    pub max_tokens: u32,
}

pub struct OpenRouter<'a> {
    pub(crate) http: &'a mut dyn HttpClient,
    pub(crate) key: &'a str,
}

fn failure(status: u16, body: &Value) -> RunError {
    let detail = body["error"]["message"].as_str().unwrap_or("").to_string();
    let (message, retryable) = match status {
        401 => (
            "OpenRouter does not accept the API key (Device page)".to_string(),
            false,
        ),
        402 => (
            "OpenRouter credits have run out; top up at openrouter.ai".to_string(),
            false,
        ),
        403 => (format!("OpenRouter refused the request: {detail}"), false),
        404 | 400 => (format!("OpenRouter: {detail}"), false),
        429 => ("OpenRouter is rate-limiting this key".to_string(), true),
        _ => (
            format!("OpenRouter answered {status}: {detail}"),
            status >= 500,
        ),
    };
    RunError { message, retryable }
}

impl OpenRouter<'_> {
    /// One completion: the model's text, cleaned.
    pub fn complete(&mut self, prompt: &Prompt<'_>) -> Result<String, RunError> {
        let mut messages = Vec::new();
        if !prompt.system.trim().is_empty() {
            messages.push(json!({ "role": "system", "content": prompt.system }));
        }
        messages.push(json!({ "role": "user", "content": prompt.user }));
        let body = json!({
            "model": prompt.model,
            "max_tokens": prompt.max_tokens.clamp(1, MAX_TOKENS_LIMIT),
            "messages": messages,
        });
        let req = HttpRequest {
            method: "POST",
            url: format!("{BASE_URL}/chat/completions"),
            headers: vec![
                ("Authorization".into(), format!("Bearer {}", self.key)),
                ("Content-Type".into(), "application/json".into()),
                // OpenRouter's app attribution headers.
                (
                    "HTTP-Referer".into(),
                    "https://github.com/matthewdeanmartin/mastomini".into(),
                ),
                ("X-Title".into(), "mastomini-bots".into()),
            ],
            body: serde_json::to_vec(&body).unwrap_or_default(),
        };
        let res = self.http.send(&req).map_err(|e| RunError {
            message: format!("Could not reach OpenRouter: {e}"),
            retryable: true,
        })?;
        let value: Value = serde_json::from_slice(&res.body).unwrap_or(Value::Null);
        if !(200..300).contains(&res.status) {
            return Err(failure(res.status, &value));
        }
        // Some providers answer 200 with an error object.
        if value.get("error").is_some() && value["choices"].is_null() {
            return Err(failure(
                value["error"]["code"].as_u64().unwrap_or(502) as u16,
                &value,
            ));
        }
        let content = value["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("");
        crate::text::clean_completion(content).ok_or_else(|| RunError {
            message: "The model replied with nothing usable".into(),
            retryable: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mastodon::testing::Scripted;

    #[test]
    fn asks_and_cleans() {
        let mut http = Scripted::default();
        http.answer(
            200,
            json!({"choices": [{"message": {"content": "Sure, here it is:\nHello!"}}]}),
        );
        let sent = http.sent.clone();
        let mut or = OpenRouter {
            http: &mut http,
            key: "sk-or-x",
        };
        let prompt = Prompt {
            system: "Be brief.",
            user: "Say hi",
            model: DEFAULT_MODEL,
            max_tokens: 99_999,
        };
        assert_eq!(or.complete(&prompt).unwrap(), "Hello!");
        let req = &sent.lock().unwrap()[0];
        assert_eq!(req.url, "https://openrouter.ai/api/v1/chat/completions");
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["model"], DEFAULT_MODEL);
        assert_eq!(body["max_tokens"], MAX_TOKENS_LIMIT);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "Say hi");
    }

    #[test]
    fn failures_say_what_to_do() {
        let mut http = Scripted::default();
        http.answer(402, json!({"error": {"message": "no credits"}}));
        http.answer(429, json!({}));
        http.answer(200, json!({"choices": [{"message": {"content": "  "}}]}));
        let mut or = OpenRouter {
            http: &mut http,
            key: "k",
        };
        let p = Prompt {
            system: "",
            user: "x",
            model: "m",
            max_tokens: 10,
        };
        let e = or.complete(&p).unwrap_err();
        assert!(e.message.contains("credits") && !e.retryable);
        assert!(or.complete(&p).unwrap_err().retryable);
        assert!(or
            .complete(&p)
            .unwrap_err()
            .message
            .contains("nothing usable"));
    }
}
