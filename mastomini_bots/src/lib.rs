//! mastomini-bots: many Mastodon bots on one ESP32-S3.
//!
//! - [`bot`]: what a bot is; [`bots`]: the bots in this build.
//! - [`schedule`] and [`tz`]: when they run, in local time.
//! - [`service`]: the state and scheduling rules; [`api`]: the admin site.
//! - [`mastodon`]: the client bots post with, over any [`mastodon::HttpClient`].
//!
//! The binaries (`src/bin/desktop.rs`, `src/bin/esp32.rs`) add the network,
//! storage and the scheduler thread.

pub mod api;
pub mod auth;
pub mod bot;
pub mod bots;
#[cfg(feature = "desktop")]
pub mod desktop_client;
pub mod http;
pub mod mastodon;
pub mod openrouter;
pub mod runner;
pub mod schedule;
pub mod service;
pub mod settings;
pub mod store;
pub mod template;
pub mod text;
pub mod tz;
pub mod web;
