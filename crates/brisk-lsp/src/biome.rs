//! Biome, the Rust linter and formatter for JS, TS, JSON, CSS and GraphQL. Brisk uses it the
//! way PyCharm's Biome plugin does: in projects that have it (a `biome.json` or the package
//! in `node_modules`), or everywhere when Settings asks for that. The executable comes from
//! the project's `node_modules`, then `PATH`, then the copy Brisk downloaded into its
//! settings folder (one platform package from the npm registry, about 38 MB).
//!
//! Its language server (`biome lsp-proxy`) gives the problems as you type and the quick
//! fixes; Reformat Code and Fix All run the command line on the text, which keeps nothing
//! in memory.

use super::typescript::{curl, run_tool, system_tool};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// npm's name for this platform's package: `@biomejs/cli-win32-x64`.
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
    Some(format!("@biomejs/cli-{os}-{arch}"))
}

fn exe() -> &'static str {
    if cfg!(windows) {
        "biome.exe"
    } else {
        "biome"
    }
}

/// The files Biome checks and formats.
pub fn supports(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    matches!(ext.as_str(), "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "json" | "jsonc" | "css" | "graphql" | "gql")
        && !name.ends_with(".min.js")
        && !name.ends_with(".min.css")
        && name != "package-lock.json"
}

/// The project's Biome configuration: `biome.json` or `biome.jsonc` in the root.
pub fn config(root: &Path) -> Option<PathBuf> {
    ["biome.json", "biome.jsonc"].into_iter().map(|n| root.join(n)).find(|p| p.is_file())
}

/// Brisk's own copies: `<settings>/tools/biome/<version>/biome(.exe)`.
pub fn tools_dir() -> PathBuf {
    brisk_platform::dirs::config_dir().join("tools").join("biome")
}

/// The project's Biome (`node_modules` in the root or a folder above it).
pub fn in_project(root: &Path) -> Option<PathBuf> {
    let pkg = platform_package()?;
    root.ancestors().map(|d| d.join("node_modules").join(&pkg).join(exe())).find(|p| p.is_file())
}

fn on_path() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| std::env::split_paths(&path).map(|d| d.join(exe())).find(|p| p.is_file()))
}

/// The newest copy Brisk downloaded.
pub fn downloaded() -> Option<PathBuf> {
    let mut versions: Vec<(Vec<u32>, PathBuf)> = std::fs::read_dir(tools_dir())
        .ok()?
        .flatten()
        .filter_map(|e| {
            let v: Vec<u32> = e.file_name().to_str()?.split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
            let biome = e.path().join(exe());
            biome.is_file().then_some((v, biome))
        })
        .collect();
    versions.sort();
    versions.pop().map(|(_, p)| p)
}

/// `BRISK_BIOME` (smokes and tests), the project's, `PATH`, else Brisk's download.
pub fn find(root: &Path) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("BRISK_BIOME") {
        return Some(PathBuf::from(p)).filter(|p| p.is_file());
    }
    in_project(root).or_else(on_path).or_else(downloaded)
}

/// Whether the project uses Biome: its configuration or its package.
pub fn in_use(root: &Path) -> bool {
    config(root).is_some() || in_project(root).is_some()
}

