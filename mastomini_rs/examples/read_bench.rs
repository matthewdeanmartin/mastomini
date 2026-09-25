//! Repeatable in-process read workload. No network, disk, or existing store.
//! cargo run --release --locked --example read_bench -- 4096 500
use mastomini::api::{entities, Ctx};
use mastomini::domain::query::PageQuery;
use mastomini::domain::records::Role;
use mastomini::domain::{Config, NewMember, NewStatus, Service};
use mastomini::store::mem::MemStore;
use serde_json::json;
use std::{hint::black_box, time::Instant};

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let count: usize = args.get(1).map_or(4096, |v| v.parse().unwrap());
    let rounds: usize = args.get(2).map_or(500, |v| v.parse().unwrap());
    assert!(count <= 4096 && rounds > 0);
    let mut svc = Service::open(
        MemStore::default(),
        Config {
            host: "bench.local".into(),
            password_rounds: 1,
        },
    )
    .unwrap();
    let start = 1_790_000_000_000;
    svc.provision(
        NewMember {
            username: "alice".into(),
            password: "benchmark-only".into(),
            display_name: "Alice".into(),
            role: Role::Owner,
        },
        None,
        start,
    )
    .unwrap();
    for i in 0..count {
        // Advance the synthetic clock so seeding exercises the real governor.
        svc.post_status(
            0,
            NewStatus {
                text: format!("Post {i} #rust https://example.test/link"),
                ..Default::default()
            },
            start + (i as u64 + 1) * 60000,
        )
        .unwrap();
    }
    let ctx = Ctx::new("http://bench.local");
    let q = PageQuery {
        limit: 40,
        ..Default::default()
    };
    let mut times = Vec::new();
    let mut bytes = 0;
    for _ in 0..rounds {
        let begin = Instant::now();
        let page = svc.home(0, &q);
        let rows: Vec<_> = page
            .into_iter()
            .map(|(_, e)| entities::entry(&svc, &ctx, e, Some(0)))
            .collect();
        let body = serde_json::to_vec(&rows).unwrap();
        bytes = body.len();
        black_box(body);
        times.push(begin.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    println!(
        "{}",
        json!({"workload":"40-row home query + entity JSON + serialize", "transport":"none", "profile":if cfg!(debug_assertions){"debug"}else{"release"},"statuses":count,"rounds":rounds,"response_bytes":bytes,"p50_ms":times[(rounds-1)/2],"p95_ms":times[(rounds*95).div_ceil(100)-1]})
    );
}
