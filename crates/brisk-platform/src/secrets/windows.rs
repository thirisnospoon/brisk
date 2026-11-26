use super::{hex, unhex};
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};

/// `dpapi:<hex>`: encrypted for the current Windows user.
pub fn seal(p: &str) -> std::io::Result<String> {
    protect(p.as_bytes()).map(|b| format!("dpapi:{}", hex(&b))).ok_or_else(|| std::io::Error::other("DPAPI encryption failed"))
}

/// A `dpapi:` value, or a `plain:` one (hex of the UTF-8 text).
pub fn open(v: &str) -> Option<String> {
    if let Some(h) = v.strip_prefix("plain:") {
        return String::from_utf8(unhex(h)?).ok();
    }
    if let Some(h) = v.strip_prefix("dpapi:") {
        return String::from_utf8(unprotect(&unhex(h)?)?).ok();
    }
    None
}

/// The file inherits the ACL of the user's profile folder.
pub fn private_file_options() -> std::fs::OpenOptions {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    o
}

fn blob(b: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: b.len() as u32, pbData: b.as_ptr() as *mut u8 }
}

fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    // SAFETY: the API allocated `cbData` bytes at `pbData` with LocalAlloc.
    unsafe {
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as _);
        v
    }
}

fn protect(data: &[u8]) -> Option<Vec<u8>> {
    let input = blob(data);
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: valid input blob, null optional params, out blob filled by the API.
    let ok = unsafe { CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out) };
    (ok != 0).then(|| take(out))
}

fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    let input = blob(data);
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: as above.
    let ok =
        unsafe { CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out) };
    (ok != 0).then(|| take(out))
}
