//! Read-only, bounded trend estimates. No action authority comes from metrics.
use crate::mastodon::{normalize_instance, HttpClient, HttpRequest};
use serde::{Deserialize, Serialize};

const HOUR: u64 = 3_600_000;
const FRESH: u64 = 15 * 60_000;
const MAX_BODY: usize = 32 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Trend {
    pub host: String,
    pub bytes_per_hour: f64,
    pub floor_bytes: f64,
    pub estimated_floor_ms: u64,
    pub observed_ms: u64,
}

#[derive(Deserialize)]
struct Index {
    series: Vec<Series>,
}
#[derive(Deserialize)]
struct Series {
    id: u32,
    tags: String,
    field: String,
}
#[derive(Deserialize)]
struct Query {
    series: Vec<Data>,
}
#[derive(Deserialize)]
struct Data {
    id: u32,
    kind: String,
    #[serde(default)]
    next: Option<u64>,
    #[serde(default)]
    points: Vec<Point>,
}
#[derive(Clone, Copy, Deserialize)]
struct Point {
    t: u64,
    v: f64,
}

fn get<T: serde::de::DeserializeOwned>(
    http: &mut dyn HttpClient,
    url: String,
) -> Result<T, String> {
    let response = http
        .send_bounded(
            &HttpRequest {
                method: "GET",
                url,
                headers: vec![("Accept".into(), "application/json".into())],
                body: Vec::new(),
            },
            MAX_BODY,
        )
        .map_err(|_| "Collector unavailable".to_string())?;
    if response.status != 200 || response.body.len() > MAX_BODY {
        return Err("Collector response unavailable or too large".into());
    }
    serde_json::from_slice(&response.body).map_err(|_| "Invalid collector response".into())
}

fn tag<'a>(tags: &'a str, name: &str) -> Option<&'a str> {
    tags.split(',').find_map(|s| {
        s.split_once('=')
            .filter(|(k, _)| *k == name)
            .map(|(_, v)| v)
    })
}

/// At most four hosts and two series per query. A missing or ambiguous uptime
/// series is unknown, never permission to extrapolate across a reboot.
pub fn read(
    http: &mut dyn HttpClient,
    collector: &str,
    boards: &[(&str, f64)],
    now: u64,
) -> Result<Vec<Trend>, String> {
    let base = normalize_instance(collector)?;
    let index: Index = get(http, format!("{base}/api/v1/series"))?;
    if index.series.len() > 128 {
        return Err("Collector has too many series".into());
    }
    let mut out = Vec::new();
    for &(host, floor) in boards.iter().take(4) {
        let heaps: Vec<_> = index
            .series
            .iter()
            .filter(|s| s.field == "heap_internal_free" && tag(&s.tags, "host") == Some(host))
            .collect();
        if heaps.len() != 1 {
            continue;
        }
        let heap = heaps[0];
        let uptimes: Vec<_> = index
            .series
            .iter()
            .filter(|s| s.field == "uptime_s" && s.tags == heap.tags)
            .collect();
        if uptimes.len() != 1 {
            continue;
        }
        let uptime = uptimes[0];
        // Refuse paging/buckets rather than quietly interpreting partial or
        // averaged samples as a continuous raw series. 192 points fit 90 minutes
        // at the household's actual 30-second scrape interval.
        let q: Query = get(
            http,
            format!(
                "{base}/api/v1/query?ids={},{}&from={}&to={}&max=192&limit=192&raw=1&shape=rows",
                heap.id,
                uptime.id,
                now.saturating_sub(90 * 60_000),
                now.saturating_add(1)
            ),
        )?;
        if q.series.len() != 2
            || q.series
                .iter()
                .any(|s| s.kind != "raw" || s.next.is_some() || s.points.len() > 192)
        {
            continue;
        }
        let Some(h) = q.series.iter().find(|s| s.id == heap.id) else {
            continue;
        };
        let Some(u) = q.series.iter().find(|s| s.id == uptime.id) else {
            continue;
        };
        if let Some(t) = estimate(host, &h.points, &u.points, floor, now) {
            out.push(t);
        }
    }
    Ok(out)
}

