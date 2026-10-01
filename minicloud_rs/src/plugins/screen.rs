use crate::store::Store;
use crate::{dismiss_state, Error, Job, Notice, Result, State, MAX_NOTICES};
use serde_json::{json, Value};

pub struct Plugin;
impl crate::Plugin for Plugin {
    fn name(&self) -> &'static str {
        "screen"
    }
    fn accepts(&self, topic: &str) -> bool {
        matches!(
            topic,
            "minicloud/screen/notify" | "minicloud/screen/read" | "minicloud/image/show"
        )
    }
    fn run(&self, job: &Job, state: &mut State, _store: &Store) -> Result<Value> {
        let mut value = job.payload.clone();
        value
            .as_object_mut()
            .ok_or_else(|| Error::new(400, "expected JSON object"))?
            .remove("event_id");
        if job.topic == "minicloud/screen/read" {
            let source = value["source"].as_str().unwrap_or("");
            let id = value["id"].as_str().unwrap_or("");
            if !crate::name(source) || !crate::name(id) {
                return Err(Error::new(400, "invalid source/id"));
            }
            dismiss_state(state, &format!("{source}/{id}"));
            return Ok(json!({"dismissed":true}));
        }
        let notice: Notice = serde_json::from_value(value)?;
        notice.validate()?;
        if let Some(image) = &notice.image {
            let blob = state
                .blobs
                .iter()
                .find(|b| b.bucket == image.bucket && b.key == image.key)
                .ok_or_else(|| Error::new(404, "image blob does not exist"))?;
            #[cfg(target_os = "espidf")]
            if blob.mime != "application/x-rgb565" {
                return Err(Error::new(400, "prepare the image for the LCD in the file browser, then select its .rgb565 version"));
            }
            if !blob.mime.starts_with("image/") && blob.mime != "application/x-rgb565" {
                return Err(Error::new(400, "blob is not an image"));
            }
        }
        if notice.source == "nanacoin"
            && (notice.id.starts_with("stats-0-") || notice.id.starts_with("stats-1-"))
        {
            let prefix = &notice.id[..8];
            state.notices.retain(|n| {
                !(n.source == "nanacoin" && n.id.starts_with(prefix) && n.id != notice.id)
                    && !(matches!(n.source.as_str(), "minicloud" | "deployment")
                        && n.text == "Minicloud is ready!")
            });
        }
        if state.dismissed.contains(&notice.identity()) {
            return Ok(json!({"already_read":true}));
        }
        if let Some(existing) = state
            .notices
            .iter_mut()
            .find(|n| n.identity() == notice.identity())
        {
            let mut notice = notice;
            notice.expires_at = notice.expires_at.min(existing.expires_at);
            *existing = notice;
        } else {
            if state.notices.len() == MAX_NOTICES {
                return Err(Error::new(429, "screen inbox full"));
            }
            state.notices.push(notice);
        }
        Ok(json!({"displayed":true}))
    }
}
