//! In-memory store and a fault-injecting wrapper for power-cut tests.

use super::{entries_for, Key, Ns, Store, StoreError, StoreStats, Visitor, VALUE_MAX};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct MemStore {
    map: BTreeMap<(Ns, Key), Vec<u8>>,
    used_entries: usize,
    total_entries: usize,
}

impl Default for MemStore {
    fn default() -> Self {
        MemStore::with_capacity(super::NVS_8MIB_ENTRIES)
    }
}

impl MemStore {
    /// A store whose `stats` reports `total_entries` of capacity, for testing
    /// the low-watermark eviction path without writing megabytes.
    pub fn with_capacity(total_entries: usize) -> MemStore {
        MemStore {
            map: BTreeMap::new(),
            used_entries: 0,
            total_entries,
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn keys(&self, ns: Ns) -> Vec<String> {
        self.map
            .keys()
            .filter(|(n, _)| *n == ns)
            .map(|(_, k)| k.as_str().to_string())
            .collect()
    }

    /// Would writing `value_len` bytes under `key` fit?
    pub fn check_fits(&self, ns: Ns, key: &Key, value_len: usize) -> Result<(), StoreError> {
        if value_len > VALUE_MAX {
            return Err(StoreError::ValueTooLarge);
        }
        let old = self
            .map
            .get(&(ns, *key))
            .map_or(0, |v| entries_for(v.len()));
        if self.used_entries - old + entries_for(value_len) > self.total_entries {
            return Err(StoreError::Full);
        }
        Ok(())
    }
}

impl Store for MemStore {
    fn get(&mut self, ns: Ns, key: &Key) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.map.get(&(ns, *key)).cloned())
    }

    fn set(&mut self, ns: Ns, key: &Key, value: &[u8]) -> Result<(), StoreError> {
        self.check_fits(ns, key, value.len())?;
        self.used_entries += entries_for(value.len());
        if let Some(old) = self.map.insert((ns, *key), value.to_vec()) {
            self.used_entries -= entries_for(old.len());
        }
        Ok(())
    }

    fn erase(&mut self, ns: Ns, key: &Key) -> Result<(), StoreError> {
        if let Some(old) = self.map.remove(&(ns, *key)) {
            self.used_entries -= entries_for(old.len());
        }
        Ok(())
    }

    fn for_each(&mut self, ns: Ns, visit: &mut Visitor<'_>) -> Result<(), StoreError> {
        for ((n, key), value) in self.map.range((ns, Key::new("!")?)..) {
            if *n != ns {
                break;
            }
            visit(key, value)?;
        }
        Ok(())
    }

    fn stats(&mut self) -> Result<StoreStats, StoreError> {
        Ok(StoreStats {
            used_entries: self.used_entries,
            total_entries: self.total_entries,
        })
    }
}

/// Simulates losing power: once `budget` mutating operations have completed,
/// every later operation fails and nothing more reaches the inner store.
/// Reads keep working until the cut so the service can be exercised normally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// Fail before a mutation; no change reaches the backing store.
    Write,
    /// Complete a mutation, then lose the acknowledgement.
    PersistThenError,
    /// Reject a mutation without latching the simulated power cut.
    Full,
    Read,
    Enumerate,
    /// Fail after visiting a prefix of a namespace.
    Visit,
    Stats,
}

#[derive(Debug)]
pub struct FaultStore<S: Store> {
    pub inner: S,
    budget: Option<usize>,
    pub writes: usize,
    cut: bool,
    failure: Option<(Failure, usize)>,
}

impl<S: Store> FaultStore<S> {
    pub fn new(inner: S) -> FaultStore<S> {
        FaultStore {
            inner,
            budget: None,
            writes: 0,
            cut: false,
            failure: None,
        }
    }

    /// Allow `writes` more mutating operations, then cut power.
    pub fn cut_after(&mut self, writes: usize) {
        self.budget = Some(self.writes + writes);
    }

    /// Fail after `successful` matching operations; other operations are unaffected.
    pub fn fail_after(&mut self, failure: Failure, successful: usize) {
        self.failure = Some((failure, successful));
    }

    fn inject(
        state: &mut Option<(Failure, usize)>,
        cut: &mut bool,
        at: Failure,
    ) -> Result<(), StoreError> {
        if *cut {
            return Err(StoreError::Io("power cut".into()));
        }
        let Some((failure, remaining)) = state.as_mut() else {
            return Ok(());
        };
        if *failure != at {
            return Ok(());
        }
        if *remaining > 0 {
            *remaining -= 1;
            return Ok(());
        }
        *state = None;
        if at == Failure::Full {
            return Err(StoreError::Full);
        }
        *cut = true;
        Err(StoreError::Io(format!("injected {at:?}")))
    }

