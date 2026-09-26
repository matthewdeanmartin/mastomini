//! mastomini: a non-federating Mastodon-compatible server for one household.
//!
//! The library is transport-neutral: the desktop binary and (later) the
//! ESP32 firmware feed requests into [`api::handle`] and persist through a
//! [`store::Store`]. See `spec/` for the design.

pub mod api;
pub mod auth;
pub mod avatar;
pub mod captive;
pub mod codec;
pub mod crypto;
pub mod domain;
pub mod http;
pub mod http_transport;
pub mod ids;
pub mod incidents;
pub mod store;
pub mod text;
pub mod tls;
pub mod web;
