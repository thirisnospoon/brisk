//! Python project managers: uv and Poetry. Which one a project uses (from its lock file or
//! `pyproject.toml`), where the tool is installed, and the command lines for syncing the
//! environment and for adding, removing and upgrading dependencies. Running them is up to
//! the caller; only [`poetry_env_python`] starts a process.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Manager {
    Uv,
    Poetry,
}

impl Manager {
    pub fn name(self) -> &'static str {
        match self {
            Manager::Uv => "uv",
            Manager::Poetry => "Poetry",
        }
    }

    pub fn lock_file(self) -> &'static str {
        match self {
            Manager::Uv => "uv.lock",
            Manager::Poetry => "poetry.lock",
        }
    }

    fn exe(self) -> &'static str {
        match self {
            Manager::Uv => "uv",
            Manager::Poetry => "poetry",
        }
    }

    /// Creates or updates the environment from the lock file (locking first when needed).
    pub fn sync_args(self) -> Vec<String> {
        match self {
            Manager::Uv => vec!["sync".into()],
            Manager::Poetry => vec!["install".into()],
        }
    }

    /// Re-resolves the lock file from `pyproject.toml` without touching the environment.
    pub fn lock_args(self) -> Vec<String> {
        vec!["lock".into()]
    }

    /// Adds a dependency to `pyproject.toml`, locks and installs it.
    pub fn add_args(self, requirement: &str, dev: bool) -> Vec<String> {
        let mut a = vec!["add".to_string()];
        if dev {
            match self {
                Manager::Uv => a.push("--dev".into()),
                Manager::Poetry => a.extend(["--group".into(), "dev".into()]),
            }
        }
        a.push(requirement.into());
        a
    }

    /// Removes a dependency from `pyproject.toml` (whichever group it is in), locks and
    /// uninstalls it.
    pub fn remove_args(self, name: &str, dev: bool) -> Vec<String> {
        let mut a = vec!["remove".to_string()];
        if dev {
            match self {
                Manager::Uv => a.push("--dev".into()),
                Manager::Poetry => a.extend(["--group".into(), "dev".into()]),
            }
        }
        a.push(name.into());
        a
    }

    /// Upgrades one dependency within the constraints of `pyproject.toml`.
    pub fn upgrade_args(self, name: &str) -> Vec<String> {
        match self {
            Manager::Uv => vec!["sync".into(), "--upgrade-package".into(), name.into()],
            Manager::Poetry => vec!["update".into(), name.into()],
        }
    }
}

/// `[section]` headers of a TOML text, in order (tables and arrays of tables).
fn sections(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter_map(|l| {
        let l = l.trim();
        let l = l.strip_prefix("[[").and_then(|s| s.strip_suffix("]]")).or_else(|| l.strip_prefix('[').and_then(|s| s.strip_suffix(']')))?;
        Some(l.trim())
    })
}

/// The manager a project uses: its lock file decides, then `[tool.uv]` / `[tool.poetry]` in
/// `pyproject.toml`. `None` for plain pip projects.
pub fn detect(root: &Path) -> Option<Manager> {
    if root.join("uv.lock").is_file() {
        return Some(Manager::Uv);
    }
    if root.join("poetry.lock").is_file() {
        return Some(Manager::Poetry);
    }
    let text = std::fs::read_to_string(root.join("pyproject.toml")).ok()?;
    detect_pyproject(&text)
}

pub fn detect_pyproject(text: &str) -> Option<Manager> {
    for s in sections(text) {
        if s == "tool.poetry" || s.starts_with("tool.poetry.") {
            return Some(Manager::Poetry);
        }
        if s == "tool.uv" || s.starts_with("tool.uv.") {
            return Some(Manager::Uv);
        }
    }
    None
}

