//! mastomini: a non-federating Mastodon-compatible server for one household.
//!
//! The library is transport-neutral: the desktop binary and (later) the
//! ESP32 firmware feed requests into [`api::handle`] and persist through a
//! [`store::Store`]. See `spec/` for the design.

#![forbid(unsafe_code)]

pub mod api;
pub mod auth;
pub mod avatar;
pub mod board_status;
pub mod captive;
pub mod codec;
pub mod crypto;
pub mod domain;
pub mod http;
pub mod ids;
pub mod media;
/// The board's incident history now lives in the framework
/// (`/.well-known/incidents`); re-exported for the status light and `/diag`.
pub use miniframework::incidents;
pub mod store;
pub mod text;
pub mod tls;
pub mod web;

pub mod scheduler_bridge;
pub mod server;

// The firmware imports this policy from the bots crate; run its host tests here too.
#[cfg(test)]
#[path = "../../mastomini_bots/src/household_trust.rs"]
mod household_trust;
