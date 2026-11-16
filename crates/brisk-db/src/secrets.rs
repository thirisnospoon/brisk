//! Saved passwords, outside the project: `<config dir>/credentials.json`.
//! On Windows every value is encrypted with DPAPI for the current user (what Windows
//! Credential Manager uses underneath). Elsewhere the file is created with 0600 permissions.

use std::collections::BTreeMap;
use std::path::PathBuf;

fn path() -> PathBuf {
    crate::config_dir().join("credentials.json")
}

fn read_all() -> BTreeMap<String, String> {
    std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn write_all(map: &BTreeMap<String, String>) -> std::io::Result<()> {
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let json = serde_json::to_string_pretty(map).map_err(std::io::Error::other)?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&p)?;
        return f.write_all(json.as_bytes());
    }
    #[cfg(not(unix))]
    std::fs::write(p, json)
}

pub fn store_password(id: &str, password: &str) -> std::io::Result<()> {
    let mut m = read_all();
    m.insert(id.to_string(), seal(password)?);
    write_all(&m)
}

pub fn load_password(id: &str) -> Option<String> {
    read_all().get(id).and_then(|v| open(v))
}

pub fn forget_password(id: &str) {
    let mut m = read_all();
    if m.remove(id).is_some() {
        let _ = write_all(&m);
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(windows)]
mod dpapi {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN};

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

    pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
        let input = blob(data);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // SAFETY: valid input blob, null optional params, out blob filled by the API.
        let ok = unsafe { CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out) };
        (ok != 0).then(|| take(out))
    }

    pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        let input = blob(data);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // SAFETY: as above.
        let ok = unsafe { CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out) };
        (ok != 0).then(|| take(out))
    }
}

#[cfg(windows)]
fn seal(p: &str) -> std::io::Result<String> {
    dpapi::protect(p.as_bytes()).map(|b| format!("dpapi:{}", hex(&b))).ok_or_else(|| std::io::Error::other("DPAPI encryption failed"))
}

#[cfg(not(windows))]
fn seal(p: &str) -> std::io::Result<String> {
    Ok(format!("plain:{}", hex(p.as_bytes())))
}

fn open(v: &str) -> Option<String> {
    if let Some(h) = v.strip_prefix("plain:") {
        return String::from_utf8(unhex(h)?).ok();
    }
    #[cfg(windows)]
    if let Some(h) = v.strip_prefix("dpapi:") {
        return String::from_utf8(dpapi::unprotect(&unhex(h)?)?).ok();
    }
    None
}

#[cfg(test)]
pub(crate) fn roundtrip(p: &str) -> Option<String> {
    open(&seal(p).ok()?)
}
