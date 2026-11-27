//! Where TypeScript 7's native `tsc` comes from: the project's own `node_modules` (its
//! platform package), else the copy Brisk downloaded into its settings folder. Downloading
//! fetches only the one platform package from the npm registry (about 10 MB) through the
//! system's `curl` and unpacks it with its `tar`.

use std::path::{Path, PathBuf};
use std::process::Stdio;

/// npm's name for this platform's native package: `@typescript/typescript-win32-x64`.
pub fn platform_package() -> Option<String> {
    let os = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        "linux" => "linux",
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return None,
    };
    Some(format!("@typescript/typescript-{os}-{arch}"))
}

fn exe() -> &'static str {
    if cfg!(windows) {
        "tsc.exe"
    } else {
        "tsc"
    }
}

/// Brisk's own copies: `<settings>/tools/typescript/<version>/lib/tsc`.
pub fn tools_dir() -> PathBuf {
    brisk_platform::dirs::config_dir().join("tools").join("typescript")
}

/// The native `tsc` of the project (`node_modules` in the root or a folder above it).
pub fn in_project(root: &Path) -> Option<PathBuf> {
    let pkg = platform_package()?;
    root.ancestors().map(|d| d.join("node_modules").join(&pkg).join("lib").join(exe())).find(|p| p.is_file())
}

/// The newest copy Brisk downloaded.
pub fn downloaded() -> Option<PathBuf> {
    let mut versions: Vec<(Vec<u32>, PathBuf)> = std::fs::read_dir(tools_dir())
        .ok()?
        .flatten()
        .filter_map(|e| {
            let v: Vec<u32> = e.file_name().to_str()?.split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
            let tsc = e.path().join("lib").join(exe());
            tsc.is_file().then_some((v, tsc))
        })
        .collect();
    versions.sort();
    versions.pop().map(|(_, p)| p)
}

/// `BRISK_TSC` (smokes and tests), the project's, else Brisk's download.
pub fn find(root: &Path) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("BRISK_TSC") {
        return Some(PathBuf::from(p)).filter(|p| p.is_file());
    }
    in_project(root).or_else(downloaded)
}

pub(crate) fn system_tool(name: &str) -> PathBuf {
    if cfg!(windows) {
        if let Some(root) = std::env::var_os("SystemRoot") {
            let p = PathBuf::from(root).join("System32").join(format!("{name}.exe"));
            if p.is_file() {
                return p;
            }
        }
    }
    PathBuf::from(name)
}

pub(crate) fn run_tool(c: &mut std::process::Command, what: &str) -> Result<Vec<u8>, String> {
    let out = c.stdin(Stdio::null()).output().map_err(|e| format!("cannot start {what}: {e}"))?;
    if !out.status.success() {
        let e = String::from_utf8_lossy(&out.stderr);
        return Err(format!("{what}: {}", e.trim().trim_start_matches("curl: ")));
    }
    Ok(out.stdout)
}

pub(crate) fn curl(url: &str) -> std::process::Command {
    let mut c = brisk_platform::process::command(system_tool("curl"));
    c.args(["--silent", "--show-error", "--fail", "--location", "--connect-timeout", "10", "--max-time", "600"]).arg(url);
    c
}

/// Downloads the latest TypeScript 7 for this platform into [`tools_dir`] (blocking: run it
/// off the UI thread); the path of its `tsc`.
pub fn download() -> Result<PathBuf, String> {
    let pkg = platform_package().ok_or("TypeScript has no native build for this platform")?;
    let meta = run_tool(curl(&format!("https://registry.npmjs.org/{pkg}/latest")).args(["-H", "Accept: application/json"]), "npm registry")?;
    let meta: serde_json::Value = serde_json::from_slice(&meta).map_err(|e| format!("npm registry: {e}"))?;
    let version = meta["version"].as_str().ok_or("npm registry: no version")?.to_string();
    let tarball = meta.pointer("/dist/tarball").and_then(|v| v.as_str()).ok_or("npm registry: no tarball")?;
    if !version.starts_with(|c: char| c.is_ascii_digit()) || version.split('.').next().and_then(|m| m.parse::<u32>().ok()).is_none_or(|m| m < 7) {
        return Err(format!("unexpected TypeScript version {version}"));
    }
    let dir = tools_dir().join(&version);
    let tsc = dir.join("lib").join(exe());
    if tsc.is_file() {
        return Ok(tsc);
    }
    let tmp = tools_dir().join(format!(".{version}.part"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
    let archive = tmp.join("package.tgz");
    run_tool(curl(tarball).arg("--output").arg(&archive), "download")?;
    // npm tarballs hold everything under `package/`.
    run_tool(brisk_platform::process::command(system_tool("tar")).arg("-xzf").arg(&archive).arg("-C").arg(&tmp).args(["--strip-components", "1"]), "tar")?;
    let _ = std::fs::remove_file(&archive);
    if !tmp.join("lib").join(exe()).is_file() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("the package has no tsc".into());
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&tmp, &dir).map_err(|e| format!("cannot move {}: {e}", dir.display()))?;
    Ok(tsc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_project_copy() {
        let root = std::env::temp_dir().join(format!("brisk-tsc-find-{}", std::process::id()));
        let Some(pkg) = platform_package() else { return };
        let lib = root.join("node_modules").join(&pkg).join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join(exe()), "").unwrap();
        std::fs::create_dir_all(root.join("apps/web")).unwrap();
        assert_eq!(in_project(&root.join("apps/web")), Some(lib.join(exe())));
        let _ = std::fs::remove_dir_all(&root);
    }
}
