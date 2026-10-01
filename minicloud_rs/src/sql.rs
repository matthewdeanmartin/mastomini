//! Desktop-only stretch goal. Bounded, named SQLite files; no raw filesystem
//! paths, ATTACH, extension loading, unbounded query output or unlimited work.
#[cfg(target_os = "espidf")]
compile_error!("SQLite is not yet supported on the C6; build firmware without sqlite");
use crate::{store::Store, Error, Result};
use rusqlite::{
    hooks::{AuthAction, AuthContext, Authorization},
    limits::Limit,
    types::{Value as SqlValue, ValueRef},
    Connection,
};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

fn db_error(e: rusqlite::Error) -> Error {
    Error::new(400, e.to_string())
}
fn connection(store: &Store, database: &str) -> Result<Connection> {
    if !crate::name(database) {
        return Err(Error::new(400, "invalid database name"));
    }
    let path = store.root.join(format!("db-{database}.sqlite"));
    if !path.exists()
        && std::fs::read_dir(&store.root)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name().to_string_lossy().starts_with("db-")
                    && e.path().extension().is_some_and(|s| s == "sqlite")
            })
            .count()
            >= 4
    {
        return Err(Error::new(429, "four database limit reached"));
    }
    let conn = Connection::open(path).map_err(db_error)?;
    conn.busy_timeout(Duration::from_millis(100))
        .map_err(db_error)?;
    conn.execute_batch("PRAGMA page_size=4096; PRAGMA max_page_count=128;")
        .map_err(db_error)?;
    conn.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)
        .map_err(db_error)?;
    conn.set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, 2048)
        .map_err(db_error)?;
    conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, 4096)
        .map_err(db_error)?;
    conn.authorizer(Some(|context: AuthContext<'_>| match context.action {
        AuthAction::Attach { .. } | AuthAction::Detach { .. } => Authorization::Deny,
        AuthAction::Pragma { pragma_name, .. } if pragma_name != "table_info" => {
            Authorization::Deny
        }
        _ => Authorization::Allow,
    }));
    let start = Instant::now();
    conn.progress_handler(
        1000,
        Some(move || start.elapsed() > Duration::from_millis(100)),
    );
    Ok(conn)
}
pub fn query(store: &Store, database: &str, value: &Value) -> Result<Value> {
    let statements = value["statements"]
        .as_array()
        .ok_or_else(|| Error::new(400, "statements array required"))?;
    if statements.is_empty() || statements.len() > 8 {
        return Err(Error::new(400, "1-8 statements required"));
    }
    let mut conn = connection(store, database)?;
    let transaction = conn.transaction().map_err(db_error)?;
    let mut results = Vec::new();
    let mut output_bytes = 0;
    for statement in statements {
        let sql = statement["sql"]
            .as_str()
            .ok_or_else(|| Error::new(400, "sql required"))?;
        let params = statement
            .get("params")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let params: Vec<SqlValue> = params
            .iter()
            .map(|v| match v {
                Value::Null => Ok(SqlValue::Null),
                Value::String(s) => Ok(SqlValue::Text(s.clone())),
                Value::Number(n) => n
                    .as_i64()
                    .map(SqlValue::Integer)
                    .or_else(|| n.as_f64().map(SqlValue::Real))
                    .ok_or_else(|| Error::new(400, "invalid number")),
                Value::Bool(b) => Ok(SqlValue::Integer(*b as i64)),
                _ => Err(Error::new(400, "SQL params must be scalars")),
            })
            .collect::<Result<_>>()?;
        let mut prepared = transaction.prepare(sql).map_err(db_error)?;
        if prepared.column_count() == 0 {
            let changed = prepared
                .execute(rusqlite::params_from_iter(&params))
                .map_err(db_error)?;
            results.push(json!({"changed":changed}));
        } else {
            let columns: Vec<_> = prepared
                .column_names()
                .into_iter()
                .map(String::from)
                .collect();
            let mut rows = prepared
                .query(rusqlite::params_from_iter(&params))
                .map_err(db_error)?;
            let mut output = Vec::new();
            while let Some(row) = rows.next().map_err(db_error)? {
                if output.len() == 100 {
                    return Err(Error::new(413, "query exceeds 100 rows; add LIMIT"));
                }
                let mut values = Vec::new();
                for index in 0..columns.len() {
                    values.push(match row.get_ref(index).map_err(db_error)? {
                        ValueRef::Null => Value::Null,
                        ValueRef::Integer(i) => json!(i),
                        ValueRef::Real(f) => json!(f),
                        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                        ValueRef::Blob(_) => {
                            return Err(Error::new(400, "use blob service for binary data"))
                        }
                    });
                }
                output_bytes += serde_json::to_vec(&values)?.len();
                if output_bytes > 8192 {
                    return Err(Error::new(413, "query output exceeds 8 KiB"));
                }
                output.push(values);
            }
            results.push(json!({"columns":columns,"rows":output}));
        }
    }
    transaction.commit().map_err(db_error)?;
    Ok(json!({"results":results}))
}
pub fn insert_event(store: &Store, database: &str, id: &str, value: &Value) -> Result<Value> {
    let conn = connection(store, database)?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS worker_events(event_id TEXT PRIMARY KEY, payload TEXT NOT NULL);").map_err(db_error)?;
    let changed = conn
        .execute(
            "INSERT OR IGNORE INTO worker_events(event_id,payload) VALUES (?1,?2)",
            rusqlite::params![id, serde_json::to_string(value)?],
        )
        .map_err(db_error)?;
    Ok(json!({"inserted":changed,"database":database}))
}
