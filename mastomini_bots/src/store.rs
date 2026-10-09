//! Small durable settings: the admin password verifier and each bot's
//! configuration and last run. Values are JSON text under short keys (NVS
//! keys are at most 15 bytes). Written rarely: on configuration changes and
//! once per run, never per request.

use std::collections::BTreeMap;

pub trait KvStore: Send {
    fn get(&self, key: &str) -> Result<Option<String>, String>;
    fn put(&mut self, key: &str, value: &str) -> Result<(), String>;
    fn delete(&mut self, key: &str) -> Result<(), String>;
}

#[derive(Debug, Default, Clone)]
pub struct MemStore {
    pub map: BTreeMap<String, String>,
}

impl KvStore for MemStore {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.map.get(key).cloned())
    }
    fn put(&mut self, key: &str, value: &str) -> Result<(), String> {
        self.map.insert(key.to_string(), value.to_string());
        Ok(())
    }
    fn delete(&mut self, key: &str) -> Result<(), String> {
        self.map.remove(key);
        Ok(())
    }
}

/// Desktop: one JSON file, replaced atomically on every write.
#[cfg(feature = "desktop")]
pub struct FileStore {
    path: std::path::PathBuf,
    map: BTreeMap<String, String>,
}

#[cfg(feature = "desktop")]
impl FileStore {
    pub fn open(path: impl Into<std::path::PathBuf>) -> Result<FileStore, String> {
        let path = path.into();
        let map = match std::fs::read_to_string(&path) {
            Ok(text) => {
                serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        Ok(FileStore { path, map })
    }

    fn save(&self) -> Result<(), String> {
        let tmp = self.path.with_extension("tmp");
        let text = serde_json::to_string_pretty(&self.map).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("{}: {e}", self.path.display()))
    }
}

#[cfg(feature = "desktop")]
impl KvStore for FileStore {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.map.get(key).cloned())
    }
    fn put(&mut self, key: &str, value: &str) -> Result<(), String> {
        let previous = self.map.insert(key.to_string(), value.to_string());
        if let Err(error) = self.save() {
            match previous {
                Some(value) => {
                    self.map.insert(key.into(), value);
                }
                None => {
                    self.map.remove(key);
                }
            }
            return Err(error);
        }
        Ok(())
    }
    fn delete(&mut self, key: &str) -> Result<(), String> {
        if let Some(previous) = self.map.remove(key) {
            if let Err(error) = self.save() {
                self.map.insert(key.into(), previous);
                return Err(error);
            }
        }
        Ok(())
    }
}

#[cfg(all(test, feature = "desktop"))]
mod tests {
    use super::*;

    #[test]
    fn file_store_round_trips_replaces_and_deletes_without_losing_other_keys() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.json");
        let mut store = FileStore::open(&path).unwrap();
        assert_eq!(store.get("missing").unwrap(), None);
        store.put("one", "first").unwrap();
        store.put("two", "second").unwrap();
        store.put("one", "replacement").unwrap();
        let mut reopened = FileStore::open(&path).unwrap();
        assert_eq!(reopened.get("one").unwrap().as_deref(), Some("replacement"));
        reopened.delete("one").unwrap();
        reopened.delete("missing").unwrap();
        let reopened = FileStore::open(path).unwrap();
        assert_eq!(reopened.get("one").unwrap(), None);
        assert_eq!(reopened.get("two").unwrap().as_deref(), Some("second"));
    }

    #[test]
    fn failed_writes_do_not_change_the_live_or_durable_store() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.json");
        let mut store = FileStore::open(&path).unwrap();
        store.put("key", "old").unwrap();
        std::fs::create_dir(path.with_extension("tmp")).unwrap();
        assert!(store.put("key", "new").is_err());
        assert!(store.put("fresh", "new").is_err());
        assert!(store.delete("key").is_err());
        assert_eq!(store.get("key").unwrap().as_deref(), Some("old"));
        assert_eq!(store.get("fresh").unwrap(), None);
        assert_eq!(
            FileStore::open(&path)
                .unwrap()
                .get("key")
                .unwrap()
                .as_deref(),
            Some("old")
        );
    }

    #[test]
    fn corrupt_or_unreadable_files_are_reported_instead_of_reset() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.json");
        std::fs::write(&path, "not JSON").unwrap();
        assert!(FileStore::open(&path).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "not JSON");
        assert!(FileStore::open(temp.path()).is_err());
    }
}
