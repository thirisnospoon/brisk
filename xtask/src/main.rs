//! `cargo xtask <task>` (the alias is in `.cargo/config.toml`): Brisk's release tasks, in Rust
//! so that they run wherever Cargo does.
//!
//! - `package-portable [--no-build]`: `dist/Brisk-<version>-windows-x64.zip` (see `package.rs`).
//! - `budget [options]`: size, startup and memory of the release build against its limits
//!   (see `budget.rs`, or `cargo xtask budget --help`).
//!
//! `scripts\package-portable.ps1` and `scripts\budget.ps1` are wrappers that call these.

mod budget;
mod package;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask <task>

tasks:
  package-portable [--no-build]   build dist/Brisk-<version>-windows-x64.zip
  budget [options]                check size, startup and memory (--help for the options)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("package-portable") => package::run(&args[1..]),
        Some("budget") => budget::run(&args[1..]),
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// The repository root: the folder above `xtask/`.
fn root() -> PathBuf {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    here.parent().unwrap_or(here).to_path_buf()
}

/// `cargo <args>` in the repository root, with its output on the console.
fn cargo(args: &[&str]) -> Result<(), String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = std::process::Command::new(cargo).args(args).current_dir(root()).status().map_err(|e| format!("cannot run cargo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo {} failed", args.join(" ")))
    }
}

/// `bytes` in MB with one decimal, rounded half to even like PowerShell's `[math]::Round`.
fn mb(bytes: f64) -> f64 {
    (bytes / (1024.0 * 1024.0) * 10.0).round_ties_even() / 10.0
}

#[cfg(test)]
mod tests {
    #[test]
    fn megabytes_with_one_decimal() {
        assert_eq!(super::mb(39_023_187.0), 37.2);
        assert_eq!(super::mb(0.0), 0.0);
        assert_eq!(super::mb(1024.0 * 1024.0 * 26.0), 26.0);
    }
}
