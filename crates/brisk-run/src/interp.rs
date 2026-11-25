//! Python interpreter discovery: project virtualenvs (instant, from `pyvenv.cfg`), then
//! conda / `py -0p` / `PATH` (probed in the background, each with a 3 s limit).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InterpKind {
    Venv,
    Conda,
    System,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interpreter {
    pub path: PathBuf,
    /// "3.12.1"; empty when unknown.
    pub version: String,
    pub kind: InterpKind,
}

impl Interpreter {
    /// "3.12" from "3.12.1".
    pub fn short_version(&self) -> String {
        self.version.split('.').take(2).collect::<Vec<_>>().join(".")
    }

    /// Status bar label, PyCharm style: "Python 3.12 (myproject)" for a project venv,
    /// "Python 3.12 (envname)" for other venvs, "Python 3.12" for a system interpreter.
    pub fn label(&self, root: &Path) -> String {
        let v = if self.version.is_empty() { "Python".to_string() } else { format!("Python {}", self.short_version()) };
        match self.kind {
            InterpKind::System => v,
            InterpKind::Venv | InterpKind::Conda => {
                let Some(dir) = Self::venv_dir_of(&self.path).or_else(|| self.path.parent().map(Path::to_path_buf)) else { return v };
                let dir_name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if let Some(project) = poetry_env_project(&dir_name) {
                    return format!("{v} Poetry ({project})");
                }
                let name = if dir.parent() == Some(root) { root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() } else { dir_name };
                format!("{v} ({name})")
            }
        }
    }

    /// The environment directory for an interpreter inside a venv (`<dir>/Scripts/python.exe`
    /// or `<dir>/bin/python`), detected by `pyvenv.cfg`.
    pub fn venv_dir_of(python: &Path) -> Option<PathBuf> {
        let dir = python.parent()?.parent()?;
        dir.join("pyvenv.cfg").is_file().then(|| dir.to_path_buf())
    }
}

/// `<dir>/Scripts/python.exe` on Windows, `<dir>/bin/python` elsewhere, if it exists.
pub fn venv_python(dir: &Path) -> Option<PathBuf> {
    let candidates: &[(&str, &str)] = if cfg!(windows) { &[("Scripts", "python.exe")] } else { &[("bin", "python"), ("bin", "python3")] };
    candidates.iter().map(|(d, f)| dir.join(d).join(f)).find(|p| p.is_file())
}

/// The project name in a Poetry environment's folder name, `<project>-<8-char hash>-py3.12`.
fn poetry_env_project(dir_name: &str) -> Option<&str> {
    let (rest, py) = dir_name.rsplit_once("-py")?;
    if py.is_empty() || !py.chars().all(|c| c.is_ascii_digit() || c == '.') || rest.len() < 10 || !rest.is_char_boundary(rest.len() - 9) {
        return None;
    }
    let (project, hash) = rest.split_at(rest.len() - 9);
    (hash.starts_with('-') && hash[1..].chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')).then_some(project)
}

/// The Python version a virtual environment records in its `pyvenv.cfg` ("3.12.1"), or "".
pub fn venv_version(dir: &Path) -> String {
    pyvenv_version(dir)
}

fn pyvenv_version(dir: &Path) -> String {
    let Ok(cfg) = std::fs::read_to_string(dir.join("pyvenv.cfg")) else { return String::new() };
    let mut found = String::new();
    for line in cfg.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let k = k.trim();
        let v = v.trim();
        // `version` (venv), `version_info` (uv, virtualenv: "3.12.1.final.0").
        if k == "version" || (k == "version_info" && found.is_empty()) {
            found = v.split('.').take(3).collect::<Vec<_>>().join(".");
        }
    }
    found
}

/// Virtualenvs inside the project, in PyCharm's order of preference. No process is started.
pub fn project_venvs(root: &Path) -> Vec<Interpreter> {
    let mut out = Vec::new();
    for name in [".venv", "venv", "env", ".env"] {
        let dir = root.join(name);
        if !dir.join("pyvenv.cfg").is_file() {
            continue;
        }
        if let Some(path) = venv_python(&dir) {
            out.push(Interpreter { path, version: pyvenv_version(&dir), kind: InterpKind::Venv });
        }
    }
    out
}