    fn fault(&mut self, at: Failure) -> Result<(), StoreError> {
        Self::inject(&mut self.failure, &mut self.cut, at)
    }

    pub fn is_cut(&self) -> bool {
        self.cut
    }

    fn admit(&mut self) -> Result<(), StoreError> {
        if self.cut {
            return Err(StoreError::Io("power cut".into()));
        }
        if let Some(budget) = self.budget {
            if self.writes >= budget {
                self.cut = true;
                return Err(StoreError::Io("power cut".into()));
            }
        }
        self.writes += 1;
        Ok(())
    }
}

impl<S: Store> Store for FaultStore<S> {
    fn get(&mut self, ns: Ns, key: &Key) -> Result<Option<Vec<u8>>, StoreError> {
        self.fault(Failure::Read)?;
        self.inner.get(ns, key)
    }

    fn set(&mut self, ns: Ns, key: &Key, value: &[u8]) -> Result<(), StoreError> {
        self.admit()?;
        self.fault(Failure::Write)?;
        self.fault(Failure::Full)?;
        self.inner.set(ns, key, value)?;
        self.fault(Failure::PersistThenError)
    }

    fn erase(&mut self, ns: Ns, key: &Key) -> Result<(), StoreError> {
        self.admit()?;
        self.fault(Failure::Write)?;
        self.fault(Failure::Full)?;
        self.inner.erase(ns, key)?;
        self.fault(Failure::PersistThenError)
    }

    fn for_each(&mut self, ns: Ns, visit: &mut Visitor<'_>) -> Result<(), StoreError> {
        self.fault(Failure::Enumerate)?;
        let failure = &mut self.failure;
        let cut = &mut self.cut;
        self.inner.for_each(ns, &mut |key, value| {
            Self::inject(failure, cut, Failure::Visit)?;
            visit(key, value)
        })
    }

    fn stats(&mut self) -> Result<StoreStats, StoreError> {
        self.fault(Failure::Stats)?;
        self.inner.stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lost_acknowledgements_persist_and_full_does_not() {
        let key = Key::new("a").unwrap();
        for erase in [false, true] {
            let mut fault = FaultStore::new(MemStore::default());
            fault.inner.set(Ns::Cfg, &key, b"old").unwrap();
            fault.fail_after(Failure::PersistThenError, 0);
            let result = if erase {
                fault.erase(Ns::Cfg, &key)
            } else {
                fault.set(Ns::Cfg, &key, b"new")
            };
            assert!(matches!(result, Err(StoreError::Io(_))));
            assert_eq!(
                fault.inner.get(Ns::Cfg, &key).unwrap(),
                if erase { None } else { Some(b"new".to_vec()) }
            );
            assert!(fault.stats().is_err());
        }
        let mut fault = FaultStore::new(MemStore::default());
        fault.fail_after(Failure::Full, 0);
        assert_eq!(fault.set(Ns::Cfg, &key, b"no"), Err(StoreError::Full));
        assert_eq!(fault.get(Ns::Cfg, &key).unwrap(), None);
        fault.set(Ns::Cfg, &key, b"retry").unwrap();
    }

    #[test]
    fn namespaces_are_isolated_and_ordered() {
        let mut store = MemStore::default();
        store.set(Ns::Stat, &Key::new("s2").unwrap(), b"b").unwrap();
        store.set(Ns::Stat, &Key::new("s1").unwrap(), b"a").unwrap();
        store.set(Ns::Acct, &Key::new("a0").unwrap(), b"x").unwrap();
        let mut seen = Vec::new();
        store
            .for_each(Ns::Stat, &mut |k, v| {
                seen.push((k.as_str().to_string(), v.to_vec()));
                Ok(())
            })
            .unwrap();
        assert_eq!(
            seen,
            vec![("s1".into(), b"a".to_vec()), ("s2".into(), b"b".to_vec())]
        );
    }

    #[test]
    fn capacity_is_enforced() {
        let mut store = MemStore::with_capacity(4);
        store
            .set(Ns::Cfg, &Key::new("a").unwrap(), &[0; 32])
            .unwrap();
        assert_eq!(
            store.set(Ns::Cfg, &Key::new("b").unwrap(), &[0; 32]),
            Err(StoreError::Full)
        );
    }

    #[test]
    fn fault_store_cuts_power() {
        let mut store = FaultStore::new(MemStore::default());
        store.cut_after(1);
        store.set(Ns::Cfg, &Key::new("a").unwrap(), b"1").unwrap();
        assert!(store.set(Ns::Cfg, &Key::new("b").unwrap(), b"2").is_err());
        assert!(store.is_cut());
        assert_eq!(store.inner.len(), 1);
    }
}
