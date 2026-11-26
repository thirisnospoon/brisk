//! The system's default fonts, and the folders of fonts Brisk ships.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[cfg_attr(windows, path = "windows.rs")]
#[cfg_attr(not(windows), path = "unix.rs")]
mod sys;

/// The default UI font of this system.
pub const UI: &str = sys::UI;

/// The default editor and console font of this system.
pub const MONO: &str = sys::MONO;

/// The UI font: `BRISK_UI_FONT` overrides [`UI`] (for comparing fonts).
pub fn ui() -> &'static str {
    static F: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    F.get_or_init(|| env_font("BRISK_UI_FONT").unwrap_or(UI))
}

/// The default editor and console font: `BRISK_MONO_FONT` overrides [`MONO`].
pub fn mono() -> &'static str {
    static F: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    F.get_or_init(|| env_font("BRISK_MONO_FONT").unwrap_or(MONO))
}

fn env_font(var: &str) -> Option<&'static str> {
    std::env::var(var).ok().filter(|f| !f.trim().is_empty()).map(|f| &*Box::leak(f.into_boxed_str()))
}

/// Folders whose `.ttf` / `.otf` files Brisk loads at start: `BRISK_FONT_DIR`, then `fonts`
/// next to the exe.
pub fn extra_dirs() -> Vec<PathBuf> {
    let exe = std::env::current_exe().ok();
    extra_dirs_from(std::env::var_os("BRISK_FONT_DIR"), exe.as_deref().and_then(Path::parent))
}

fn extra_dirs_from(env_dir: Option<OsString>, exe_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = env_dir.map(Into::into).into_iter().collect();
    if let Some(d) = exe_dir {
        v.push(d.join("fonts"));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_dirs_env_first() {
        let exe_dir = Path::new("brisk-install");
        assert_eq!(extra_dirs_from(Some("mine".into()), Some(exe_dir)), vec![PathBuf::from("mine"), exe_dir.join("fonts")]);
        assert_eq!(extra_dirs_from(None, Some(exe_dir)), vec![exe_dir.join("fonts")]);
        assert!(extra_dirs_from(None, None).is_empty());
    }
}
