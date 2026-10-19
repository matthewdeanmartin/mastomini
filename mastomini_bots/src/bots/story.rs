//! Shared bounded text generation for the daily fiction bots.
use crate::{
    bot::{Run, RunError},
    mastodon::{NewStatus, Visibility},
    openrouter::Prompt,
    settings::{schedule_settings, Setting},
};

pub fn settings() -> Vec<Setting> {
    let mut s = schedule_settings("daily", "12:00", "us_eastern", "5");
    s.push(super::good_morning::visibility_setting());
    s
}
pub fn date(run: &Run<'_>) -> String {
    let d = run.settings.tz("zone").local(run.slot_ms);
    format!("{}-{:02}-{:02}", d.year, d.month, d.day)
}
pub fn generate(run: &mut Run<'_>, rules: &str, input: &str) -> Result<String, RunError> {
    let model = run.default_model().to_string();
    run.llm()?.complete(&Prompt {
        system: rules,
        user: input,
        model: &model,
        max_tokens: 900,
    })
}
pub fn status(run: &Run<'_>, text: &str, limit: usize, parent: Option<String>) -> NewStatus {
    NewStatus {
        text: crate::text::fit(&crate::text::defuse_mentions(text, &[]), limit),
        visibility: Visibility::parse(run.settings.get("visibility")),
        spoiler_text: None,
        in_reply_to_id: parent,
    }
}
pub fn invalid(message: &str) -> RunError {
    RunError {
        message: message.into(),
        retryable: true,
    }
}