/// The names of the project's development dependencies: `[dependency-groups].dev` (PEP 735,
/// uv), `[tool.uv].dev-dependencies` and `[tool.poetry.group.dev.dependencies]` /
/// `[tool.poetry.dev-dependencies]`. Normalized like PyPI names (`Foo_Bar` → `foo-bar`).
pub fn dev_dependencies(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut section = String::new();
    let mut in_list = false;
    for raw in text.lines() {
        let line = raw.split(" #").next().unwrap_or(raw).trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if !in_list && line.starts_with('[') && !line.starts_with("[\"") && !line.starts_with("['") {
            section = sections(line).next().unwrap_or("").to_string();
            continue;
        }
        let table = matches!(section.as_str(), "tool.poetry.group.dev.dependencies" | "tool.poetry.dev-dependencies");
        if table {
            if let Some((k, _)) = line.split_once('=') {
                let k = k.trim().trim_matches('"');
                if k != "python" {
                    out.push(normalize(k));
                }
            }
            continue;
        }
        let list_start = (section == "dependency-groups" && line.split_once('=').is_some_and(|(k, _)| k.trim() == "dev"))
            || (section == "tool.uv" && line.split_once('=').is_some_and(|(k, _)| k.trim() == "dev-dependencies"));
        let rest = if in_list {
            line
        } else if list_start {
            in_list = true;
            line.split_once('=').map_or("", |(_, v)| v)
        } else {
            continue;
        };
        let mut s = rest;
        while let Some(q) = s.find(['"', '\'']) {
            let quote = &s[q..q + 1];
            let Some(end) = s[q + 1..].find(quote) else { break };
            let req = &s[q + 1..q + 1 + end];
            let name: String = req.chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
            if !name.is_empty() {
                out.push(normalize(&name));
            }
            s = &s[q + 2 + end..];
        }
        if rest.rfind(['"', '\'']).map_or(rest, |i| &rest[i..]).contains(']') {
            in_list = false;
        }
    }
    out
}

/// A hash of the parts of `pyproject.toml` that the lock file depends on: the project's name,
/// version, Python requirement and dependencies, `[dependency-groups]`, and the uv and Poetry
/// tables. Editing anything else (`[tool.ruff]`, the description) keeps the hash.
pub fn deps_fingerprint(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    const PROJECT_KEYS: &[&str] = &["name", "version", "requires-python", "dependencies", "optional-dependencies"];
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut section = String::new();
    let mut key_kept = false;
    for raw in text.lines() {
        let line = raw.split(" #").next().unwrap_or(raw).trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && sections(line).next().is_some() && !line.starts_with("[\"") && !line.starts_with("['") {
            section = sections(line).next().unwrap_or("").to_string();
            key_kept = false;
            continue;
        }
        let relevant_section = section.starts_with("project.optional-dependencies")
            || section == "dependency-groups"
            || section == "tool.uv"
            || section.starts_with("tool.uv.")
            || section == "tool.poetry"
            || section.starts_with("tool.poetry.");
        let keep = if relevant_section {
            true
        } else if section == "project" {
            // A key starts a value that may continue over several lines (arrays).
            if let Some((k, _)) = line.split_once('=').filter(|(k, _)| !k.contains(['"', '\'', '[', ','])) {
                key_kept = PROJECT_KEYS.contains(&k.trim());
            }
            key_kept
        } else {
            false
        };
        if keep {
            section.hash(&mut h);
            line.hash(&mut h);
        }
    }
    h.finish()
}

