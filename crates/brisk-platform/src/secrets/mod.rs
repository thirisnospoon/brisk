//! Saved passwords, outside the project: `<config dir>/credentials.json`, a JSON map from an
//! id to a sealed value. On Windows every value is encrypted with DPAPI for the current user
//! (what Windows Credential Manager uses underneath) and stored as `dpapi:<hex>`. Elsewhere
//! there is no keyring support yet: saving a password is refused and nothing is loaded.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[cfg_attr(windows, path = "windows.rs")]
#[cfg_attr(not(windows), path = "unix.rs")]
mod sys;

fn path() -> PathBuf {
    crate::dirs::config_dir().join("credentials.json")
}

pub fn store_password(id: &str, password: &str) -> std::io::Result<()> {
    store_at(&path(), id, sys::seal(password)?)
}

pub fn load_password(id: &str) -> Option<String> {
    load_at(&path(), id)
}

pub fn forget_password(id: &str) {
    forget_at(&path(), id)
}

/// The saved values. No file is an empty map; a file that does not parse is an error, so
/// that the next save does not replace every saved password with just the new one.
fn read_all(p: &Path) -> std::io::Result<BTreeMap<String, String>> {
    match std::fs::read_to_string(p) {
        Ok(s) if s.trim().is_empty() => Ok(BTreeMap::new()),
        Ok(s) => serde_json::from_str(&s)
            .map_err(|e| std::io::Error::other(format!("{} is damaged ({e}); passwords are not saved until it is fixed or deleted", p.display()))),
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
    let written = (|| {
        use std::io::Write;
        let mut f = sys::private_file_options().open(&tmp)?;
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

fn load_at(p: &Path, id: &str) -> Option<String> {
    read_all(p).ok()?.get(id).and_then(|v| sys::open(v))
}

fn forget_at(p: &Path, id: &str) {
    // A damaged file is left as it is (see `read_all`).
    if let Ok(mut m) = read_all(p) {
        if m.remove(id).is_some() {
            let _ = write_all(p, &m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("brisk-secrets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn round_trip() {
        let d = scratch("round-trip");
        let p = d.join("credentials.json");
        let password = "pä$$ wörd";
        let sealed = sys::seal(password);
        if cfg!(windows) {
            let sealed = sealed.expect("DPAPI encrypts");
            assert!(sealed.starts_with("dpapi:"), "{sealed}");
            assert!(!sealed.contains(&to_hex(password.as_bytes())), "stored encrypted");
            store_at(&p, "db", sealed).expect("store");
            assert_eq!(load_at(&p, "db").as_deref(), Some(password));
            forget_at(&p, "db");
            assert_eq!(load_at(&p, "db"), None);
        } else {
            // No keyring: saving is refused, and even a value on disk is not read.
            let e = sealed.expect_err("saving is refused");
            assert_eq!(e.kind(), std::io::ErrorKind::Unsupported);
            store_at(&p, "db", format!("plain:{}", to_hex(password.as_bytes()))).expect("store");
            assert_eq!(load_at(&p, "db"), None);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn damaged_file_is_not_overwritten() {
        let d = scratch("damaged");
        let p = d.join("credentials.json");
        store_at(&p, "a", "plain:61".into()).expect("store a");
        store_at(&p, "b", "plain:62".into()).expect("store b");
        assert_eq!(read_all(&p).map(|m| m.len()).ok(), Some(2));
        assert_eq!(std::fs::read_dir(&d).map(|r| r.count()).ok(), Some(1), "no temporary file left");
        let damaged = "{\"a\": \"plain:61\",";
        std::fs::write(&p, damaged).expect("damage the file");
        assert!(store_at(&p, "c", "plain:63".into()).is_err());
        forget_at(&p, "a");
        assert_eq!(std::fs::read_to_string(&p).ok().as_deref(), Some(damaged), "kept as it was");
        let _ = std::fs::remove_dir_all(&d);
    }
}
