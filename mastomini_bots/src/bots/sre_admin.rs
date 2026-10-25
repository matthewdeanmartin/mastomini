//! Weekly counts only. No extra task or model; reuses the daily SRE run.
use crate::bot::Run;
use crate::mastodon::HttpRequest;
use serde_json::Value;

pub fn week(slot_ms: u64) -> u64 {
    (slot_ms / 86_400_000 + 3) / 7
}

pub fn due(run: &Run<'_>) -> bool {
    run.settings.yes("admin_digest")
        && run
            .state
            .get("admin_week")
            .and_then(|s| s.parse::<u64>().ok())
            != Some(week(run.slot_ms))
}

pub fn counts(run: &mut Run<'_>) -> String {
    let local = run
        .state
        .get("admin_local")
        .map(String::as_str)
        .unwrap_or("Bot counts unavailable")
        .to_string();
    let key = run.settings.get("admin_key");
    if key.is_empty() {
        return format!("{local}. Mastomini admin counts: no read key configured.");
    }
    let request = HttpRequest {
        method: "GET",
        url: format!("{}/api/mastomini/v1/admin/digest", run.instance),
        headers: vec![
            ("Authorization".into(), format!("Bearer {key}")),
            ("Accept".into(), "application/json".into()),
        ],
        body: Vec::new(),
    };
    let counts = run
        .http
        .send(&request)
        .ok()
        .filter(|r| r.status == 200)
        .and_then(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .filter(|v| v["schema"] == "mastomini.admin-digest/1");
    let Some(v) = counts else {
        return format!("{local}. Mastomini admin counts unavailable.");
    };
    match (v["unresolved_reports"].as_u64(), v["pending_approvals"].as_u64(), v["invites_expiring_7d"].as_u64()) {
        (Some(reports), Some(approvals), Some(invites)) => format!("{local}. Mastomini: {reports} open reports, {approvals} pending approvals, {invites} invites expire within 7d."),
        _ => format!("{local}. Mastomini admin counts invalid."),
    }
}

/// Expected builds are explicit host=build entries from verified receipts.
/// Missing baselines and missing fingerprints are unknown, never all-clear.
pub fn drift(expected: &str, actual: &[(&str, &str)]) -> String {
    let mut changed = 0;
    let mut unknown = 0;
    let mut names = String::new();
    for (host, build) in actual {
        let baseline = expected
            .lines()
            .filter_map(|line| line.trim().split_once('='))
            .find(|(h, _)| h.trim() == *host)
            .map(|(_, b)| b.trim());
        match baseline {
            Some(b) if !b.is_empty() && !build.is_empty() && *build != "dev" => {
                if b != *build {
                    changed += 1;
                    if names.chars().count() < 40 {
                        if !names.is_empty() {
                            names.push(',');
                        }
                        names.extend(
                            host.trim_end_matches(".local")
                                .chars()
                                .take(40 - names.chars().count()),
                        );
                    }
                }
            }
            _ => unknown += 1,
        }
    }
    let names = if names.is_empty() {
        String::new()
    } else {
        format!(" ({names})")
    };
    format!("Firmware: {changed} changed{names}, {unknown} unknown baselines/builds ({} boards checked).", actual.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_dev_builds_are_unknown_not_matching() {
        assert_eq!(
            drift(
                "a=one\nb=old\nc=dev",
                &[("a", "one"), ("b", "new"), ("c", "dev"), ("d", "x")]
            ),
            "Firmware: 1 changed (b), 2 unknown baselines/builds (4 boards checked)."
        );
    }
}
