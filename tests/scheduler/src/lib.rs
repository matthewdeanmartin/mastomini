//! Cross-board contract tests, isolated from ESP-IDF component discovery.
#[cfg(test)]
mod tests {
    use mastobots::mastodon::{HttpClient, HttpRequest, HttpResponse};
    use mastobots::service::ConfigUpdate;
    use mastomini::api::{handle, time, Ctx};
    use mastomini::domain::{records::Role, scheduled::Bridge, Config, NewMember, Service};
    use mastomini::http::Request;
    use mastomini::store::mem::MemStore;
    use std::sync::{Arc, Mutex};
    const T0: u64 = 1_790_000_000_000;
    struct Server {
        svc: Service<MemStore>,
        ctx: Ctx,
    }
    fn setup() -> (Server, String, String) {
        let mut svc = Service::open(
            MemStore::default(),
            Config {
                host: "mastomini.test".into(),
                password_rounds: 1,
            },
        )
        .unwrap();
        svc.provision(
            NewMember {
                username: "alice".into(),
                password: "alicepw".into(),
                display_name: "Alice".into(),
                role: Role::Owner,
            },
            Some("Home".into()),
            T0,
        )
        .unwrap();
        let (_, key) = svc
            .create_api_key(0, "alicepw", "Existing bots key", "read write", T0)
            .unwrap();
        svc.set_scheduler(Bridge {
            bots_url: "https://mastomini-bots.local".into(),
            api_key: key.clone(),
        })
        .unwrap();
        (
            Server {
                svc,
                ctx: Ctx::new("https://mastomini.test"),
            },
            key.clone(),
            key,
        )
    }
    fn schedule(s: &mut Server, token: &str, at: u64) -> u64 {
        let req = Request::new("POST", "/api/v1/statuses")
            .with_header("Authorization", &format!("Bearer {token}"))
            .with_body(
                "application/json",
                serde_json::to_vec(
                    &serde_json::json!({"status":"Due later", "scheduled_at":time::iso(at)}),
                )
                .unwrap(),
            );
        let r = handle(&mut s.svc, &s.ctx, &req, Some(T0));
        assert_eq!(r.status, 200);
        r.json_body()["id"].as_str().unwrap().parse().unwrap()
    }
    struct Callback {
        main: Arc<Mutex<Service<MemStore>>>,
        ctx: Ctx,
        now: u64,
        lose_reply: bool,
    }
    impl HttpClient for Callback {
        fn send(&mut self, call: &HttpRequest) -> Result<HttpResponse, String> {
            assert_eq!(call.method, "POST");
            let path = call.url.strip_prefix("https://mastomini.test").unwrap();
            let mut req =
                Request::new("POST", path).with_body("application/json", call.body.clone());
            req.headers = call.headers.clone();
            let r = handle(
                &mut self.main.lock().unwrap(),
                &self.ctx,
                &req,
                Some(self.now),
            );
            if self.lose_reply {
                self.lose_reply = false;
                return Err("reply lost".into());
            }
            Ok(HttpResponse {
                status: r.status,
                body: r.body,
            })
        }
    }

    #[test]
    fn both_boards_handoff_and_retry_a_lost_publication_reply() {
        let (mut s, alice, key) = setup();
        let at = T0 + 600_000;
        let id = schedule(&mut s, &alice, at);
        let main = Arc::new(Mutex::new(s.svc));
        let mut bots = mastobots::service::Service::open(
            mastobots::store::MemStore::default(),
            mastobots::bots::all(),
        )
        .unwrap();
        bots.configure(
            0,
            ConfigUpdate {
                instance: Some("https://mastomini.test".into()),
                token: Some(key.clone()),
                ..Default::default()
            },
            Some(T0),
        )
        .unwrap();
        mastomini::scheduler_bridge::transfer(&main, |job| {
            let mut req = mastobots::http::Request::new("POST", "/api/v1/scheduler/jobs")
                .with_header("Authorization", &format!("Bearer {}", job.key));
            req.body = job.body.clone();
            let r = mastobots::scheduler::receive(&mut bots, &req);
            assert_eq!(r.status, 200, "{}", r.json_body());
            Ok(r.status)
        });
        assert!(main
            .lock()
            .unwrap()
            .scheduling
            .acknowledged
            .contains(&(id, 1)));
        assert_eq!(bots.scheduler.jobs.len(), 1);
        let bots = Arc::new(Mutex::new(bots));
        let mut client = Callback {
            main: Arc::clone(&main),
            ctx: s.ctx,
            now: at,
            lose_reply: true,
        };
        assert_eq!(mastobots::scheduler::tick(&bots, &mut client, None), 0);
        assert_eq!(
            mastobots::scheduler::tick(&bots, &mut client, Some(at - 1)),
            0
        );
        assert_eq!(mastobots::scheduler::tick(&bots, &mut client, Some(at)), 1);
        assert_eq!(main.lock().unwrap().state.statuses.len(), 1);
        assert_eq!(bots.lock().unwrap().scheduler.jobs.len(), 1);
        client.now += 30_001;
        assert_eq!(
            mastobots::scheduler::tick(&bots, &mut client, Some(at + 30_001)),
            1
        );
        assert!(bots.lock().unwrap().scheduler.jobs.is_empty());
        assert_eq!(main.lock().unwrap().state.statuses.len(), 1);
    }
}
