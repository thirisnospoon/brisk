//! The keymap in use: PyCharm's default plus the user's changes from `keymap.json` in the
//! settings folder. Every window rebuilds its key resolver when the generation changes.

use brisk_keymap::{pycharm_keymap, Keymap, UserKeymap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

static USER: RwLock<Option<UserKeymap>> = RwLock::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(1);

pub fn path() -> std::path::PathBuf {
    brisk_db::config_dir().join("keymap.json")
}

/// The user's changes (read from disk the first time).
pub fn user() -> UserKeymap {
    if let Some(u) = USER.read().ok().and_then(|u| u.clone()) {
        return u;
    }
    let u: UserKeymap = std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if let Ok(mut w) = USER.write() {
        *w = Some(u.clone());
    }
    u
}

pub fn effective() -> Keymap {
    user().apply(&pycharm_keymap())
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

/// Saves the changes and makes every window pick them up.
pub fn save(u: &UserKeymap) -> std::io::Result<()> {
    let p = path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if u.is_empty() {
        let _ = std::fs::remove_file(&p);
    } else {
        std::fs::write(&p, serde_json::to_string_pretty(u).map_err(std::io::Error::other)? + "\n")?;
    }
    if let Ok(mut w) = USER.write() {
        *w = Some(u.clone());
    }
    GENERATION.fetch_add(1, Ordering::SeqCst);
    Ok(())
}
