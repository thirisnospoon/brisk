//! The keymap in use: PyCharm's default plus the user's changes from `keymap.json` in the
//! settings folder. Every window rebuilds its key resolver when the generation changes.
//!
//! Each project window is its own process, so a change is a read-modify-write of the file
//! (another window's changes stay), and a file that does not parse is never overwritten.

use brisk_keymap::{pycharm_keymap, Keymap, UserKeymap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

static USER: RwLock<Option<UserKeymap>> = RwLock::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(1);
/// Why `keymap.json` could not be read; changes are refused until it is fixed or deleted.
static LOAD_ERROR: RwLock<Option<String>> = RwLock::new(None);

pub fn path() -> std::path::PathBuf {
    brisk_db::config_dir().join("keymap.json")
}

/// The file's contents: empty when there is none, an error when it does not parse.
fn read_file(p: &std::path::Path) -> Result<UserKeymap, String> {
    match std::fs::read_to_string(p) {
        Ok(t) if t.trim().is_empty() => Ok(UserKeymap::default()),
        Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{} is not a valid keymap ({e}); fix or delete it", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(UserKeymap::default()),
        Err(e) => Err(format!("Cannot read {}: {e}", p.display())),
    }
}

/// The user's changes (read from disk the first time). A broken file means the defaults
/// for now; it is kept as it is, with a copy in `keymap.json.bak`.
pub fn user() -> UserKeymap {
    if let Some(u) = USER.read().ok().and_then(|u| u.clone()) {
        return u;
    }
    let p = path();
    let u = match read_file(&p) {
        Ok(u) => u,
        Err(e) => {
            let _ = std::fs::copy(&p, p.with_extension("json.bak"));
            eprintln!("keymap: {e}");
            if let Ok(mut w) = LOAD_ERROR.write() {
                *w = Some(e);
            }
            UserKeymap::default()
        }
    };
    if let Ok(mut w) = USER.write() {
        *w = Some(u.clone());
    }
    u
}

/// Why the saved keymap is not in use (for a notification), if it could not be read.
pub fn load_error() -> Option<String> {
    LOAD_ERROR.read().ok().and_then(|e| e.clone())
}

pub fn effective() -> Keymap {
    user().apply(&pycharm_keymap())
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

/// Applies one change to the keymap on disk (re-read first, so changes saved by another
/// window are kept), saves it and makes every window here pick it up. Returns what `f`
/// returned.
pub fn change<R>(f: impl FnOnce(&mut UserKeymap) -> R) -> std::io::Result<R> {
    let p = path();
    let mut u = read_file(&p).map_err(std::io::Error::other)?;
    let r = f(&mut u);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if u.is_empty() {
        match std::fs::remove_file(&p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    } else {
        let text = serde_json::to_string_pretty(&u).map_err(std::io::Error::other)? + "\n";
        crate::editor::write_atomic(&p, text.as_bytes())?;
    }
    if let Ok(mut w) = LOAD_ERROR.write() {
        *w = None;
    }
    if let Ok(mut w) = USER.write() {
        *w = Some(u);
    }
    GENERATION.fetch_add(1, Ordering::SeqCst);
    Ok(r)
}