/// PEP 503 name: lower case, runs of `-_.` become `-`.
pub fn normalize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut dash = false;
    for c in name.trim().chars() {
        if matches!(c, '-' | '_' | '.') {
            dash = true;
        } else {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// Where the tool lives: next to the project interpreter (installed into the venv), on `PATH`,
/// or in the standard places of its official installers.
pub fn find_tool(manager: Manager, project_python: Option<&Path>) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{}.exe", manager.exe()) } else { manager.exe().to_string() };
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(p) = project_python.and_then(Path::parent) {
        dirs.push(p.to_path_buf());
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    if let Some(h) = &home {
        // uv's and Poetry's standalone installers, and pipx.
        dirs.push(h.join(".local").join("bin"));
        dirs.push(h.join(".cargo").join("bin"));
    }
    if let Some(a) = brisk_platform::dirs::roaming_app_data() {
        dirs.push(a.join("Python").join("Scripts"));
        dirs.push(a.join("pypoetry").join("venv").join("Scripts"));
    }
    dirs.into_iter().map(|d| d.join(&exe)).find(|p| p.is_file())
}

/// Whether the lock file is missing or older than `pyproject.toml`.
pub fn lock_outdated(root: &Path, manager: Manager) -> bool {
    let modified = |p: PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    match (modified(root.join("pyproject.toml")), modified(root.join(manager.lock_file()))) {
        (Some(_), None) => true,
        (Some(p), Some(l)) => p > l,
        _ => false,
    }
}

/// The interpreter of the project's Poetry environment (`poetry env info --executable`), which
/// usually lives outside the project. Starts Poetry: call it off the UI thread.
pub fn poetry_env_python(poetry: &Path, root: &Path) -> Option<PathBuf> {
    let mut cmd = Command::new(poetry);
    cmd.args(["env", "info", "--executable", "--no-interaction"]).current_dir(root).stdin(Stdio::null()).stderr(Stdio::null()).stdout(Stdio::piped());
    brisk_platform::process::no_window(&mut cmd);
    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut s);
        s
    });
    let t0 = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if t0.elapsed() > Duration::from_secs(10) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let out = reader.join().ok()?;
    let p = PathBuf::from(out.lines().map(str::trim).find(|l| !l.is_empty())?);
    p.is_file().then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_from_pyproject() {
        assert_eq!(detect_pyproject("[project]\nname = \"x\"\n\n[tool.uv]\ndev-dependencies = []\n"), Some(Manager::Uv));
        assert_eq!(detect_pyproject("[tool.poetry]\nname = \"x\"\n"), Some(Manager::Poetry));
        assert_eq!(detect_pyproject("[tool.poetry.dependencies]\npython = \"^3.11\"\n"), Some(Manager::Poetry));
        assert_eq!(detect_pyproject("[project]\nname = \"x\"\n[tool.ruff]\n"), None);
        assert_eq!(detect_pyproject("[tool.uvicorn]\n"), None);
    }

    #[test]
    fn lock_file_wins() {
        let dir = std::env::temp_dir().join(format!("brisk-pyproj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(detect(&dir), None);
        std::fs::write(dir.join("pyproject.toml"), "[tool.poetry]\nname = \"x\"\n").unwrap();
        assert_eq!(detect(&dir), Some(Manager::Poetry));
        assert!(lock_outdated(&dir, Manager::Poetry));
        std::fs::write(dir.join("uv.lock"), "version = 1\n").unwrap();
        assert_eq!(detect(&dir), Some(Manager::Uv));
        assert!(!lock_outdated(&dir, Manager::Uv));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_lines() {
        assert_eq!(Manager::Uv.add_args("requests>=2", false), ["add", "requests>=2"]);
        assert_eq!(Manager::Uv.add_args("pytest", true), ["add", "--dev", "pytest"]);
        assert_eq!(Manager::Poetry.add_args("pytest", true), ["add", "--group", "dev", "pytest"]);
        assert_eq!(Manager::Poetry.remove_args("rich", false), ["remove", "rich"]);
        assert_eq!(Manager::Uv.upgrade_args("rich"), ["sync", "--upgrade-package", "rich"]);
        assert_eq!(Manager::Poetry.upgrade_args("rich"), ["update", "rich"]);
        assert_eq!(Manager::Uv.sync_args(), ["sync"]);
        assert_eq!(Manager::Poetry.sync_args(), ["install"]);
    }

    #[test]
    fn dev_dependency_names() {
        let uv = "[project]\ndependencies = [\"requests\"]\n\n[dependency-groups]\ndev = [\n  \"pytest>=8\",  # tests\n  'Ruff_Lint',\n]\nlint = [\"mypy\"]\n";
        assert_eq!(dev_dependencies(uv), vec!["pytest", "ruff-lint"]);
        let old_uv = "[tool.uv]\ndev-dependencies = [\"pytest\", \"coverage[toml]>=7\"]\n";
        assert_eq!(dev_dependencies(old_uv), vec!["pytest", "coverage"]);
        let poetry = "[tool.poetry.dependencies]\npython = \"^3.12\"\nrich = \"*\"\n\n[tool.poetry.group.dev.dependencies]\npytest = \"^8\"\nblack = {version = \"*\"}\n";
        assert_eq!(dev_dependencies(poetry), vec!["pytest", "black"]);
    }

    #[test]
    fn fingerprint_ignores_unrelated_edits() {
        let base = "[project]\nname = \"x\"\ndescription = \"a\"\ndependencies = [\n  \"rich\",\n]\n\n[tool.ruff]\nline-length = 100\n";
        let fp = deps_fingerprint(base);
        assert_eq!(fp, deps_fingerprint(&base.replace("line-length = 100", "line-length = 120")));
        assert_eq!(fp, deps_fingerprint(&base.replace("description = \"a\"", "description = \"b\"")));
        assert_eq!(fp, deps_fingerprint(&base.replace("  \"rich\",\n", "  \"rich\",  # pretty output\n")));
        assert_ne!(fp, deps_fingerprint(&base.replace("\"rich\"", "\"rich>=13\"")));
        assert_ne!(fp, deps_fingerprint(&format!("{base}\n[dependency-groups]\ndev = [\"pytest\"]\n")));
        let poetry = "[tool.poetry.dependencies]\npython = \"^3.12\"\n";
        assert_ne!(deps_fingerprint(poetry), deps_fingerprint(&poetry.replace("^3.12", "^3.11")));
    }

    #[test]
    fn normalizes_names() {
        assert_eq!(normalize("Foo_Bar.baz"), "foo-bar-baz");
        assert_eq!(normalize("  requests "), "requests");
    }
}
