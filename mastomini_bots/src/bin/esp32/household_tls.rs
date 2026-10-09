//! Load the unchanged household root using mbedTLS's extension callback.
//! Only the precisely supported nameConstraints value is handled here;
//! all normal signature, chain, date and hostname checks remain required.
#![deny(unsafe_op_in_unsafe_fn)]
use super::household_trust;
use esp_idf_svc::sys::*;
use std::{
    ffi::{c_int, c_void},
    sync::OnceLock,
};

struct Anchor(Box<mbedtls_x509_crt>);
// SAFETY: initialized once, then immutable and retained for the firmware's
// lifetime. mbedTLS permits a CA chain to be shared across configurations.
unsafe impl Send for Anchor {}
unsafe impl Sync for Anchor {}
static ANCHOR: OnceLock<Result<Anchor, String>> = OnceLock::new();

impl Drop for Anchor {
    fn drop(&mut self) {
        // SAFETY: created only after init; free releases fields, not the Box.
        unsafe { mbedtls_x509_crt_free(self.0.as_mut()) };
    }
}

unsafe extern "C" fn extension(
    _context: *mut c_void,
    _cert: *const mbedtls_x509_crt,
    oid: *const mbedtls_x509_buf,
    _critical: c_int,
    start: *const u8,
    end: *const u8,
) -> c_int {
    // SAFETY: mbedTLS supplies an OID and a bounded extension slice while
    // parsing the statically embedded trust anchor, never a peer certificate.
    if oid.is_null() || start.is_null() || end.is_null() {
        return MBEDTLS_ERR_X509_INVALID_EXTENSIONS;
    }
    let oid = unsafe { &*oid };
    let len = unsafe { end.offset_from(start) };
    if oid.p.is_null() || len < 0 {
        return MBEDTLS_ERR_X509_INVALID_EXTENSIONS;
    }
    let (name, value) = unsafe {
        (
            std::slice::from_raw_parts(oid.p, oid.len),
            std::slice::from_raw_parts(start, len as usize),
        )
    };
    if name == [0x55, 0x1d, 0x1e] && value == household_trust::constraints_der() {
        0
    } else {
        MBEDTLS_ERR_X509_INVALID_EXTENSIONS
    }
}

pub fn prepare(der: &'static [u8]) -> Result<(), String> {
    ANCHOR
        .get_or_init(|| {
            let mut cert = Box::new(mbedtls_x509_crt::default());
            // SAFETY: initialized exclusive allocation; init cannot fail.
            unsafe { mbedtls_x509_crt_init(cert.as_mut()) };
            let mut anchor = Anchor(cert);
            // SAFETY: initialized owned cert, static DER; parse copies buffers.
            let result = unsafe {
                mbedtls_x509_crt_parse_der_with_ext_cb(
                    anchor.0.as_mut(),
                    der.as_ptr(),
                    der.len(),
                    1,
                    Some(extension),
                    std::ptr::null_mut(),
                )
            };
            if result != 0 {
                return Err(format!("household CA parse failed: mbedtls={result}"));
            }
            Ok(anchor)
        })
        .as_ref()
        .map(|_| ())
        .map_err(Clone::clone)
}

unsafe extern "C" fn verify_names(
    _context: *mut c_void,
    cert: *mut mbedtls_x509_crt,
    _depth: c_int,
    flags: *mut u32,
) -> c_int {
    // SAFETY: mbedTLS supplies the parsed chain node and mutable flags.
    // Add failures only: never clear its existing verification errors.
    if cert.is_null() || flags.is_null() {
        return MBEDTLS_ERR_X509_INVALID_EXTENSIONS;
    }
    let mut san = unsafe { &(*cert).subject_alt_names as *const mbedtls_x509_sequence };
    while !san.is_null() {
        // SAFETY: SDK's parsed, finite SAN list is live throughout this callback.
        let value = unsafe { &(*san).buf };
        if value.len != 0
            && (value.p.is_null()
                || !household_trust::san_allowed(value.tag, unsafe {
                    std::slice::from_raw_parts(value.p, value.len)
                }))
        {
            unsafe { *flags |= MBEDTLS_X509_BADCERT_OTHER };
        }
        san = unsafe { (*san).next };
    }
    0
}

pub unsafe extern "C" fn attach(config: *mut c_void) -> esp_err_t {
    let config = config.cast::<mbedtls_ssl_config>();
    if config.is_null() {
        return ESP_ERR_INVALID_ARG;
    }
    let Some(Ok(anchor)) = ANCHOR.get() else {
        return ESP_ERR_INVALID_STATE;
    };
    // SAFETY: ESP-TLS passes the connection's live SSL configuration. The
    // root and callback outlive every connection; public requests use a
    // different attach function and cannot trust this household anchor.
    unsafe {
        mbedtls_ssl_conf_authmode(config, MBEDTLS_SSL_VERIFY_REQUIRED as _);
        // C accepts mut pointers; verification only reads the retained CA.
        mbedtls_ssl_conf_ca_chain(
            config,
            anchor.0.as_ref() as *const _ as *mut _,
            std::ptr::null_mut(),
        );
        mbedtls_ssl_conf_verify(config, Some(verify_names), std::ptr::null_mut());
    }
    ESP_OK
}
