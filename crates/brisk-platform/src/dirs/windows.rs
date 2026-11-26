use super::Env;
use std::path::PathBuf;

/// `%APPDATA%\Brisk`.
pub fn config_dir(env: Env) -> Option<PathBuf> {
    roaming_app_data(env).map(|a| a.join("Brisk"))
}

pub fn roaming_app_data(env: Env) -> Option<PathBuf> {
    env("APPDATA").map(PathBuf::from)
}

pub fn home(env: Env) -> Option<PathBuf> {
    super::non_empty(env, "USERPROFILE")
}

pub fn roots() -> Vec<PathBuf> {
    // SAFETY: no arguments; returns a bit mask of the drive letters.
    drive_roots(unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() })
}

/// `C:\` and the like for the set bits of a `GetLogicalDrives` mask (bit 0 is `A:`).
pub fn drive_roots(mask: u32) -> Vec<PathBuf> {
    (0..26u8).filter(|i| mask & (1 << i) != 0).map(|i| PathBuf::from(format!("{}:\\", (b'A' + i) as char))).collect()
}
