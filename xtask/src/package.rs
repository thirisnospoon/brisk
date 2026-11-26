//! The portable Windows package: `dist/Brisk-<version>-windows-x64.zip`, holding brisk.exe,
//! the README and licenses, debugpy, ty and ruff in `lib/`, the third-party notices and the
//! `portable` marker. Entry names use `/`, so unzip and other non-Windows tools read them.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub fn run(args: &[String]) -> ExitCode {
    let mut no_build = false;
    for a in args {
        match a.as_str() {
            "--no-build" => no_build = true,
            "-h" | "--help" => {
                println!("usage: cargo xtask package-portable [--no-build]\n\n  --no-build   package the brisk.exe already in target/release");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("package-portable: unknown argument {other}");
                return ExitCode::from(2);
            }
        }
    }
    match package(&crate::root(), no_build) {
        Ok(zip) => {
            let size = std::fs::metadata(&zip).map_or(0, |m| m.len());
            println!("Packaged {} ({} MB)", zip.display(), crate::mb(size as f64));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("package-portable: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `version = "…"` of the workspace: the first line of Cargo.toml that starts with it.
pub fn version(cargo_toml: &str) -> Option<String> {
    cargo_toml.lines().find_map(|l| {
        let v = l.strip_prefix("version")?.trim_start().strip_prefix('=')?.trim_start().strip_prefix('"')?;
        v.split_once('"').map(|(v, _)| v.to_string())
    })
}

/// The folder and the zip of the package, its contents copied from the repository. Returns
/// the zip's path.
pub fn package(root: &Path, no_build: bool) -> Result<PathBuf, String> {
    let toml = std::fs::read_to_string(root.join("Cargo.toml")).map_err(|e| format!("Cargo.toml: {e}"))?;
    let version = version(&toml).ok_or("no version in Cargo.toml")?;
    if !no_build {
        crate::cargo(&["build", "--release", "-p", "brisk-app"])?;
    }
    let name = format!("Brisk-{version}-windows-x64");
    let dist = root.join("dist");
    let dir = dist.join(&name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("cannot remove {}: {e}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

    let exe = root.join("target").join("release").join("brisk.exe");
    copy(&exe, &dir)?;
    for f in ["README.md", "LICENSE-MIT", "LICENSE-APACHE"] {
        copy(&root.join(f), &dir)?;
    }
    // The debugger: debugpy next to brisk.exe (lib/debugpy), used when the project's Python
    // has none.
    fetch(root, "fetch-debugpy.ps1", &[root.join("vendor").join("debugpy-lib").join("debugpy").join("__init__.py")])?;
    let lib = dir.join("lib");
    copy_dir(&root.join("vendor").join("debugpy-lib"), &lib)?;
    // Code insight: ty and ruff in lib/, used when neither the project interpreter nor PATH
    // has them.
    let tools = root.join("vendor").join("tools");
    let tool_files: Vec<PathBuf> = ["ty", "ruff"].iter().flat_map(|t| [tools.join(format!("{t}.exe")), tools.join(format!("{t}.LICENSE.txt"))]).collect();
    fetch(root, "fetch-tools.ps1", &tool_files)?;
    for f in &tool_files {
        copy(f, &lib)?;
    }
    // Licenses of the Rust crates and icons compiled into brisk.exe (scripts/third-party-notices.ps1).
    copy(&root.join("THIRD-PARTY-NOTICES.txt"), &dir)?;
    std::fs::write(
        dir.join("portable"),
        "This file switches Brisk to portable mode: settings, database consoles and saved passwords\r\n\
         are kept in the 'data' folder next to brisk.exe instead of %APPDATA%\\Brisk.\r\n\
         Delete this file to use %APPDATA%\\Brisk.\r\n",
    )
    .map_err(|e| format!("cannot write the portable marker: {e}"))?;

    let zip = dist.join(format!("{name}.zip"));
    if zip.exists() {
        std::fs::remove_file(&zip).map_err(|e| format!("cannot remove {}: {e}", zip.display()))?;
    }
    write_zip(&dir, &name, &zip)?;
    Ok(zip)
}

/// The vendored tools, through the fetch script (it does nothing when they are already
/// there). Without PowerShell they have to be there already.
fn fetch(root: &Path, script: &str, needed: &[PathBuf]) -> Result<(), String> {
    let run = brisk_platform::process::command("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(root.join("scripts").join(script))
        .current_dir(root)
        .status();
    match run {
        Ok(s) if !s.success() => return Err(format!("{script} failed")),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("cannot run {script}: {e}")),
    }
    match needed.iter().find(|p| !p.is_file()) {
        Some(missing) => Err(format!("{} is missing (scripts/{script} puts it there)", missing.display())),
        None => Ok(()),
    }
}

fn copy(file: &Path, into: &Path) -> Result<(), String> {
    let name = file.file_name().ok_or_else(|| format!("{} has no file name", file.display()))?;
    std::fs::copy(file, into.join(name)).map(|_| ()).map_err(|e| format!("cannot copy {}: {e}", file.display()))
}

/// `from` as `to`, with everything in it.
fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("cannot create {}: {e}", to.display()))?;
    for e in std::fs::read_dir(from).map_err(|e| format!("cannot read {}: {e}", from.display()))? {
        let e = e.map_err(|e| format!("cannot read {}: {e}", from.display()))?;
        let path = e.path();
        if path.is_dir() {
            copy_dir(&path, &to.join(e.file_name()))?;
        } else {
            std::fs::copy(&path, to.join(e.file_name())).map_err(|err| format!("cannot copy {}: {err}", path.display()))?;
        }
    }
    Ok(())
}

/// Every file under `dir`, as paths relative to it with `/` between the parts.
fn files(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = format!("{prefix}{}", e.file_name().to_string_lossy());
        if e.path().is_dir() {
            files(&e.path(), &format!("{name}/"), out)?;
        } else {
            out.push((name, e.path()));
        }
    }
    Ok(())
}

/// `dir` as `zip`, its files under `top/`, deflated.
fn write_zip(dir: &Path, top: &str, zip: &Path) -> Result<(), String> {
    let mut list = Vec::new();
    files(dir, &format!("{top}/"), &mut list)?;
    let file = std::fs::File::create(zip).map_err(|e| format!("cannot create {}: {e}", zip.display()))?;
    let mut w = zip::ZipWriter::new(file);
    for (name, path) in list {
        let data = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let mut options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        if let Some(t) = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok()).and_then(dos_time) {
            options = options.last_modified_time(t);
        }
        w.start_file(name.as_str(), options).map_err(|e| format!("zip {name}: {e}"))?;
        w.write_all(&data).map_err(|e| format!("zip {name}: {e}"))?;
    }
    w.finish().map_err(|e| format!("cannot finish {}: {e}", zip.display()))?;
    Ok(())
}

/// A file time as the zip format stores it (UTC: the format has no time zone).
fn dos_time(t: std::time::SystemTime) -> Option<zip::DateTime> {
    let secs = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    let (y, m, d) = civil_from_days(days);
    zip::DateTime::from_date_and_time(u16::try_from(y).ok()?, m, d, (rest / 3600) as u8, (rest / 60 % 60) as u8, (rest % 60) as u8).ok()
}

/// Days since 1970-01-01 → (year, month, day) in the proleptic Gregorian calendar.
fn civil_from_days(z: i64) -> (i64, u8, u8) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_version() {
        let toml = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.3.0\"\nedition = \"2021\"\n[dependencies]\nx = { version = \"1\" }\n";
        assert_eq!(version(toml).as_deref(), Some("0.3.0"));
        assert_eq!(version("  version = \"9\"\n").as_deref(), None);
    }

    #[test]
    fn calendar_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_418), (2025, 11, 26));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn zip_holds_every_file_under_the_top_folder() {
        let tmp = std::env::temp_dir().join(format!("brisk-xtask-zip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let dir = tmp.join("pkg");
        std::fs::create_dir_all(dir.join("lib").join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "aaaa").unwrap();
        std::fs::write(dir.join("lib").join("sub").join("b.py"), "print(1)\n").unwrap();
        let zip_path = tmp.join("pkg.zip");
        write_zip(&dir, "Top", &zip_path).unwrap();
        let mut z = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
        let mut names: Vec<(String, u64)> = (0..z.len()).map(|i| z.by_index(i).map(|f| (f.name().to_string(), f.size())).unwrap()).collect();
        names.sort();
        assert_eq!(names, [("Top/a.txt".to_string(), 4), ("Top/lib/sub/b.py".to_string(), 9)]);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
