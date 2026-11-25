use super::Env;
use std::path::PathBuf;

/// `%APPDATA%\Brisk`.
pub fn config_dir(env: Env) -> Option<PathBuf> {
    roaming_app_data(env).map(|a| a.join("Brisk"))
}

pub fn roaming_app_data(env: Env) -> Option<PathBuf> {
    env("APPDATA").map(PathBuf::from)
}