/// Downloads the latest Biome for this platform into [`tools_dir`] (blocking: run it off the
/// UI thread); the path of the executable.
pub fn download() -> Result<PathBuf, String> {
    let pkg = platform_package().ok_or("Biome has no build for this platform")?;
    let meta = run_tool(curl(&format!("https://registry.npmjs.org/{pkg}/latest")).args(["-H", "Accept: application/json"]), "npm registry")?;
    let meta: serde_json::Value = serde_json::from_slice(&meta).map_err(|e| format!("npm registry: {e}"))?;
    let version = meta["version"].as_str().ok_or("npm registry: no version")?.to_string();
    let tarball = meta.pointer("/dist/tarball").and_then(|v| v.as_str()).ok_or("npm registry: no tarball")?;
    if !version.starts_with(|c: char| c.is_ascii_digit()) || version.contains(['/', '\\']) {
        return Err(format!("unexpected Biome version {version}"));
    }
    let dir = tools_dir().join(&version);
    let biome = dir.join(exe());
    if biome.is_file() {
        return Ok(biome);
    }
    let tmp = tools_dir().join(format!(".{version}.part"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
    let archive = tmp.join("package.tgz");
    run_tool(curl(tarball).arg("--output").arg(&archive), "download")?;
    // npm tarballs hold everything under `package/`.
    run_tool(brisk_platform::process::command(system_tool("tar")).arg("-xzf").arg(&archive).arg("-C").arg(&tmp).args(["--strip-components", "1"]), "tar")?;
    let _ = std::fs::remove_file(&archive);
    if !tmp.join(exe()).is_file() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("the package has no Biome executable".into());
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&tmp, &dir).map_err(|e| format!("cannot move {}: {e}", dir.display()))?;
    // Older downloads are not needed any more.
    for e in std::fs::read_dir(tools_dir()).into_iter().flatten().flatten() {
        if e.path() != dir && e.file_name().to_str().is_some_and(|n| n.starts_with(|c: char| c.is_ascii_digit())) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    Ok(biome)
}

/// Runs Biome's command line on `text` as if it were `file` (the project's configuration
/// applies to that path); the text it prints.
fn on_text(biome: &Path, root: &Path, args: &[&str], file: &Path, text: &str) -> Result<String, String> {
    let mut c = brisk_platform::process::command(biome);
    c.args(args).arg(format!("--stdin-file-path={}", file.display()));
    c.current_dir(root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| format!("cannot start Biome: {e}"))?;
    brisk_platform::process::adopt(&child);
    let mut input = child.stdin.take().ok_or("no stdin")?;
    let owned = text.to_string();
    let writer = std::thread::spawn(move || {
        let _ = input.write_all(owned.as_bytes());
    });
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let _ = writer.join();
    if !out.status.success() {
        return Err(first_error(&String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The first line of Biome's report that says what went wrong ("× Expected an expression…").
fn first_error(report: &str) -> String {
    report
        .lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix('×').or_else(|| l.strip_prefix('x')).map(str::trim))
        .filter(|l| !l.is_empty())
        .or_else(|| report.lines().map(str::trim).find(|l| !l.is_empty() && !l.contains('━')))
        .unwrap_or("it failed")
        .to_string()
}

/// Reformat Code: the text formatted by the project's Biome settings.
pub fn format(biome: &Path, root: &Path, file: &Path, text: &str) -> Result<String, String> {
    on_text(biome, root, &["format"], file, text).map_err(|e| format!("Biome: {e}"))
}

/// Fix all: Biome's safe fixes, sorted imports and formatting (`biome check --write`).
pub fn fix_all(biome: &Path, root: &Path, file: &Path, text: &str) -> Result<String, String> {
    on_text(biome, root, &["check", "--write"], file, text).map_err(|e| format!("Biome: {e}"))
}

/// Optimize Imports: only Biome's import sorting.
pub fn organize_imports(biome: &Path, root: &Path, file: &Path, text: &str) -> Result<String, String> {
    on_text(biome, root, &["check", "--write", "--linter-enabled=false", "--formatter-enabled=false"], file, text).map_err(|e| format!("Biome: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_files() {
        assert!(supports(Path::new("src/app.tsx")));
        assert!(supports(Path::new("style.css")));
        assert!(supports(Path::new("biome.jsonc")));
        assert!(!supports(Path::new("vendor/jquery.min.js")));
        assert!(!supports(Path::new("package-lock.json")));
        assert!(!supports(Path::new("main.py")));
    }

    #[test]
    fn finds_the_project_copy_and_config() {
        let root = std::env::temp_dir().join(format!("brisk-biome-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let Some(pkg) = platform_package() else { return };
        let dir = root.join("node_modules").join(&pkg);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(exe()), "").unwrap();
        std::fs::create_dir_all(root.join("apps/web")).unwrap();
        assert_eq!(in_project(&root.join("apps/web")), Some(dir.join(exe())));
        assert!(config(&root).is_none());
        std::fs::write(root.join("biome.jsonc"), "{}").unwrap();
        assert_eq!(config(&root), Some(root.join("biome.jsonc")));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// With `BRISK_BIOME` set to a Biome executable: the server's problems for two versions
    /// of a file, a quick fix, and the command line's formatting.
    #[test]
    fn real_biome() {
        let Some(biome) = std::env::var_os("BRISK_BIOME").map(PathBuf::from) else { return };
        let root = std::env::temp_dir().join(format!("brisk-biome-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("app.js");
        let text = "let a = 1
if (a == 1) { debugger }
";
        std::fs::write(&file, text).unwrap();
        let s = crate::Server::start_biome(&biome, &root).unwrap();
        s.did_open(&file, 1, text);
        let d = futures::executor::block_on(s.pushed_diagnostics(&file, 1)).unwrap();
        assert!(d.iter().any(|d| d.code.ends_with("noDebugger")), "{d:?}");
        let debugger = d.iter().find(|d| d.code.ends_with("noDebugger")).unwrap();
        let fixes = futures::executor::block_on(s.code_actions(&file, debugger.start, debugger.end, &[])).unwrap();
        assert!(fixes.iter().any(|f| f.kind.starts_with("quickfix.biome")), "{fixes:?}");
        s.did_change(
            &file,
            2,
            "const a = 1;
console.log(a);
",
        );
        let d = futures::executor::block_on(s.pushed_diagnostics(&file, 2)).unwrap();
        assert!(d.is_empty(), "{d:?}");
        assert_eq!(
            format(&biome, &root, &file, "let b=1").unwrap(),
            "let b = 1;
"
        );
        assert!(format(&biome, &root, &file, "let b =").is_err());
        drop(s);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn error_line() {
        let report = "c.js:2:1 parse ━━━━\n\n  × Expected an expression, or an assignment.\n  \n";
        assert_eq!(first_error(report), "Expected an expression, or an assignment.");
    }
}