fn hidden(cmd: &mut Command) -> &mut Command {
    brisk_platform::process::no_window(cmd);
    cmd.stdin(Stdio::null()).stderr(Stdio::null()).stdout(Stdio::piped())
}

/// Run a command with a time limit and return its stdout.
fn output_within(cmd: &mut Command, limit: Duration) -> Option<String> {
    let mut child = hidden(cmd).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut s);
        s
    });
    let t0 = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => return reader.join().ok(),
            Ok(Some(_)) => return None,
            Ok(None) if t0.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => return None,
        }
    }
}

/// Ask an interpreter for its real executable and version. Resolves launcher stubs such as
/// `WindowsApps\python.exe`; returns `None` for stubs that only open the Store.
pub fn probe(python: &Path) -> Option<Interpreter> {
    let out = output_within(
        Command::new(python).args(["-I", "-c", "import sys;print(sys.executable);print('%d.%d.%d'%sys.version_info[:3]);print(sys.prefix!=sys.base_prefix);print('conda-meta' in __import__('os').listdir(sys.prefix))"]),
        Duration::from_secs(3),
    )?;
    let mut lines = out.lines().map(str::trim);
    let exe = PathBuf::from(lines.next().filter(|l| !l.is_empty())?);
    let version = lines.next().unwrap_or("").to_string();
    let venv = lines.next() == Some("True");
    let conda = lines.next() == Some("True");
    let kind = if venv {
        InterpKind::Venv
    } else if conda {
        InterpKind::Conda
    } else {
        InterpKind::System
    };
    Some(Interpreter { path: exe, version, kind })
}

fn same_file(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().to_lowercase();
    norm(a) == norm(b)
}

/// Every interpreter we can find: project venvs, `$VIRTUAL_ENV`, `$CONDA_PREFIX`, the Windows
/// `py` launcher's list, then `python3`/`python` on `PATH`. Slow (starts processes): call it
/// off the UI thread.
pub fn discover_all(root: &Path) -> Vec<Interpreter> {
    let mut out = project_venvs(root);
    let mut candidates: Vec<PathBuf> = Vec::new();
    // A Poetry project's environment usually lives in Poetry's cache, outside the project.
    if crate::pyproj::detect(root) == Some(crate::pyproj::Manager::Poetry) {
        if let Some(py) = crate::pyproj::find_tool(crate::pyproj::Manager::Poetry, None).and_then(|p| crate::pyproj::poetry_env_python(&p, root)) {
            candidates.push(py);
        }
    }
    for var in ["VIRTUAL_ENV", "CONDA_PREFIX"] {
        if let Some(d) = std::env::var_os(var) {
            let d = PathBuf::from(d);
            candidates.extend(venv_python(&d).or_else(|| Some(d.join(if cfg!(windows) { "python.exe" } else { "bin/python" }))));
        }
    }
    if cfg!(windows) {
        if let Some(list) = output_within(Command::new("py").arg("-0p"), Duration::from_secs(3)) {
            for line in list.lines() {
                // " -V:3.12 *        C:\Program Files\Python312\python.exe"
                if let Some(i) = line.find(":\\") {
                    candidates.push(PathBuf::from(line[i.saturating_sub(1)..].trim()));
                }
            }
        }
    }
    let names: &[&str] = if cfg!(windows) { &["python.exe", "python3.exe"] } else { &["python3", "python"] };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for n in names {
                let p = dir.join(n);
                if p.is_file() {
                    candidates.push(p);
                }
            }
        }
    }
    for c in candidates {
        if !c.exists() || out.iter().any(|i| same_file(&i.path, &c)) {
            continue;
        }
        if let Some(i) = probe(&c) {
            if !out.iter().any(|o| same_file(&o.path, &i.path)) {
                out.push(i);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poetry_env_names() {
        assert_eq!(poetry_env_project("smokepoetry-nLnZ7pEb-py3.14"), Some("smokepoetry"));
        assert_eq!(poetry_env_project("my-app-a_-B9zQ1-py3.12"), Some("my-app"));
        assert_eq!(poetry_env_project(".venv"), None);
        assert_eq!(poetry_env_project("tools-py3"), None);
    }
}
