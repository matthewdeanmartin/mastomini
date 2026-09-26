//! [`KvStore`] on an NVS partition (`store`, see partitions.csv): one
//! namespace, JSON strings. NVS commits each write atomically and levels
//! wear; values are written only when settings change and once per run.

use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsCustom};
use mastobots::store::KvStore;
use std::ffi::CStr;

pub const PARTITION: &CStr = c"store";
const NAMESPACE: &str = "bots";
/// NVS strings may be up to 4000 bytes; a bot record is well under 1 KiB.
const VALUE_MAX: usize = 4000;

pub struct NvsKv {
    nvs: EspNvs<NvsCustom>,
}

impl NvsKv {
    pub fn open(partition: EspNvsPartition<NvsCustom>) -> Result<NvsKv, String> {
        let nvs = EspNvs::new(partition, NAMESPACE, true).map_err(|e| format!("NVS: {e}"))?;
        Ok(NvsKv { nvs })
    }
}

impl KvStore for NvsKv {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        let mut buf = vec![0u8; VALUE_MAX + 1];
        match self.nvs.get_str(key, &mut buf) {
            Ok(value) => Ok(value.map(str::to_string)),
            Err(e) => Err(format!("NVS get {key}: {e}")),
        }
    }

    fn put(&mut self, key: &str, value: &str) -> Result<(), String> {
        if value.len() > VALUE_MAX {
            return Err(format!(
                "{key}: {} bytes is too large to store",
                value.len()
            ));
        }
        self.nvs
            .set_str(key, value)
            .map_err(|e| format!("NVS set {key}: {e}"))
    }

    fn delete(&mut self, key: &str) -> Result<(), String> {
        self.nvs
            .remove(key)
            .map(|_| ())
            .map_err(|e| format!("NVS remove {key}: {e}"))
    }
}
