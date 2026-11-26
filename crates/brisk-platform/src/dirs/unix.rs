use super::Env;
use std::path::PathBuf;

/// `~/Library/Application Support/Brisk` on macOS. Elsewhere `None`: the caller uses the XDG
/// folder.
pub fn config_dir(env: Env) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return env("HOME").map(|h| PathBuf::from(h).join("Library").join("Application Support").join("Brisk"));
    }
    None
}

pub fn roaming_app_data(_: Env) -> Option<PathBuf> {
    None
}

pub fn home(env: Env) -> Option<PathBuf> {
    super::non_empty(env, "HOME")
}

pub fn roots() -> Vec<PathBuf> {
    vec![PathBuf::from("/")]
}
