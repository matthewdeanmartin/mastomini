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
use std::ffi::{CStr, CString};

pub const PARTITION: &CStr = c"store";

pub struct NvsStore {
    /// Indexed like `Ns::ALL`.
    handles: Vec<EspNvs<NvsCustom>>,
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

    fn commit(&self, ns: Ns) -> Result<(), StoreError> {
        // SAFETY: the handle is owned by `self` and open for writing.
        sys::esp!(unsafe { sys::nvs_commit(self.handle(ns).handle()) }).map_err(io)
    }

    /// Keys of blob entries in a namespace, sorted.
    fn keys(&self, ns: Ns) -> Result<Vec<Key>, StoreError> {
        let namespace = CString::new(ns.name()).map_err(|_| StoreError::BadKey)?;
        let mut keys = Vec::new();
        let mut it: sys::nvs_iterator_t = core::ptr::null_mut();
        // SAFETY: NUL-terminated names; the iterator is released below on
        // every path that obtained one.
        let found = unsafe {
            sys::nvs_entry_find(
                PARTITION.as_ptr(),
                namespace.as_ptr(),
                sys::nvs_type_t_NVS_TYPE_BLOB,
                &mut it,
            )
        };
        if found == sys::ESP_ERR_NVS_NOT_FOUND {
            return Ok(keys);
        }
        sys::esp!(found).map_err(io)?;
        let result = (|| loop {
            let mut info = sys::nvs_entry_info_t::default();
            // SAFETY: `it` is a live iterator from nvs_entry_find/next.
            sys::esp!(unsafe { sys::nvs_entry_info(it, &mut info) }).map_err(io)?;
            // SAFETY: NVS NUL-terminates key names within the 16-byte field.
            let name = unsafe { CStr::from_ptr(info.key.as_ptr()) };
            let key = Key::new(name.to_str().map_err(|_| StoreError::BadKey)?)?;
            keys.push(key);
            // SAFETY: as above; on the last entry NVS sets `it` to null.
            let next = unsafe { sys::nvs_entry_next(&mut it) };
            if next == sys::ESP_ERR_NVS_NOT_FOUND || it.is_null() {
                return Ok(());
            }
            sys::esp!(next).map_err(io)?;
        })();
        // SAFETY: releasing null is allowed; otherwise `it` is live.
        unsafe { sys::nvs_release_iterator(it) };
        result?;
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
        let mut buf = vec![0; len];
        let value = handle.get_blob(key.as_str(), &mut buf).map_err(io)?;
        Ok(value.map(<[u8]>::to_vec))
    }

    fn set(&mut self, ns: Ns, key: &Key, value: &[u8]) -> Result<(), StoreError> {
        if value.len() > VALUE_MAX {
            return Err(StoreError::ValueTooLarge);
        }
        self.handle(ns).set_blob(key.as_str(), value).map_err(io)?;
        self.commit(ns)
    }

    fn erase(&mut self, ns: Ns, key: &Key) -> Result<(), StoreError> {
        self.handle(ns).remove(key.as_str()).map_err(io)?;
        self.commit(ns)
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
