#![cfg(feature = "sqlite")]
use minicloud::{sql, store::Store, Cloud};
use serde_json::json;

#[test]
fn transaction_rolls_back_and_database_paths_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sql::query(
        &store,
        "household",
        &json!({"statements":[{"sql":"CREATE TABLE notes(id INTEGER PRIMARY KEY, text TEXT)"}]}),
    )
    .unwrap();
    let failed = sql::query(
        &store,
        "household",
        &json!({"statements":[{"sql":"INSERT INTO notes VALUES(1, 'Hello')"},{"sql":"INSERT INTO missing VALUES(2)"}]}),
    );
    assert!(failed.is_err());
    let result = sql::query(
        &store,
        "household",
        &json!({"statements":[{"sql":"SELECT COUNT(*) FROM notes"}]}),
    )
    .unwrap();
    assert_eq!(result["results"][0]["rows"][0][0], 0);
    assert!(sql::query(
        &store,
        "../escape",
        &json!({"statements":[{"sql":"SELECT 1"}]})
    )
    .is_err());
    assert!(sql::query(
        &store,
        "household",
        &json!({"statements":[{"sql":"ATTACH DATABASE 'escape.sqlite' AS evil"}]})
    )
    .is_err());
    assert!(sql::query(
        &store,
        "household",
        &json!({"statements":[{"sql":"PRAGMA max_page_count=9999999"}]})
    )
    .is_err());
}
#[test]
fn sql_worker_side_effect_is_idempotent_even_when_job_ack_is_lost() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Cloud::open(dir.path(), "sqlite-test-token-1234".into()).unwrap();
    let value = json!({"event_id":"weather-1","database":"home","record":{"weather":"sunny"}});
    c.enqueue("minicloud/db/insert", value, None).unwrap();
    // Simulate SQL having committed immediately before cloud state write failed.
    sql::insert_event(
        &c.store,
        "home",
        "sql_insert:weather-1",
        &json!({"weather":"sunny"}),
    )
    .unwrap();
    c.work_once().unwrap();
    let result = sql::query(
        &c.store,
        "home",
        &json!({"statements":[{"sql":"SELECT COUNT(*) FROM worker_events"}]}),
    )
    .unwrap();
    assert_eq!(result["results"][0]["rows"][0][0], 1);
    assert_eq!(c.state.receipts[0].status, "done");
}
