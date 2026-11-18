//! Panic hook: every panic (UI thread, background task, reader thread) is appended to
//! `<data dir>/crash.log` with its thread, location and a backtrace, before the default hook
//! runs (release builds abort right after, so this is the only trace a user can send).

use std::io::Write;

const MAX_LOG: u64 = 1 << 20;

pub fn log_path() -> std::path::PathBuf {
    brisk_db::config_dir().join("crash.log")
}

pub fn install() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let path = log_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Keep the log small: start over once it passes 1 MiB.
        let append = std::fs::metadata(&path).map(|m| m.len() < MAX_LOG).unwrap_or(false);
        let file = std::fs::OpenOptions::new().create(true).write(true).append(append).truncate(!append).open(&path);
        if let Ok(mut f) = file {
            let when = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let thread = std::thread::current().name().unwrap_or("<unnamed>").to_string();
            let bt = std::backtrace::Backtrace::force_capture();
            let _ = writeln!(f, "=== Brisk {} panic at unix {when}, thread '{thread}'\n{info}\n{bt}\n", env!("CARGO_PKG_VERSION"));
        }
        default(info);
    }));
}
