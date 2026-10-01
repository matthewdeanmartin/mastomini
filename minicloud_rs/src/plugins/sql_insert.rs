// minicloud-feature: sqlite
use crate::{store::Store, Error, Job, Result, State};
use serde_json::Value;
pub struct Plugin;
impl crate::Plugin for Plugin {
    fn name(&self) -> &'static str {
        "sql_insert"
    }
    fn accepts(&self, topic: &str) -> bool {
        topic == "minicloud/db/insert"
    }
    fn run(&self, job: &Job, _state: &mut State, store: &Store) -> Result<Value> {
        let database = job.payload["database"]
            .as_str()
            .ok_or_else(|| Error::new(400, "database required"))?;
        let record = job
            .payload
            .get("record")
            .ok_or_else(|| Error::new(400, "record required"))?;
        // DB primary key makes a retry safe if SQL commits before cloud state.
        crate::sql::insert_event(store, database, &job.id, record)
    }
}
