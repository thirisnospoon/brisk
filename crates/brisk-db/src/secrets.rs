//! Saved passwords, outside the project: `<config dir>/credentials.json`.
//! On Windows every value is encrypted with DPAPI for the current user (what Windows
//! Credential Manager uses underneath). Elsewhere the file is created with 0600 permissions.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn path() -> PathBuf {
    crate::config_dir().join("credentials.json")
}

/// The saved values. No file is an empty map; a file that does not parse is an error, so
/// that the next save does not replace every saved password with just the new one.
fn read_all(p: &Path) -> std::io::Result<BTreeMap<String, String>> {
    match std::fs::read_to_string(p) {
        Ok(s) if s.trim().is_empty() => Ok(BTreeMap::new()),
        Ok(s) => serde_json::from_str(&s).map_err(|e| std::io::Error::other(format!("{} is damaged ({e}); passwords are not saved until it is fixed or deleted", p.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e),
    }
}

/// Written to a temporary file that then replaces the old one: a crash mid-write leaves the
/// old file whole.
fn write_all(p: &Path, map: &BTreeMap<String, String>) -> std::io::Result<()> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let json = serde_json::to_string_pretty(map).map_err(std::io::Error::other)?;
    let tmp = p.with_extension(format!("tmp{}", std::process::id()));
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let written = (|| {
        use std::io::Write;
        let mut f = o.open(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, p)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

fn store_at(p: &Path, id: &str, sealed: String) -> std::io::Result<()> {
    let mut m = read_all(p)?;
    m.insert(id.to_string(), sealed);
    write_all(p, &m)
}

pub fn store_password(id: &str, password: &str) -> std::io::Result<()> {
    store_at(&path(), id, seal(password)?)
}

pub fn load_password(id: &str) -> Option<String> {
    read_all(&path()).ok()?.get(id).and_then(|v| open(v))
}

pub fn forget_password(id: &str) {
    let p = path();
    // A damaged file is left as it is (see `read_all`).
    if let Ok(mut m) = read_all(&p) {
        if m.remove(id).is_some() {
            let _ = write_all(&p, &m);
        }
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

#[cfg(test)]
mod t {
    use super::*;

    #[test]
    fn damaged_file_is_not_overwritten() {
        let d = std::env::temp_dir().join(format!("brisk-secrets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p = d.join("credentials.json");
        store_at(&p, "a", "plain:61".into()).unwrap();
        store_at(&p, "b", "plain:62".into()).unwrap();
        assert_eq!(read_all(&p).unwrap().len(), 2);
        assert!(std::fs::read_dir(&d).unwrap().count() == 1, "no temporary file left");
        std::fs::write(&p, "{\"a\": \"plain:61\",").unwrap();
        assert!(store_at(&p, "c", "plain:63".into()).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"a\": \"plain:61\",", "kept as it was");
        let _ = std::fs::remove_dir_all(&d);
    }
}
