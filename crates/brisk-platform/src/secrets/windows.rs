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

// The stored form of the encrypted bytes. Only Windows stores passwords.
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::{hex, unhex};

    #[test]
    fn hex_round_trip() {
        assert_eq!(hex(&[0, 0x61, 0xff]), "0061ff");
        assert_eq!(unhex("0061ff"), Some(vec![0, 0x61, 0xff]));
        assert_eq!(unhex("061"), None);
        assert_eq!(unhex("zz"), None);
    }
}
