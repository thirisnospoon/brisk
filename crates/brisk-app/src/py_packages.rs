//! Packages of a Python interpreter through pip (`python -m pip`), or uv when the interpreter
//! has no pip (uv-made virtual environments). Every call blocks: run them in the background.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tool {
    Pip,
    Uv(PathBuf),
}

impl Tool {
    pub fn label(&self) -> &'static str {
        match self {
            Tool::Pip => "pip",
            Tool::Uv(_) => "uv",
        }
    }
}

fn command(program: &Path) -> Command {
    let mut c = Command::new(program);
    c.env("PYTHONIOENCODING", "utf-8").env("PIP_DISABLE_PIP_VERSION_CHECK", "1").env("PIP_NO_INPUT", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}

fn on_path(exe: &str) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) { vec![format!("{exe}.exe")] } else { vec![exe.to_string()] };
    std::env::split_paths(&std::env::var_os("PATH")?).flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

/// pip when the interpreter has it, else uv from PATH, else pip (its error is shown).
pub fn tool_for(python: &Path) -> Tool {
    let pip = command(python).args(["-m", "pip", "--version"]).output().is_ok_and(|o| o.status.success());
    if pip {
        return Tool::Pip;
    }
    on_path("uv").map(Tool::Uv).unwrap_or(Tool::Pip)
}

fn run(python: &Path, tool: &Tool, args: &[&str]) -> Result<String, String> {
    let out = match tool {
        Tool::Pip => command(python).args(["-m", "pip"]).args(args).output(),
        Tool::Uv(uv) => {
            let py = python.to_string_lossy().to_string();
            let mut a: Vec<&str> = vec!["pip", args[0], "--python", &py];
            // uv has no -y: uninstall never asks.
            a.extend(args[1..].iter().filter(|x| **x != "-y"));
            command(uv).args(&a).output()
        }
    }
    .map_err(|e| format!("Cannot run {}: {e}", tool.label()))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(stdout)
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = err.lines().rev().find(|l| l.trim_start().starts_with("ERROR") || l.contains("error:")).or_else(|| err.lines().rev().find(|l| !l.trim().is_empty())).unwrap_or("failed");
        Err(msg.trim().to_string())
    }
}

pub fn parse_list(json: &str) -> Vec<Package> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).unwrap_or_default();
    let mut list: Vec<Package> = v
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| Some(Package { name: p["name"].as_str()?.to_string(), version: p["version"].as_str().unwrap_or("").to_string() }))
                .collect()
        })
        .unwrap_or_default();
    list.sort_by_key(|p| p.name.to_lowercase());
    list
}

pub fn list(python: &Path, tool: &Tool) -> Result<Vec<Package>, String> {
    run(python, tool, &["list", "--format=json"]).map(|s| parse_list(&s))
}

/// "requests" + "2.31" → "requests==2.31"; a version with its own operator (">=2") is kept.
pub fn requirement(name: &str, version: &str) -> String {
    let (name, version) = (name.trim(), version.trim());
    if version.is_empty() {
        name.to_string()
    } else if version.starts_with(['=', '<', '>', '!', '~']) {
        format!("{name}{version}")
    } else {
        format!("{name}=={version}")
    }
}

pub fn install(python: &Path, tool: &Tool, name: &str, version: &str) -> Result<(), String> {
    run(python, tool, &["install", &requirement(name, version)]).map(|_| ())
}

pub fn uninstall(python: &Path, tool: &Tool, name: &str) -> Result<(), String> {
    run(python, tool, &["uninstall", "-y", name]).map(|_| ())
}

/// `pip install --upgrade name`.
pub fn upgrade(python: &Path, tool: &Tool, name: &str) -> Result<(), String> {
    run(python, tool, &["install", "--upgrade", name]).map(|_| ())
}

/// Normalized project name (PEP 503): `Foo_Bar.baz` → `foo-bar-baz`.
pub fn normalize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.trim().chars() {
        let c = if matches!(c, '_' | '.') { '-' } else { c.to_ascii_lowercase() };
        if !(c == '-' && out.ends_with('-')) {
            out.push(c);
        }
    }
    out
}

/// `pip list --outdated --format=json`: normalized name → latest version.
pub fn parse_outdated(json: &str) -> std::collections::HashMap<String, String> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).unwrap_or_default();
    v.as_array()
        .map(|a| a.iter().filter_map(|p| Some((normalize(p["name"].as_str()?), p["latest_version"].as_str()?.to_string()))).collect())
        .unwrap_or_default()
}

pub fn outdated(python: &Path, tool: &Tool) -> Result<std::collections::HashMap<String, String>, String> {
    run(python, tool, &["list", "--outdated", "--format=json"]).map(|s| parse_outdated(&s))
}

/// PyPI's Simple index (every project name); `BRISK_PYPI_INDEX` overrides it (a mirror, or a
/// `file://` list for tests).
fn index_url() -> String {
    std::env::var("BRISK_PYPI_INDEX").unwrap_or_else(|_| "https://pypi.org/simple/".into())
}

