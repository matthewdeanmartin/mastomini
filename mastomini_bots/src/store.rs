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
        self.map.insert(key.to_string(), value.to_string());
        self.save()
    }
    fn delete(&mut self, key: &str) -> Result<(), String> {
        if self.map.remove(key).is_some() {
            self.save()?;
        }
        Ok(())
    }
}
