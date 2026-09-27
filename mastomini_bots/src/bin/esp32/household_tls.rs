//! Load the unchanged household root using mbedTLS's extension callback.
//! Only the precisely supported nameConstraints value is handled here;
//! all normal signature, chain, date and hostname checks remain required.
use super::household_trust;
use esp_idf_svc::sys::*;
use std::{
    ffi::{c_int, c_void},
    sync::OnceLock,
};

struct Anchor(*mut mbedtls_x509_crt);
// SAFETY: initialized once, then immutable and retained for the firmware's
// lifetime. mbedTLS permits a CA chain to be shared across configurations.
unsafe impl Send for Anchor {}
unsafe impl Sync for Anchor {}
static ANCHOR: OnceLock<Result<Anchor, String>> = OnceLock::new();

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
    let name = std::slice::from_raw_parts((*oid).p, (*oid).len);
    let value = std::slice::from_raw_parts(start, end.offset_from(start) as usize);
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
            // SAFETY: initialized owned cert, static DER; parse copies buffers.
            unsafe {
                mbedtls_x509_crt_init(cert.as_mut());
                let result = mbedtls_x509_crt_parse_der_with_ext_cb(
                    cert.as_mut(),
                    der.as_ptr(),
                    der.len(),
                    1,
                    Some(extension),
                    std::ptr::null_mut(),
                );
                if result != 0 {
                    mbedtls_x509_crt_free(cert.as_mut());
                    return Err(format!("household CA parse failed: mbedtls={result}"));
                }
            }
            Ok(Anchor(Box::into_raw(cert)))
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
    let mut san = &(*cert).subject_alt_names as *const mbedtls_x509_sequence;
    while !san.is_null() {
        let value = &(*san).buf;
        if value.len != 0
            && !household_trust::san_allowed(
                value.tag,
                std::slice::from_raw_parts(value.p, value.len),
            )
        {
            *flags |= MBEDTLS_X509_BADCERT_OTHER;
        }
        san = (*san).next;
    }
    0
}

pub unsafe extern "C" fn attach(config: *mut c_void) -> esp_err_t {
    let config = config.cast::<mbedtls_ssl_config>();
    let Some(Ok(anchor)) = ANCHOR.get() else {
        return ESP_ERR_INVALID_STATE;
    };
    // SAFETY: ESP-TLS passes the connection's live SSL configuration. The
    // root and callback outlive every connection; public requests use a
    // different attach function and cannot trust this household anchor.
    mbedtls_ssl_conf_authmode(config, MBEDTLS_SSL_VERIFY_REQUIRED as _);
    mbedtls_ssl_conf_ca_chain(config, anchor.0, std::ptr::null_mut());
    mbedtls_ssl_conf_verify(config, Some(verify_names), std::ptr::null_mut());
    ESP_OK
}
