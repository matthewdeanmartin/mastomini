//! [`Store`] on ESP-IDF NVS (spec/02-storage.md "Store design").
//!
//! One NVS namespace per [`Ns`]. Every `set`/`erase` is committed before it
//! returns; NVS makes each one atomic with respect to power loss and handles
//! wear levelling. Nothing here ever erases or formats the partition.

use esp_idf_svc::{
    handle::RawHandle,
    nvs::{EspNvs, EspNvsPartition, NvsCustom},
    sys::{self, EspError},
};
use mastomini::store::{Key, Ns, Store, StoreError, StoreStats, Visitor, VALUE_MAX};
use std::ffi::CStr;

pub const PARTITION: &CStr = c"store";

pub struct NvsStore {
    /// Indexed like `Ns::ALL`.
    handles: Vec<EspNvs<NvsCustom>>,
}

/// Pinned svc's safe iterator ignores advance errors; Store requires failures
/// to propagate, so retain only this fallible SDK boundary.
struct Entries(sys::nvs_iterator_t);
impl Drop for Entries {
    fn drop(&mut self) {
        // SAFETY: exclusively owned iterator, or null (accepted by IDF).
        unsafe { sys::nvs_release_iterator(self.0) };
    }
}

fn io(e: EspError) -> StoreError {
    if e.code() == sys::ESP_ERR_NVS_NOT_ENOUGH_SPACE {
        StoreError::Full
    } else {
        StoreError::Io(format!("NVS: {e}"))
    }
}

impl NvsStore {
    pub fn open(partition: EspNvsPartition<NvsCustom>) -> Result<NvsStore, StoreError> {
        let handles = Ns::ALL
            .iter()
            .map(|ns| EspNvs::new(partition.clone(), ns.name(), true).map_err(io))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(NvsStore { handles })
    }

    fn handle(&self, ns: Ns) -> &EspNvs<NvsCustom> {
        &self.handles[ns as usize]
    }

    /// Keys of blob entries in a namespace, sorted.
    fn keys(&self, ns: Ns) -> Result<Vec<Key>, StoreError> {
        let mut keys = Vec::new();
        let mut it = Entries(core::ptr::null_mut());
        // SAFETY: owned open namespace handle; initialized iterator output.
        let found = unsafe {
            sys::nvs_entry_find_in_handle(
                self.handle(ns).handle(),
                sys::nvs_type_t_NVS_TYPE_BLOB,
                &mut it.0,
            )
        };
        if found == sys::ESP_ERR_NVS_NOT_FOUND {
            return Ok(keys);
        }
        sys::esp!(found).map_err(io)?;
        loop {
            if it.0.is_null() {
                return Err(StoreError::Corrupt(
                    "NVS returned a null live iterator".into(),
                ));
            }
            let mut info = sys::nvs_entry_info_t::default();
            // SAFETY: `it` is a live iterator from nvs_entry_find/next.
            sys::esp!(unsafe { sys::nvs_entry_info(it.0, &mut info) }).map_err(io)?;
            let bytes = info.key.map(|b| b as u8);
            let name = CStr::from_bytes_until_nul(&bytes).map_err(|_| StoreError::BadKey)?;
            let key = Key::new(name.to_str().map_err(|_| StoreError::BadKey)?)?;
            keys.push(key);
            // SAFETY: as above; on the last entry NVS sets `it` to null.
            let next = unsafe { sys::nvs_entry_next(&mut it.0) };
            if next == sys::ESP_ERR_NVS_NOT_FOUND {
                break;
            }
            sys::esp!(next).map_err(io)?;
        }
        keys.sort();
        Ok(keys)
    }
}

impl Store for NvsStore {
    fn get(&mut self, ns: Ns, key: &Key) -> Result<Option<Vec<u8>>, StoreError> {
        let handle = self.handle(ns);
        let Some(len) = handle.blob_len(key.as_str()).map_err(io)? else {
            return Ok(None);
        };
        mastomini::store::read_blob(len, |buf| {
            handle
                .get_blob(key.as_str(), buf)
                .map(|value| value.map(<[u8]>::len))
                .map_err(io)
        })
    }

    fn set(&mut self, ns: Ns, key: &Key, value: &[u8]) -> Result<(), StoreError> {
        if value.len() > VALUE_MAX {
            return Err(StoreError::ValueTooLarge);
        }
        // svc 0.52.1 commits inside set_blob.
        self.handle(ns).set_blob(key.as_str(), value).map_err(io)
    }

    fn erase(&mut self, ns: Ns, key: &Key) -> Result<(), StoreError> {
        self.handle(ns).remove(key.as_str()).map(|_| ()).map_err(io)
    }

    fn for_each(&mut self, ns: Ns, visit: &mut Visitor<'_>) -> Result<(), StoreError> {
        for key in self.keys(ns)? {
            if let Some(value) = self.get(ns, &key)? {
                visit(&key, &value)?;
            }
        }
        Ok(())
    }

    fn stats(&mut self) -> Result<StoreStats, StoreError> {
        let mut stats = sys::nvs_stats_t::default();
        // SAFETY: NUL-terminated partition name and a live out-pointer.
        sys::esp!(unsafe { sys::nvs_get_stats(PARTITION.as_ptr(), &mut stats) }).map_err(io)?;
        Ok(StoreStats {
            used_entries: stats.used_entries,
            total_entries: stats.total_entries,
        })
    }
}
