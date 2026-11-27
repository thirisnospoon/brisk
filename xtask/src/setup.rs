//! The Windows installer: `dist/Brisk-<version>-setup.exe`, the `brisk-setup` exe with
//! setup.manifest swapped in and the portable zip appended (minus the `portable` marker,
//! which the setup drops when it installs). Signing, when there is a certificate, happens
//! after this step: the setup finds its zip in front of the signature.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub fn run(args: &[String]) -> ExitCode {
    let mut no_build = false;
    for a in args {
        match a.as_str() {
            "--no-build" => no_build = true,
            "-h" | "--help" => {
                println!("usage: cargo xtask package-setup [--no-build]\n\n  --no-build   use brisk.exe and brisk-setup.exe already in target/release");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("package-setup: unknown argument {other}");
                return ExitCode::from(2);
            }
        }
    }
    match package(&crate::root(), no_build) {
        Ok(exe) => {
            let size = std::fs::metadata(&exe).map_or(0, |m| m.len());
            println!("Packaged {} ({} MB)", exe.display(), crate::mb(size as f64));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("package-setup: {e}");
            ExitCode::FAILURE
        }
    }
}

pub fn package(root: &Path, no_build: bool) -> Result<PathBuf, String> {
    let zip = crate::package::package(root, no_build)?;
    if !no_build {
        crate::cargo(&["build", "--release", "-p", "brisk-setup"])?;
    }
    let toml = std::fs::read_to_string(root.join("Cargo.toml")).map_err(|e| format!("Cargo.toml: {e}"))?;
    let version = crate::package::version(&toml).ok_or("no version in Cargo.toml")?;
    let built = root.join("target").join("release").join("brisk-setup.exe");
    let exe = root.join("dist").join(format!("Brisk-{version}-setup.exe"));
    std::fs::copy(&built, &exe).map_err(|e| format!("cannot copy {}: {e}", built.display()))?;
    let manifest = std::fs::read_to_string(root.join("crates").join("brisk-setup").join("setup.manifest")).map_err(|e| format!("setup.manifest: {e}"))?;
    brisk_platform::install::set_manifest(&exe, &manifest)?;
    brisk_platform::install::payload::append(&exe, &zip).map_err(|e| format!("cannot append the zip: {e}"))?;
    crate::sign(&exe)?;
    Ok(exe)
}
