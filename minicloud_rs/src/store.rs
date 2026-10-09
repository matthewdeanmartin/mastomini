use crate::{Error, Result, State};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub const MAX_BLOB: u64 = 256 * 1024;
pub const BLOB_BUDGET: u64 = 3 * 1024 * 1024;
pub const MAX_STATE: u64 = 64 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Blob {
    pub bucket: String,
    pub key: String,
    pub mime: String,
    pub size: u64,
    pub hash: String,
    #[serde(default)]
    pub uploader: Option<String>,
    #[serde(default)]
    pub tier: Tier,
}
impl Blob {
    pub fn url(&self) -> String {
        format!("/blobs/{}/{}", self.bucket, self.key)
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    #[default]
    Onboard,
    Card,
}

pub struct Store {
    pub root: PathBuf,
    pub card: Option<PathBuf>,
}

pub fn object_name(bucket: &str, key: &str) -> Result<()> {
    if !crate::name(bucket) || key.len() > 128 || key.split('/').any(|part| !crate::name(part)) {
        return Err(Error::new(400, "invalid bucket/key; use ASCII letters, numbers, dots, dashes, underscores and key folders"));
    }
    Ok(())
}
pub fn mime_valid(mime: &str) -> bool {
    mime.len() <= 96
        && mime
            .split_once('/')
            .is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
        && mime
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-+.;= ".contains(&b))
}

struct HashedWriter<'a> {
    file: &'a mut File,
    digest: Sha256,
    written: u64,
}
impl Write for HashedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.written + bytes.len() as u64 > MAX_STATE {
            return Err(std::io::Error::other("state budget exhausted"));
        }
        let count = self.file.write(bytes)?;
        self.digest.update(&bytes[..count]);
        self.written += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        // SPIFFS is flat and does not implement mkdir/stat for its mount root.
        // The board adapter mounts /storage before opening the store.
        #[cfg(not(target_os = "espidf"))]
        fs::create_dir_all(root)?;
        let card = std::env::var("MINICLOUD_CARD_PATH")
            .ok()
            .or_else(|| option_env!("MINICLOUD_CARD_PATH").map(str::to_owned))
            .map(PathBuf::from);
        #[cfg(target_os = "espidf")]
        let card = card.or_else(|| {
            Path::new("/sdcard/minicloud")
                .is_dir()
                .then(|| PathBuf::from("/sdcard/minicloud"))
        });
        Self::with_card(root, card)
    }
    pub fn with_card(root: &Path, card: Option<PathBuf>) -> Result<Self> {
        if let Some(path) = &card {
            // SPIFFS cannot stat/resolve its mount root; desktop paths can alias
            // through symlinks and must be compared after canonicalization.
            #[cfg(not(target_os = "espidf"))]
            let same_root = fs::canonicalize(path)? == fs::canonicalize(root)?;
            #[cfg(target_os = "espidf")]
            let same_root = path == root;
            if !path.is_dir() || same_root {
                return Err(Error::new(
                    503,
                    "card path must be an existing, separate mounted directory",
                ));
            }
        }
        Ok(Self {
            root: root.into(),
            card,
        })
    }
    pub fn path_for(&self, blob: &Blob) -> Result<PathBuf> {
        match blob.tier {
            Tier::Onboard => Ok(self.blob_path(&blob.hash)),
            Tier::Card => Ok(self
                .card
                .as_ref()
                .filter(|p| p.is_dir())
                .ok_or_else(|| Error::new(503, "card storage unavailable"))?
                .join(format!("b-{}", &blob.hash[..28]))),
        }
    }
    pub fn migrate(&self, blob: &Blob, tier: Tier) -> Result<Blob> {
        let mut next = blob.clone();
        next.tier = tier;
        if blob.tier == tier {
            return Ok(next);
        }
        let destination = self.path_for(&next)?;
        let mut source = self.open_blob(blob)?;
        let staging = destination.with_file_name("migrate.tmp");
        let result = (|| -> Result<()> {
            let mut file = File::create(&staging)?;
            let mut digest = Sha256::new();
            let mut total = 0;
            let mut chunk = [0; 4096];
            loop {
                let n = source.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                digest.update(&chunk[..n]);
                file.write_all(&chunk[..n])?;
                total += n as u64;
            }
            file.sync_all()?;
            drop(file);
            if total != blob.size || format!("{:x}", digest.finalize()) != blob.hash {
                return Err(Error::new(507, "migration source checksum mismatch"));
            }
            if destination.exists() {
                let mut existing = File::open(&destination)?;
                let mut digest = Sha256::new();
                loop {
                    let n = existing.read(&mut chunk)?;
                    if n == 0 {
                        break;
                    }
                    digest.update(&chunk[..n]);
                }
                if format!("{:x}", digest.finalize()) != blob.hash {
                    return Err(Error::new(409, "destination checksum mismatch"));
                }
                fs::remove_file(&staging)?;
            } else {
                fs::rename(&staging, &destination)?;
            }
            #[cfg(not(target_os = "espidf"))]
            if let Some(parent) = destination.parent() {
                #[cfg(unix)]
                File::open(parent)?.sync_all()?;
                let _ = parent;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(staging);
        }
        result?;
        Ok(next)
    }
    // SPIFFS defaults to 32-byte names. Keep 112 bits in filenames and the
    // complete SHA-256 in metadata; detect a prefix collision before reuse.
    pub fn blob_path(&self, hash: &str) -> PathBuf {
        self.root.join(format!("b-{}", &hash[..28]))
    }
    pub fn open_blob(&self, blob: &Blob) -> Result<File> {
        Ok(File::open(self.path_for(blob)?)?)
    }
    fn slot(&self, index: u64) -> PathBuf {
        self.root.join(format!("state-{}.json", index % 2))
    }
    pub fn load(&self) -> Result<State> {
        let mut valid = Vec::new();
        let mut present = false;
        for index in 0..2 {
            let path = self.slot(index);
            if !path.exists() {
                continue;
            }
            present = true;
            let attempt = (|| -> Result<State> {
                let mut f = File::open(&path)?;
                if f.metadata()?.len() > MAX_STATE + 65 {
                    return Err(Error::new(507, "oversized state"));
                }
                let mut digest = [0; 65];
                f.read_exact(&mut digest)?;
                let mut body = Vec::new();
                f.read_to_end(&mut body)?;
                let hash = format!("{:x}\n", Sha256::digest(&body));
                if digest != hash.as_bytes() {
                    return Err(Error::new(507, "state checksum mismatch"));
                }
                let state: State = serde_json::from_slice(&body)?;
                if state.jobs.len() > crate::MAX_JOBS
                    || state.notices.len() > crate::MAX_NOTICES
                    || state.blobs.len() > 48
                    || state.copyrights.len() > 48
                    || state.banks.len() > 8
                    || state.dismissed.len() > 64
                    || state.receipts.len() > 32
                {
                    return Err(Error::new(507, "state exceeds configured limits"));
                }
                for bank in &state.banks {
                    bank.validate()?;
                }
                for b in &state.blobs {
                    object_name(&b.bucket, &b.key)?;
                    if b.hash.len() != 64
                        || !b.hash.bytes().all(|c| c.is_ascii_hexdigit())
                        || b.size > MAX_BLOB
                        || !mime_valid(&b.mime)
                        || self.open_blob(b)?.metadata()?.len() != b.size
                    {
                        return Err(Error::new(507, "invalid blob metadata/file"));
                    }
                }
                for (index, record) in state.copyrights.iter().enumerate() {
                    record.validate_blobs(&state.blobs)?;
                    if record.revision == 0
                        || record.revision > state.revision
                        || state.copyrights[..index].iter().any(|c| {
                            c.hash == record.hash
                                || (c.bucket == record.bucket && c.key == record.key)
                        })
                    {
                        return Err(Error::new(507, "invalid copyright registry state"));
                    }
                }
                if state
                    .banks
                    .iter()
                    .enumerate()
                    .any(|(i, b)| state.banks[..i].iter().any(|old| old.id == b.id))
                {
                    return Err(Error::new(507, "duplicate trusted bank"));
                }
                Ok(state)
            })();
            if let Ok(state) = attempt {
                valid.push(state);
            }
        }
        if valid.is_empty() && present {
            return Err(Error::new(
                507,
                "no valid state slot; refusing to reset data",
            ));
        }
        Ok(valid
            .into_iter()
            .max_by_key(|s| s.revision)
            .unwrap_or_default())
    }
    pub fn save(&self, state: &State) -> Result<()> {
        // Alternate slots: interrupted writes leave the previous generation intact.
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(self.slot(state.revision))?;
        f.write_all(&[b'0'; 65])?;
        let mut writer = HashedWriter {
            file: &mut f,
            digest: Sha256::new(),
            written: 0,
        };
        serde_json::to_writer(&mut writer, state).map_err(|e| Error::new(507, e.to_string()))?;
        let digest = format!("{:x}\n", writer.digest.finalize());
        f.seek(SeekFrom::Start(0))?;
        f.write_all(digest.as_bytes())?;
        f.sync_all()?;
        Ok(())
    }
    pub fn upload(
        &self,
        bucket: &str,
        key: &str,
        mime: &str,
        reader: &mut dyn Read,
        length: u64,
    ) -> Result<Blob> {
        object_name(bucket, key)?;
        if length > MAX_BLOB {
            return Err(Error::new(413, "blob exceeds 256 KiB"));
        }
        if !mime_valid(mime) {
            return Err(Error::new(400, "invalid Content-Type"));
        }
        if mime == "application/x-rgb565" && length != 172 * 320 * 2 {
            return Err(Error::new(
                400,
                "RGB565 must be exactly 320x172, big-endian, 110080 bytes",
            ));
        }
        let staging = self.root.join("upload.tmp");
        let result = (|| {
            let mut f = File::create(&staging)?;
            let mut digest = Sha256::new();
            let mut remaining = length;
            let mut chunk = [0; 4096];
            while remaining != 0 {
                let take = chunk.len().min(remaining as usize);
                let n = reader.read(&mut chunk[..take])?;
                if n == 0 {
                    return Err(Error::new(400, "truncated blob upload"));
                }
                f.write_all(&chunk[..n])?;
                digest.update(&chunk[..n]);
                remaining -= n as u64;
            }
            f.sync_all()?;
            drop(f);
            let hash = format!("{:x}", digest.finalize());
            let destination = self.blob_path(&hash);
            if destination.exists() {
                let mut existing = File::open(&destination)?;
                let mut check = Sha256::new();
                loop {
                    let n = existing.read(&mut chunk)?;
                    if n == 0 {
                        break;
                    }
                    check.update(&chunk[..n]);
                }
                if format!("{:x}", check.finalize()) != hash {
                    return Err(Error::new(409, "blob filename hash collision"));
                }
                fs::remove_file(&staging)?;
            } else {
                fs::rename(&staging, destination)?;
            }
            Ok(Blob {
                bucket: bucket.into(),
                key: key.into(),
                mime: mime.into(),
                size: length,
                hash,
                uploader: None,
                tier: Tier::Onboard,
            })
        })();
        if result.is_err() {
            let _ = fs::remove_file(staging);
        }
        result
    }
    pub fn collect(&self, state: &State) -> Result<()> {
        // Keep files referenced by BOTH slots so rollback remains self-contained.
        let mut refs = state.blobs.clone();
        for index in 0..2 {
            if let Ok(mut f) = File::open(self.slot(index)) {
                if f.metadata()?.len() <= MAX_STATE + 65 {
                    let mut header = [0; 65];
                    if f.read_exact(&mut header).is_ok() {
                        if let Ok(old) = serde_json::from_reader::<_, State>(f) {
                            refs.extend(old.blobs);
                        }
                    }
                }
            }
        }
        for (root, tier) in std::iter::once((&self.root, Tier::Onboard))
            .chain(self.card.iter().map(|p| (p, Tier::Card)))
        {
            for entry in fs::read_dir(root)? {
                let entry = entry?;
                let name = entry.file_name();
                if let Some(hash) = name.to_str().and_then(|n| n.strip_prefix("b-")) {
                    if hash.len() == 28
                        && !refs
                            .iter()
                            .any(|b| b.tier == tier && b.hash.starts_with(hash))
                    {
                        fs::remove_file(entry.path())?;
                    }
                }
            }
        }
        Ok(())
    }
}