fn estimate(host: &str, heap: &[Point], uptime: &[Point], floor: f64, now: u64) -> Option<Trend> {
    if heap.len() < 6 || heap.len() != uptime.len() || !floor.is_finite() || floor <= 0.0 {
        return None;
    }
    let mut start = 0;
    for (i, (h, u)) in heap.iter().zip(uptime).enumerate() {
        if h.t != u.t || !h.v.is_finite() || !u.v.is_finite() || h.v < 0.0 || u.v < 0.0 || h.t > now
        {
            return None;
        }
        if i > 0 {
            if h.t <= heap[i - 1].t {
                return None;
            }
            if h.t - heap[i - 1].t > 30 * 60_000 || u.v <= uptime[i - 1].v {
                start = i;
            }
            // Uptime must advance with sample time. This also catches a
            // reset whose new uptime happens to exceed the preceding sample.
            let elapsed = (h.t - heap[i - 1].t) as f64 / 1000.0;
            if ((u.v - uptime[i - 1].v) - elapsed).abs() > 5.0 {
                start = i;
            }
        }
    }
    let heap = &heap[start..];
    let first = heap.first()?;
    let last = heap.last()?;
    if heap.len() < 6 || last.t - first.t < HOUR || now - last.t > FRESH || last.v <= floor {
        return None;
    }
    let n = heap.len() as f64;
    let mean_x = heap
        .iter()
        .map(|p| (p.t - first.t) as f64 / HOUR as f64)
        .sum::<f64>()
        / n;
    let mean_y = heap.iter().map(|p| p.v).sum::<f64>() / n;
    let xx = heap
        .iter()
        .map(|p| ((p.t - first.t) as f64 / HOUR as f64 - mean_x).powi(2))
        .sum::<f64>();
    let xy = heap
        .iter()
        .map(|p| ((p.t - first.t) as f64 / HOUR as f64 - mean_x) * (p.v - mean_y))
        .sum::<f64>();
    let yy = heap.iter().map(|p| (p.v - mean_y).powi(2)).sum::<f64>();
    let slope = xy / xx;
    // Reject noise and harmless flat heaps. A forecast is a hint, not an SLA.
    if !slope.is_finite() || slope > -256.0 || yy <= 0.0 || xy * xy / (xx * yy) < 0.8 {
        return None;
    }
    let remaining = (last.v - floor) / -slope * HOUR as f64;
    if !remaining.is_finite() || remaining > (24 * HOUR) as f64 {
        return None;
    }
    let at = last.t.saturating_add(remaining as u64);
    if at <= now {
        return None;
    }
    Some(Trend {
        host: host.into(),
        bytes_per_hour: slope,
        floor_bytes: floor,
        estimated_floor_ms: at,
        observed_ms: last.t,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_thirty_second_scrapes_fit_and_paged_data_is_not_extrapolated() {
        use serde_json::json;
        let mut http = crate::mastodon::testing::Scripted::default();
        let heap: Vec<_> = (0..=180)
            .map(|i| json!({"t": HOUR + i * 30_000, "v": 20000 - i * 20}))
            .collect();
        let uptime: Vec<_> = (0..=180)
            .map(|i| json!({"t": HOUR + i * 30_000, "v": 3600 + i * 30}))
            .collect();
        for paged in [false, true] {
            http.answer(200, json!({"series":[{"id":1,"tags":"host=a.local,source=a","field":"heap_internal_free"},{"id":2,"tags":"host=a.local,source=a","field":"uptime_s"}]}));
            let mut query = json!({"series":[{"id":1,"kind":"raw","points":heap},{"id":2,"kind":"raw","points":uptime}]});
            if paged {
                query["series"][0]["next"] = json!(HOUR + 60_000);
            }
            http.answer(200, query);
            let trends = read(
                &mut http,
                "https://collector.local",
                &[("a.local", 4096.0)],
                HOUR + 90 * 60_000,
            )
            .unwrap();
            assert_eq!(trends.len(), usize::from(!paged));
            if let Some(t) = trends.first() {
                assert!((t.bytes_per_hour + 2400.0).abs() < 0.01);
            }
        }
    }
    fn samples() -> (Vec<Point>, Vec<Point>) {
        (
            (0..=12)
                .map(|i| Point {
                    t: HOUR + i * 600_000,
                    v: 16000.0 - i as f64 * 256.0,
                })
                .collect(),
            (0..=12)
                .map(|i| Point {
                    t: HOUR + i * 600_000,
                    v: 3600.0 + i as f64 * 600.0,
                })
                .collect(),
        )
    }
    #[test]
    fn falling_heap_estimates_floor_but_flat_stale_or_rebooted_data_does_not() {
        let (mut h, mut u) = samples();
        let now = h.last().unwrap().t;
        let trend = estimate("x", &h, &u, 4096.0, now).unwrap();
        assert!((trend.bytes_per_hour + 1536.0).abs() < 0.01);
        assert!(trend.estimated_floor_ms > now);
        assert!(estimate("x", &h, &u, 4096.0, now + FRESH + 1).is_none());
        u[10].v = 0.0;
        assert!(estimate("x", &h, &u, 4096.0, now).is_none());
        let (_, u) = samples();
        for p in &mut h {
            p.v = 8000.0;
        }
        assert!(estimate("x", &h, &u, 4096.0, now).is_none());
    }
    #[test]
    fn gaps_duplicates_future_and_noisy_data_are_unknown() {
        let (h, u) = samples();
        let now = h.last().unwrap().t;
        let mut bad = h.clone();
        bad[6].t = bad[5].t;
        assert!(estimate("x", &bad, &u, 4096.0, now).is_none());
        assert!(estimate("x", &h, &u, 4096.0, now - 1).is_none());
        let mut bad = h.clone();
        bad[5].v = 64000.0;
        assert!(estimate("x", &bad, &u, 4096.0, now).is_none());
    }
}
