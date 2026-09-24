//! Diagnostics, the clock fallback and the security summary
//! (`/api/mastomini/v1/diag`, `/clock`, `/admin/security`; spec/06).
//!
//! What only the platform knows (heap, uptime, Wi-Fi) comes from
//! [`Ctx::platform`](super::Ctx), so the same code serves the desktop build
//! and the board.

use super::{fail, time, Call, Reply};
use crate::auth;
use crate::domain::records::Role;
use crate::domain::{self, Error};
use crate::http::Response;
use crate::store::Store;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Instant;

/// Facts about the machine, for `/diag`. `None` where the platform can't say.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Platform {
    /// `desktop` or `esp32s3`.
    pub target: &'static str,
    pub uptime_ms: u64,
    pub heap_internal_free: Option<u64>,
    pub heap_internal_min_free: Option<u64>,
    pub heap_internal_largest_block: Option<u64>,
    pub psram_free: Option<u64>,
    pub psram_min_free: Option<u64>,
    pub reset_reason: Option<&'static str>,
    pub wifi_rssi: Option<i32>,
}

static STARTED: OnceLock<Instant> = OnceLock::new();

/// Milliseconds since this process started (the first [`super::Ctx`]).
pub fn host_uptime_ms() -> u64 {
    STARTED.get_or_init(Instant::now).elapsed().as_millis() as u64
}

impl Platform {
    /// What a desktop process can report.
    pub fn host() -> Platform {
        Platform {
            target: "desktop",
            uptime_ms: host_uptime_ms(),
            ..Platform::default()
        }
    }
}

/// Where the request's time came from (spec/05 "Time").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clock {
    /// SNTP on the board, the system clock on desktop.
    Synced,
    /// Set by an admin's browser (`POST /clock`): approximate.
    Manual,
    /// Not set yet: reads work, writes answer 503.
    Unset,
}

impl Clock {
    pub fn as_str(self) -> &'static str {
        match self {
            Clock::Synced => "synced",
            Clock::Manual => "manual",
            Clock::Unset => "unset",
        }
    }
}

/// How the server is reached. Only plain HTTP exists until Sprint 8, which
/// adds Easy mode (HTTPS and HTTP) and Secure mode (HTTPS only).
pub const TRANSPORT_MODE: &str = "http";

fn require_owner<S: Store>(c: &Call<'_, S>, slot: u8) -> Result<(), Response> {
    match c.svc.state.account(slot).map(|a| a.rec.role) {
        Some(Role::Owner) => Ok(()),
        _ => Err(fail(Error::Forbidden("Only the owner can see this".into()))),
    }
}

/// `GET /diag` (admins): everything the health page shows.
fn diag<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let actor = c.user()?;
    c.svc.require_admin(actor).map_err(fail)?;
    let platform = (c.ctx.platform)();
    let stats = c.svc.store().stats().ok();
    let (writes, window_ms) = c.svc.governor_counts();
    let s = &c.svc.state;
    let oldest = s.statuses.keys().next().copied();
    Ok(Response::ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "household_app": crate::web::bundled(),
        "platform": platform,
        "clock": {
            "source": c.clock.as_str(),
            "now": (c.clock != Clock::Unset).then(|| time::iso(c.now)),
        },
        "store": {
            "available": c.svc.latched().is_none(),
            "problem": c.svc.latched(),
            "entries_used": stats.map(|s| s.used_entries),
            "entries_total": stats.map(|s| s.total_entries),
            "used": stats.map(|s| s.used_fraction()),
            "low_watermark": domain::LOW_WATERMARK,
            "evictions_since_boot": s.evictions,
            "repairs_at_boot": s.repairs,
            "oldest_post_at": oldest.map(|id| time::iso(crate::ids::millis(id))),
        },
        "records": {
            "accounts": [s.active_accounts().count(), domain::MAX_ACCOUNTS],
            "statuses": [s.statuses.len(), domain::MAX_STATUSES],
            "boosts": [s.boosts.len(), domain::MAX_BOOSTS],
            "reactions": [s.reactions.len(), domain::MAX_REACTIONS],
            "apps": [s.apps.len(), domain::MAX_APPS],
            "tokens": [s.tokens.len(), domain::MAX_TOKENS],
            "reports": [s.reports.len(), domain::MAX_REPORTS],
            "codes": [s.invites.len(), domain::MAX_CODES_LIVE],
            "notifications": [s.notifications.len(), domain::MAX_NOTIFICATIONS],
        },
        "governor": {
            "writes_this_hour": writes,
            "limit_per_hour": domain::GOVERNOR_GLOBAL_HOUR,
            "limit_per_member_hour": domain::GOVERNOR_PER_ACCOUNT_HOUR,
            "window_started_at": (window_ms > 0).then(|| time::iso(window_ms)),
        },
    })))
}

/// `POST /clock` with `{ms}` (the browser's `Date.now()`): an admin sets the
/// time when the board has no internet. Accepted only while the clock isn't
/// synced, and only if later than the newest stored record, so ids stay in
/// order. Kept in RAM; SNTP takes over as soon as it syncs.
fn set_clock<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let actor = c.user()?;
    c.svc.require_admin(actor).map_err(fail)?;
    if c.clock == Clock::Synced {
        return Err(Response::error(409, "The clock is already set"));
    }
    let ms = c
        .params
        .u64("ms")
        .ok_or_else(|| Response::error(422, "Validation failed: ms is required"))?;
    let newest = crate::ids::millis(c.svc.last_id());
    // Before 2100: a typo, not a date.
    if ms <= newest || ms > 4_102_444_800_000 {
        return Err(Response::error(
            422,
            "Validation failed: That time is earlier than the newest post; check the device's clock",
        ));
    }
    c.svc.state.clock_anchor = Some((ms, (c.ctx.uptime_ms)()));
    Ok(Response::ok(
        json!({ "source": "manual", "now": time::iso(ms) }),
    ))
}

/// `GET /admin/security` (owner): transport, passwords, keys and devices.
fn security<S: Store>(c: &Call<'_, S>) -> Reply {
    let actor = c.user()?;
    require_owner(c, actor)?;
    let s = &c.svc.state;
    let members: Vec<Value> = s
        .active_accounts()
        .map(|a| {
            json!({
                "username": a.rec.username,
                "role": a.rec.role.as_str(),
                "devices": s.tokens.iter().filter(|t| t.rec.slot == a.slot).count(),
                // Without a key nobody can send them a direct message until
                // they sign in again (spec/05 "Direct messages").
                "message_key": c.svc.user_key(a.slot).is_some(),
            })
        })
        .collect();
    Ok(Response::ok(json!({
        "transport": {
            "mode": TRANSPORT_MODE,
            "https": false,
            "secure_mode_available": false,
        },
        "certificate": Value::Null,
        "household_ca": Value::Null,
        "passwords": {
            "min_length": auth::PASSWORD_MIN,
            "rounds": c.svc.config.password_rounds,
            "lockout_failures": auth::LOCKOUT_FAILURES,
            "lockout_minutes": auth::LOCKOUT_MS / 60_000,
        },
        "apps": [s.apps.len(), domain::MAX_APPS],
        "tokens": [s.tokens.len(), domain::MAX_TOKENS],
        "tokens_per_member": domain::MAX_TOKENS_PER_ACCOUNT,
        "members": members,
    })))
}

pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    Some(match (method, seg) {
        ("GET", ["diag"]) => diag(c),
        ("POST", ["clock"]) => set_clock(c),
        ("GET", ["admin", "security"]) => security(c),
        _ => return None,
    })
}
