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
    offline_in_smoke(&mut c);
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
        let msg = err
            .lines()
            .rev()
            .find(|l| l.trim_start().starts_with("ERROR") || l.contains("error:"))
            .or_else(|| err.lines().rev().find(|l| !l.trim().is_empty()))
            .unwrap_or("failed");
        Err(msg.trim().to_string())
    }
}

pub fn parse_list(json: &str) -> Vec<Package> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).unwrap_or_default();
    let mut list: Vec<Package> = v
        .as_array()
        .map(|a| {
            a.iter().filter_map(|p| Some(Package { name: p["name"].as_str()?.to_string(), version: p["version"].as_str().unwrap_or("").to_string() })).collect()
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

/// Smokes never reach the network: without a local index of their own pip gets none. Every
/// pip call made under a smoke goes through this.
pub(crate) fn offline_in_smoke(c: &mut Command) {
    if crate::smoke::active() && std::env::var_os("PIP_INDEX_URL").is_none() {
        c.env("PIP_NO_INDEX", "1");
    }
}

const PYPI: &str = "https://pypi.org/simple/";

/// PyPI's Simple index (every project name); `BRISK_PYPI_INDEX` overrides it (a mirror, or a
/// `file://` JSON for tests).
fn index_url() -> String {
    std::env::var("BRISK_PYPI_INDEX").unwrap_or_else(|_| PYPI.into())
}

/// Fetches the index with the interpreter's own urllib (no HTTP client in Brisk) and writes
/// one name per line to the file in argv[2] (through a temporary file, so a reader never sees
/// half of it): the normalized name, then a tab and the name as published when that differs.
/// Brisk itself never holds the list.
const FETCH_NAMES: &str = r#"
import gzip, json, os, sys, urllib.request
req = urllib.request.Request(sys.argv[1], headers={"Accept": "application/vnd.pypi.simple.v1+json", "Accept-Encoding": "gzip", "User-Agent": "brisk"})
with urllib.request.urlopen(req, timeout=120) as r:
    data = r.read()
    if (r.headers.get("Content-Encoding") or "") == "gzip":
        data = gzip.decompress(data)
import re
names = [p["name"] for p in json.loads(data)["projects"]]
del data
def line(n):
    k = re.sub(r"[-_.]+", "-", n.strip()).lower()
    return k if k == n else k + "\t" + n
tmp = sys.argv[2] + ".tmp"
with open(tmp, "w", encoding="utf-8", newline="\n") as f:
    f.write("\n".join(map(line, names)))
os.replace(tmp, sys.argv[2])
"#;

/// The cached names of `url`: `pypi-names.txt` for PyPI, a file per mirror otherwise.
fn names_cache(url: &str) -> PathBuf {
    let dir = brisk_db::config_dir();
    if url == PYPI {
        // v2: normalized keys (the first list kept the published names only).
        let _ = std::fs::remove_file(dir.join("pypi-names.txt"));
        return dir.join("pypi-names-v2.txt");
    }
    let h = url.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    dir.join(format!("pypi-names-v2-{:08x}.txt", h as u32))
}

/// The file listing every project name on the index, one per line: the cache when it is
/// younger than a day, else fetched into it (several MB: call it in the background).
pub fn index_file(python: &Path) -> Result<PathBuf, String> {
    let url = index_url();
    // Smokes never reach the network: they pass a local index or get no suggestions.
    if crate::smoke::active() && !url.starts_with("file:") {
        return Err("No PyPI index in smoke tests (set BRISK_PYPI_INDEX to a file:// URL)".into());
    }
    let cache = names_cache(&url);
    let fresh = std::fs::metadata(&cache)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < std::time::Duration::from_secs(24 * 3600));
    // A local file is cheap to read again (and tests change theirs).
    if fresh && !url.starts_with("file:") {
        return Ok(cache);
    }
    let _ = std::fs::create_dir_all(cache.parent().unwrap_or(Path::new(".")));
    let out = command(python).args(["-c", FETCH_NAMES, &url, &cache.to_string_lossy()]).output().map_err(|e| format!("Cannot run Python: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("Cannot load the PyPI package list: {}", err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("failed").trim()));
    }
    Ok(cache)
}

/// Picks suggestions from the matching lines of the index, keeping only the best few: the
/// exact name, then names starting with the query (shortest first), then names containing it.
struct Picker {
    q: String,
    limit: usize,
    starts: Vec<(usize, String, String)>,
    contains: Vec<(usize, String, String)>,
}

impl Picker {
    fn new(typed: &str, limit: usize) -> Option<Self> {
        let q = normalize(typed);
        (!q.is_empty()).then(|| Picker { q, limit, starts: Vec::new(), contains: Vec::new() })
    }

    /// Every line of `text` (index lines: `key` or `key\tName`) whose key contains the query.
    /// The search runs over the whole text with the standard library's substring search,
    /// so a million lines take milliseconds.
    fn scan(&mut self, text: &str) {
        let mut last_line = usize::MAX;
        for (pos, _) in text.match_indices(self.q.as_str()) {
            let start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
            if start == last_line {
                continue;
            }
            let end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
            let line = text[start..end].trim_end_matches('\r');
            let (key, name) = line.split_once('\t').unwrap_or((line, line));
            if pos - start + self.q.len() > key.len() {
                // The hit is in the published name, not the key.
                continue;
            }
            last_line = start;
            let list = if pos == start {
                &mut self.starts
            } else if self.contains.len() < self.limit * 4 {
                &mut self.contains
            } else {
                continue;
            };
            list.push((key.len(), key.to_string(), name.to_string()));
            if list.len() > self.limit * 16 {
                list.sort();
                list.truncate(self.limit);
            }
        }
    }

    fn finish(mut self) -> Vec<String> {
        self.starts.sort();
        self.contains.sort();
        self.starts.into_iter().chain(self.contains).map(|(_, _, n)| n).take(self.limit).collect()
    }
}

/// An index line for `name` (as the fetch script writes it).
#[cfg(test)]
fn index_line(name: &str) -> String {
    let k = normalize(name);
    if k == name {
        k
    } else {
        format!("{k}\t{name}")
    }
}

/// Up to `limit` names for what was typed, from a list in memory.
#[cfg(test)]
pub fn suggest(names: &[String], typed: &str, limit: usize) -> Vec<String> {
    let Some(mut p) = Picker::new(typed, limit) else { return Vec::new() };
    p.scan(&names.iter().map(|n| index_line(n)).collect::<Vec<_>>().join("\n"));
    p.finish()
}

/// Up to `limit` names for what was typed, read straight from the index file in 1 MB pieces
/// (run it in the background; nothing stays in memory). `wanted` is asked between pieces;
/// when it says no (a newer query came) the pass stops.
pub fn suggest_file(path: &Path, typed: &str, limit: usize, wanted: &dyn Fn() -> bool) -> Vec<String> {
    use std::io::Read;
    let Some(mut p) = Picker::new(typed, limit) else { return Vec::new() };
    let Ok(mut f) = std::fs::File::open(path) else { return Vec::new() };
    let mut buf: Vec<u8> = Vec::with_capacity(1 << 20);
    let mut chunk = vec![0u8; 1 << 20];
    while let Ok(n) = f.read(&mut chunk) {
        buf.extend_from_slice(&chunk[..n]);
        // Whole lines only; the rest waits for the next piece (or is the last line).
        let cut = if n == 0 { buf.len() } else { buf.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1) };
        p.scan(&String::from_utf8_lossy(&buf[..cut]));
        buf.drain(..cut);
        if n == 0 || !wanted() {
            break;
        }
    }
    if !wanted() {
        return Vec::new();
    }
    p.finish()
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
        // The same from a file.
        let f = std::env::temp_dir().join(format!("brisk-names-{}.txt", std::process::id()));
        std::fs::write(&f, names.iter().map(|n| index_line(n)).collect::<Vec<_>>().join("\n")).unwrap();
        assert_eq!(suggest_file(&f, "Requests", 10, &|| true), ["requests", "requests-oauthlib", "Requests_Toolbelt", "grequests"]);
        let _ = std::fs::remove_file(f);
        // Many names starting with the query: only the best few are kept.
        let many: Vec<String> = (0..5000).map(|i| format!("req-{i}")).chain(["req".to_string()]).collect();
        assert_eq!(suggest(&many, "req", 3), ["req", "req-0", "req-1"]);
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
