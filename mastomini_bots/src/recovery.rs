//! Explicit USB recovery, applied once per recovery image before services start.
use crate::store::KvStore;

const MARKER: &str = "admin_reset";

pub fn reset_admin_once(store: &mut impl KvStore, id: Option<&str>) -> Result<bool, String> {
    let Some(id) = id else { return Ok(false) };
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid admin recovery identifier".into());
    }
    if store.get(MARKER)?.as_deref() == Some(id) {
        return Ok(false);
    }
    // Do not expose setup until both durable operations succeed. A power
    // failure between them retries recovery before starting the web server.
    store.delete("admin")?;
    store.put(MARKER, id)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Verifier;
    use crate::store::MemStore;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn recovery_preserves_bot_secrets_and_does_not_reset_new_password_on_reboot() {
        let mut store = MemStore::default();
        store.put("admin", "old verifier").unwrap();
        store.put("b.llm", "saved bot settings and key").unwrap();
        store.put("i.openrouter", "saved model key").unwrap();
        let before = store.map.clone();
        assert!(!reset_admin_once(&mut store, None).unwrap());
        assert!(reset_admin_once(&mut store, Some("bad")).is_err());
        assert_eq!(store.map, before);
        assert!(reset_admin_once(&mut store, Some(ID)).unwrap());
        assert_eq!(store.get("admin").unwrap(), None);
        assert_eq!(store.get("b.llm").unwrap(), before.get("b.llm").cloned());
        assert_eq!(
            store.get("i.openrouter").unwrap(),
            before.get("i.openrouter").cloned()
        );
        let verifier = serde_json::to_string(&Verifier::new("1234").unwrap()).unwrap();
        store.put("admin", &verifier).unwrap();
        assert!(!reset_admin_once(&mut store, Some(ID)).unwrap());
        assert!(!reset_admin_once(&mut store, None).unwrap());
        assert_eq!(
            store.get("admin").unwrap().as_deref(),
            Some(verifier.as_str())
        );
    }

    #[test]
    fn marker_write_failure_is_reported_and_retried_before_serving_setup() {
        struct Failing(MemStore, bool);
        impl KvStore for Failing {
            fn get(&self, key: &str) -> Result<Option<String>, String> {
                self.0.get(key)
            }
            fn put(&mut self, key: &str, value: &str) -> Result<(), String> {
                if self.1 {
                    return Err("write failed".into());
                }
                self.0.put(key, value)
            }
            fn delete(&mut self, key: &str) -> Result<(), String> {
                self.0.delete(key)
            }
        }
        let mut store = Failing(MemStore::default(), true);
        assert!(reset_admin_once(&mut store, Some(ID)).is_err());
        store.1 = false;
        assert!(reset_admin_once(&mut store, Some(ID)).unwrap());
        assert!(!reset_admin_once(&mut store, Some(ID)).unwrap());
    }
}
