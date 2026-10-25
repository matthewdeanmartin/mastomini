//! The P4's TF card (components/p4-network/sd_card.c), for images from
//! other servers. No card: no images, and everything else works.
use mastomini::media::MediaDir;

const MOUNT: &str = "/sdcard";

pub fn mount() -> Option<MediaDir> {
    let mut capacity = 0u64;
    // SAFETY: a NUL-terminated static path and a local the call fills in;
    // the C side keeps nothing that points at either.
    let err = unsafe { esp_idf_svc::sys::mastomini_sd_mount(c"/sdcard".as_ptr(), &mut capacity) };
    if err != esp_idf_svc::sys::ESP_OK {
        log::warn!("No SD card ({err}): images from other servers are off");
        return None;
    }
    log::info!(
        "SD card mounted at {MOUNT}: {} MiB",
        capacity / (1024 * 1024)
    );
    match MediaDir::open(std::path::Path::new(MOUNT)) {
        Ok(media) => Some(media),
        Err(e) => {
            log::warn!("SD card: {e}; images from other servers are off");
            None
        }
    }
}
