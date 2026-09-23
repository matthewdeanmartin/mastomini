//! Desktop store: an append-only operation log with a CRC per record.
//!
//! On open the log is replayed into memory and rewritten compactly (live keys
//! only) through a temporary file and an atomic rename. An incomplete final
//! record, left by a crash mid-append, is dropped. Any other corruption fails
//! the open instead of guessing. A sibling `.lock` file holds an exclusive
//! lock so two servers cannot write one store.

use super::mem::MemStore;
use super::{Key, Ns, Store, StoreError, StoreStats, Visitor};
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"MMSTORE1";
const OP_SET: u8 = 1;
const OP_ERASE: u8 = 2;

pub struct FileStore {
    path: PathBuf,
    file: File,
    mem: MemStore,
    _lock: File,
}

fn io(e: std::io::Error) -> StoreError {
    StoreError::Io(e.to_string())
}

impl FileStore {
    pub fn open(path: impl AsRef<Path>) -> Result<FileStore, StoreError> {
        let path = path.as_ref().to_path_buf();
        let lock_path = path.with_extension("lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(io)?;
        lock.try_lock_exclusive().map_err(|_| {
            StoreError::Io(format!(
                "{} is locked by another mastomini process",
                path.display()
            ))
        })?;

        let mut mem = MemStore::default();
        if path.exists() {
            let mut bytes = Vec::new();
            File::open(&path)
                .map_err(io)?
                .read_to_end(&mut bytes)
                .map_err(io)?;
            replay(&bytes, &mut mem)?;
        }
        compact(&path, &mut mem)?;
        let file = OpenOptions::new().append(true).open(&path).map_err(io)?;
        Ok(FileStore {
            path,
            file,
            mem,
            _lock: lock,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn append(&mut self, op: u8, ns: Ns, key: &Key, value: &[u8]) -> Result<(), StoreError> {
        let payload = encode(op, ns, key, value);
        let mut record = Vec::with_capacity(payload.len() + 8);
        record.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        record.extend_from_slice(&crc32fast::hash(&payload).to_le_bytes());
        record.extend_from_slice(&payload);
        self.file.write_all(&record).map_err(io)?;
        self.file.sync_data().map_err(io)
    }
}

fn encode(op: u8, ns: Ns, key: &Key, value: &[u8]) -> Vec<u8> {
    let key = key.as_str().as_bytes();
    let mut out = Vec::with_capacity(8 + key.len() + value.len());
    out.push(op);
    out.push(ns as u8);
    out.push(key.len() as u8);
    out.extend_from_slice(key);
    out.extend_from_slice(&(value.len() as u32).to_le_bytes());
    out.extend_from_slice(value);
    out
}

fn corrupt(offset: usize, what: &str) -> StoreError {
    StoreError::Corrupt(format!("{what} at byte {offset}"))
}

fn replay(bytes: &[u8], mem: &mut MemStore) -> Result<(), StoreError> {
    if bytes.len() < MAGIC.len() {
        // A crash while creating the file: nothing was ever acknowledged.
        return Ok(());
    }
    if &bytes[..MAGIC.len()] != MAGIC {
        return Err(corrupt(0, "bad magic"));
    }
    let mut pos = MAGIC.len();
    while pos < bytes.len() {
        let header_end = pos + 8;
        if header_end > bytes.len() {
            return Ok(()); // torn final header
        }
        let len = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap_or([0; 4])) as usize;
        let crc = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap_or([0; 4]));
        let end = header_end + len;
        if end > bytes.len() {
            return Ok(()); // torn final record
        }
        let payload = &bytes[header_end..end];
        if crc32fast::hash(payload) != crc {
            if end == bytes.len() {
                return Ok(()); // torn final record with a complete length
            }
            return Err(corrupt(pos, "checksum mismatch"));
        }
        apply(payload, mem).map_err(|_| corrupt(pos, "malformed record"))?;
        pos = end;
    }
    Ok(())
}

fn apply(payload: &[u8], mem: &mut MemStore) -> Result<(), StoreError> {
    let bad = || StoreError::Corrupt("record".into());
    let (&op, rest) = payload.split_first().ok_or_else(bad)?;
    let (&ns, rest) = rest.split_first().ok_or_else(bad)?;
    let (&klen, rest) = rest.split_first().ok_or_else(bad)?;
    let ns = Ns::from_u8(ns).ok_or_else(bad)?;
    let key_bytes = rest.get(..klen as usize).ok_or_else(bad)?;
    let key = Key::new(core::str::from_utf8(key_bytes).map_err(|_| bad())?)?;
    let rest = &rest[klen as usize..];
    let vlen = u32::from_le_bytes(
        rest.get(..4)
            .ok_or_else(bad)?
            .try_into()
            .map_err(|_| bad())?,
    ) as usize;
    let value = rest.get(4..4 + vlen).ok_or_else(bad)?;
    match op {
        OP_SET => mem.set(ns, &key, value),
        OP_ERASE => mem.erase(ns, &key),
        _ => Err(bad()),
    }
}

fn compact(path: &Path, mem: &mut MemStore) -> Result<(), StoreError> {
    let tmp = path.with_extension("compact");
    {
        let mut out = File::create(&tmp).map_err(io)?;
        let mut buf = Vec::new();
        buf.extend_from_slice(MAGIC);
        for ns in Ns::ALL {
            mem.for_each(ns, &mut |key, value| {
                let payload = encode(OP_SET, ns, key, value);
                buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                buf.extend_from_slice(&crc32fast::hash(&payload).to_le_bytes());
                buf.extend_from_slice(&payload);
                Ok(())
            })?;
        }
        out.write_all(&buf).map_err(io)?;
        out.sync_all().map_err(io)?;
    }
    // Replaces the old log atomically on both Windows and Unix.
    std::fs::rename(&tmp, path).map_err(io)
}

impl Store for FileStore {
    fn get(&mut self, ns: Ns, key: &Key) -> Result<Option<Vec<u8>>, StoreError> {
        self.mem.get(ns, key)
    }

    fn set(&mut self, ns: Ns, key: &Key, value: &[u8]) -> Result<(), StoreError> {
        // Check capacity against the in-memory model before touching the log.
        self.mem.check_fits(ns, key, value.len())?;
        self.append(OP_SET, ns, key, value)?;
        self.mem.set(ns, key, value)
    }

    fn erase(&mut self, ns: Ns, key: &Key) -> Result<(), StoreError> {
        if self.mem.get(ns, key)?.is_none() {
            return Ok(());
        }
        self.append(OP_ERASE, ns, key, &[])?;
        self.mem.erase(ns, key)
    }

    fn for_each(&mut self, ns: Ns, visit: &mut Visitor<'_>) -> Result<(), StoreError> {
        self.mem.for_each(ns, visit)
    }

    fn stats(&mut self) -> Result<StoreStats, StoreError> {
        self.mem.stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mastomini-filestore-{}-{}",
            name,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("store.log")
    }

    #[test]
    fn survives_reopen_and_compacts() {
        let path = temp_path("reopen");
        {
            let mut store = FileStore::open(&path).unwrap();
            for i in 0..10 {
                store
                    .set(Ns::Stat, &Key::new("s1").unwrap(), &[i; 10])
                    .unwrap();
            }
            store.set(Ns::Stat, &Key::new("s2").unwrap(), b"x").unwrap();
            store.erase(Ns::Stat, &Key::new("s2").unwrap()).unwrap();
        }
        let before = std::fs::metadata(&path).unwrap().len();
        let mut store = FileStore::open(&path).unwrap();
        assert_eq!(
            store.get(Ns::Stat, &Key::new("s1").unwrap()).unwrap(),
            Some(vec![9; 10])
        );
        assert_eq!(store.get(Ns::Stat, &Key::new("s2").unwrap()).unwrap(), None);
        assert!(std::fs::metadata(&path).unwrap().len() < before);
    }

    #[test]
    fn torn_tail_is_dropped_but_middle_corruption_fails() {
        let path = temp_path("torn");
        {
            let mut store = FileStore::open(&path).unwrap();
            store.set(Ns::Cfg, &Key::new("a").unwrap(), b"one").unwrap();
            store.set(Ns::Cfg, &Key::new("b").unwrap(), b"two").unwrap();
        }
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.truncate(bytes.len() - 2);
        std::fs::write(&path, &bytes).unwrap();
        {
            let mut store = FileStore::open(&path).unwrap();
            assert!(store
                .get(Ns::Cfg, &Key::new("a").unwrap())
                .unwrap()
                .is_some());
            assert!(store
                .get(Ns::Cfg, &Key::new("b").unwrap())
                .unwrap()
                .is_none());
            store
                .set(Ns::Cfg, &Key::new("c").unwrap(), b"three")
                .unwrap();
        }
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[MAGIC.len() + 9] ^= 0xff; // inside the first record
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            FileStore::open(&path),
            Err(StoreError::Corrupt(_))
        ));
    }

    #[test]
    fn second_open_is_refused() {
        let path = temp_path("lock");
        let _first = FileStore::open(&path).unwrap();
        assert!(matches!(FileStore::open(&path), Err(StoreError::Io(_))));
    }
}
