use super::{Installed, Options};
use std::path::{Path, PathBuf};

const NONE: &str = "Brisk has no installer on this system yet";

pub fn default_dir() -> Option<PathBuf> {
    None
}

pub fn installed() -> Option<Installed> {
    None
}

pub fn register(_: &Path, _: &str, _: &Options, _: u64) -> Result<(), String> {
    Err(NONE.into())
}

pub fn set_version(_: &str) -> Result<(), String> {
    Err(NONE.into())
}

pub fn unregister() -> Result<(), String> {
    Err(NONE.into())
}

pub fn delete_on_reboot(_: &Path) {}

pub fn exe_of(_: u32) -> Option<PathBuf> {
    None
}

pub fn set_manifest(_: &Path, _: &str) -> Result<(), String> {
    Err("manifests are a Windows thing".into())
}
