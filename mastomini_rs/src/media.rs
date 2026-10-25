//! Image files on the SD card (a directory on the desktop). Without one
//! there are no images: posts keep their text and link to the original.
//!
//! One file per image, named by its id, written to a temporary name first so
//! a power cut never leaves a half image under a real name. The board mounts
//! the card with long file names on (FATFS LFN); names are 17 characters.

use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct MediaDir {
    root: PathBuf,
}

impl MediaDir {
    /// The directory for remote posts' images under `root`, created if
    /// missing.
    pub fn open(root: &Path) -> io::Result<MediaDir> {
        let root = root.join("remote");
        std::fs::create_dir_all(&root)?;
        Ok(MediaDir { root })
    }

    pub fn path(&self, id: u64) -> PathBuf {
        self.root.join(format!("{}.jpg", crate::ids::b32_str(id)))
    }

    pub fn put(&self, id: u64, bytes: &[u8]) -> io::Result<()> {
        let tmp = self.root.join(format!("{}.tmp", crate::ids::b32_str(id)));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, self.path(id))
    }

    /// Deleting a file that is already gone is not an error.
    pub fn remove(&self, id: u64) {
        let _ = std::fs::remove_file(self.path(id));
    }

    /// Every stored image id, and leftover temporary files removed.
    pub fn ids(&self) -> io::Result<Vec<u64>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let path = entry?.path();
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            match path.extension().and_then(|e| e.to_str()) {
                Some("jpg") => match crate::ids::from_b32(stem) {
                    Some(id) => out.push(id),
                    None => {
                        let _ = std::fs::remove_file(&path);
                    }
                },
                _ => {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        Ok(out)
    }
}

/// Width and height of a baseline or progressive JPEG, from its first
/// start-of-frame marker. `None` for anything that is not a JPEG.
pub fn jpeg_size(bytes: &[u8]) -> Option<(u16, u16)> {
    if bytes.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            return None;
        }
        let marker = bytes[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        // SOF0-SOF15, except DHT (C4), JPG (C8) and DAC (CC).
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let sof = bytes.get(i + 5..i + 9)?;
            let height = u16::from_be_bytes([sof[0], sof[1]]);
            let width = u16::from_be_bytes([sof[2], sof[3]]);
            return (width > 0 && height > 0).then_some((width, height));
        }
        if len < 2 {
            return None;
        }
        i += 2 + len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A JPEG header with an APP0 segment and a SOF0 of `w`×`h`.
    pub fn jpeg(w: u16, h: u16) -> Vec<u8> {
        let mut b = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
        b.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0B, 0x08]);
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9]);
        b
    }

    #[test]
    fn reads_jpeg_dimensions_and_refuses_other_files() {
        assert_eq!(jpeg_size(&jpeg(640, 480)), Some((640, 480)));
        assert_eq!(jpeg_size(b"\x89PNG\r\n"), None);
        assert_eq!(jpeg_size(&[0xFF, 0xD8, 0xFF]), None);
        assert_eq!(jpeg_size(&jpeg(0, 10)), None);
    }

    #[test]
    fn files_round_trip_and_leftovers_are_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let media = MediaDir::open(dir.path()).unwrap();
        media.put(42, b"x").unwrap();
        std::fs::write(dir.path().join("remote").join("junk.tmp"), b"y").unwrap();
        assert_eq!(media.ids().unwrap(), vec![42]);
        assert!(!dir.path().join("remote").join("junk.tmp").exists());
        media.remove(42);
        media.remove(42);
        assert!(media.ids().unwrap().is_empty());
    }
}