/// Fetches the index with the interpreter's own urllib (no HTTP client in Brisk) and prints
/// one name per line.
const FETCH_NAMES: &str = r#"
import gzip, json, sys, urllib.request
req = urllib.request.Request(sys.argv[1], headers={"Accept": "application/vnd.pypi.simple.v1+json", "Accept-Encoding": "gzip", "User-Agent": "brisk"})
with urllib.request.urlopen(req, timeout=120) as r:
    data = r.read()
    if (r.headers.get("Content-Encoding") or "") == "gzip":
        data = gzip.decompress(data)
names = [p["name"] for p in json.loads(data)["projects"]]
sys.stdout.write("\n".join(names))
"#;

fn names_cache() -> PathBuf {
    brisk_db::config_dir().join("pypi-names.txt")
}

/// All project names on PyPI: from a cache younger than a day, else fetched (several MB:
/// call it in the background) and cached.
pub fn index_names(python: &Path) -> Result<Vec<String>, String> {
    // Smokes never reach the network: they pass a local index or get no suggestions.
    if std::env::var_os("BRISK_PYPI_INDEX").is_none() && std::env::vars_os().any(|(k, _)| k.to_string_lossy().starts_with("BRISK_SMOKE_")) {
        return Err("No PyPI index in smoke tests (set BRISK_PYPI_INDEX)".into());
    }
    let cache = names_cache();
    let fresh = std::fs::metadata(&cache).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < std::time::Duration::from_secs(24 * 3600));
    if fresh && std::env::var_os("BRISK_PYPI_INDEX").is_none() {
        if let Ok(text) = std::fs::read_to_string(&cache) {
            return Ok(text.lines().map(str::to_string).collect());
        }
    }
    let out = command(python).args(["-c", FETCH_NAMES, &index_url()]).output().map_err(|e| format!("Cannot run Python: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("Cannot load the PyPI package list: {}", err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("failed").trim()));
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if std::env::var_os("BRISK_PYPI_INDEX").is_none() {
        let _ = std::fs::create_dir_all(cache.parent().unwrap_or(Path::new(".")));
        let _ = std::fs::write(&cache, &text);
    }
    Ok(text.lines().map(str::to_string).collect())
}

/// Names with their normalized form, for [`suggest_in`] (built once: PyPI has ~10^6).
pub fn name_index(names: Vec<String>) -> Vec<(String, String)> {
    names.into_iter().map(|n| (normalize(&n), n)).collect()
}

/// Up to `limit` names for what was typed: the exact name, then names starting with it
/// (shortest first), then names containing it.
#[cfg(test)]
pub fn suggest(names: &[String], typed: &str, limit: usize) -> Vec<String> {
    suggest_in(&name_index(names.to_vec()), typed, limit)
}

pub fn suggest_in(index: &[(String, String)], typed: &str, limit: usize) -> Vec<String> {
    let q = normalize(typed);
    if q.is_empty() {
        return Vec::new();
    }
    let mut starts: Vec<(usize, &String, &String)> = Vec::new();
    let mut contains: Vec<(usize, &String, &String)> = Vec::new();
    for (k, n) in index {
        if k.starts_with(&q) {
            starts.push((k.len(), k, n));
        } else if contains.len() < limit * 4 && k.contains(q.as_str()) {
            contains.push((k.len(), k, n));
        }
    }
    starts.sort();
    contains.sort();
    starts.into_iter().chain(contains).map(|(_, _, n)| n.clone()).take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_json_and_requirements() {
        let l = parse_list(r#"[{"name": "requests", "version": "2.32.3"}, {"name": "Babel", "version": "2.1"}, {"bad": 1}]"#);
        assert_eq!(l, vec![Package { name: "Babel".into(), version: "2.1".into() }, Package { name: "requests".into(), version: "2.32.3".into() }]);
        assert!(parse_list("not json").is_empty());
        assert_eq!(requirement(" requests ", ""), "requests");
        assert_eq!(requirement("requests", "2.31"), "requests==2.31");
        assert_eq!(requirement("requests", ">=2,<3"), "requests>=2,<3");
    }

    #[test]
    fn outdated_and_suggestions() {
        let o = parse_outdated(r#"[{"name": "Flask_Login", "version": "0.5", "latest_version": "0.6.3", "latest_filetype": "wheel"}]"#);
        assert_eq!(o.get("flask-login").map(String::as_str), Some("0.6.3"));
        let names: Vec<String> = ["requests-oauthlib", "requests", "grequests", "Requests_Toolbelt", "flask"].iter().map(|s| s.to_string()).collect();
        assert_eq!(suggest(&names, "Requests", 10), ["requests", "requests-oauthlib", "Requests_Toolbelt", "grequests"]);
        assert_eq!(suggest(&names, "requests_t", 10), ["Requests_Toolbelt"]);
        assert!(suggest(&names, " ", 10).is_empty());
    }

    /// Needs `python` with pip on PATH; skipped otherwise.
    #[test]
    fn pip_lists_itself() {
        let Some(py) = on_path("python") else { return };
        let tool = tool_for(&py);
        if tool != Tool::Pip {
            return;
        }
        let l = list(&py, &tool).unwrap();
        assert!(l.iter().any(|p| p.name == "pip" && !p.version.is_empty()), "{l:?}");
        let err = uninstall(&py, &tool, "brisk-surely-not-installed-pkg");
        // pip only warns about a package that is not installed.
        assert!(err.is_ok(), "{err:?}");
    }
}
