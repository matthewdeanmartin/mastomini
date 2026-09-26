//! Every bot on this device. To add one:
//!
//! 1. Write `src/bots/<name>.rs` with a type that implements
//!    [`Bot`](crate::bot::Bot) (copy `good_morning.rs`; `llm.rs` for one
//!    that writes with a model).
//! 2. Add `mod <name>;` and an entry in [`all`] below.
//! 3. `make test`, then `make firmware` and flash. Its server, API key, on/off
//!    and its own settings are chosen on the admin site; a new bot starts off.
//!
//! A type can be listed more than once with different ids: each is a
//! separate bot with its own settings, schedule and memory.

use crate::bot::Bot;

pub mod good_morning;
pub mod llm;

pub fn all() -> Vec<Box<dyn Bot>> {
    vec![
        Box::new(good_morning::GoodMorning),
        // One bot type, two configured instances with their own settings.
        Box::new(llm::LlmBot {
            id: "llm_reply",
            name: "LLM replies",
            mode: "reply",
        }),
        Box::new(llm::LlmBot {
            id: "llm_post",
            name: "LLM posts",
            mode: "post",
        }),
    ]
}

#[cfg(test)]
mod tests {
    use crate::bot::valid_id;

    #[test]
    fn every_bot_is_well_formed() {
        let bots = super::all();
        let mut ids = Vec::new();
        for bot in &bots {
            let info = bot.info();
            assert!(valid_id(info.id), "bad id {:?}", info.id);
            assert!(!ids.contains(&info.id), "duplicate id {:?}", info.id);
            info.schedule.check().unwrap();
            // Every default value is one its own setting accepts.
            let specs = bot.settings();
            for spec in &specs {
                spec.check(spec.default)
                    .unwrap_or_else(|e| panic!("{}: {e}", info.id));
            }
            let settings = crate::settings::Settings::new(specs, Default::default());
            bot.schedule(&settings).check().unwrap();
            ids.push(info.id);
        }
    }
}
