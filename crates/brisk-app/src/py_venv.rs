//! A project virtual environment: `python -m venv` with a chosen base interpreter, and the
//! project's requirements (requirements.txt, or `[project].dependencies` of pyproject.toml)
//! installed into it. Every call blocks: run them in the background.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Requirements {
    File(PathBuf),
    /// `[project].dependencies` of pyproject.toml.
    Pyproject(Vec<String>),
}

impl Requirements {
    pub fn label(&self) -> String {
        match self {
            Requirements::File(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            Requirements::Pyproject(d) => format!("pyproject.toml ({} dependencies)", d.len()),
        }
    }
}

fn command(program: &Path) -> Command {
    let mut c = Command::new(program);
    c.env("PYTHONIOENCODING", "utf-8").env("PIP_DISABLE_PIP_VERSION_CHECK", "1").env("PIP_NO_INPUT", "1");
    crate::py_packages::offline_in_smoke(&mut c);
    brisk_platform::process::no_window(&mut c);
    c
}

fn last_error(stderr: &[u8]) -> String {
    let err = String::from_utf8_lossy(stderr);
    err.lines()
        .rev()
        .find(|l| l.trim_start().starts_with("ERROR") || l.contains("Error"))
        .or_else(|| err.lines().rev().find(|l| !l.trim().is_empty()))
        .unwrap_or("failed")
        .trim()
        .to_string()
}

/// The folder for a new environment: `.venv`, else `venv` when `.venv` is taken.
pub fn free_dir(root: &Path) -> Option<PathBuf> {
    [".venv", "venv"].iter().map(|n| root.join(n)).find(|p| !p.exists())
}

/// `base -m venv dir`; the new interpreter's path.
pub fn create(base: &Path, dir: &Path) -> Result<PathBuf, String> {
    let out = command(base).arg("-m").arg("venv").arg(dir).output().map_err(|e| format!("Cannot run {}: {e}", base.display()))?;
    if !out.status.success() {
        return Err(last_error(&out.stderr));
    }
    brisk_run::venv_python(dir).ok_or_else(|| format!("{} has no Python after venv", dir.display()))
}

/// `[project].dependencies = [ "...", ... ]` of a pyproject.toml (enough for the common
/// one-string-per-entry layout; comments and both quote styles are handled).
pub fn pyproject_dependencies(text: &str) -> Vec<String> {
    let mut in_project = false;
    let mut collecting = false;
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.split(" #").next().unwrap_or(raw).trim();
        if line.starts_with('#') {
            continue;
        }
        if !collecting && line.starts_with('[') && !line.starts_with("[[") {
            in_project = line == "[project]";
            continue;
        }
        let rest = if collecting {
            line
        } else if in_project && line.starts_with("dependencies") {
            match line.split_once('=') {
                Some((k, v)) if k.trim() == "dependencies" => {
                    collecting = true;
                    v.trim().trim_start_matches('[')
                }
                _ => continue,
            }
        } else {
            continue;
        };
        let mut s = rest;
        while let Some(q) = s.find(['"', '\'']) {
            let quote = &s[q..q + 1];
            let Some(end) = s[q + 1..].find(quote) else { break };
            let dep = s[q + 1..q + 1 + end].trim();
            if !dep.is_empty() {
                out.push(dep.to_string());
            }
            s = &s[q + 2 + end..];
        }
        if rest.contains(']') && collecting {
            // The closing bracket after the last string.
            let after_strings = rest.rfind(['"', '\'']).map_or(rest, |i| &rest[i..]);
            if after_strings.contains(']') {
                break;
            }
        }
    }
    out
}

/// What the project asks to install, if anything.
pub fn requirements(root: &Path) -> Option<Requirements> {
    let req = root.join("requirements.txt");
    if req.is_file() && std::fs::read_to_string(&req).is_ok_and(|t| t.lines().any(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))) {
        return Some(Requirements::File(req));
    }
    let deps = pyproject_dependencies(&std::fs::read_to_string(root.join("pyproject.toml")).ok()?);
    (!deps.is_empty()).then_some(Requirements::Pyproject(deps))
}

pub fn install(python: &Path, root: &Path, req: &Requirements) -> Result<(), String> {
    let mut c = command(python);
    c.current_dir(root).args(["-m", "pip", "install"]);
    match req {
        Requirements::File(p) => {
            c.arg("-r").arg(p);
        }
        Requirements::Pyproject(deps) => {
            c.args(deps);
        }
    }
    let out = c.output().map_err(|e| format!("Cannot run pip: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(last_error(&out.stderr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pyproject_deps() {
        let t = "[build-system]\nrequires = [\"hatchling\"]\n\n[project]\nname = \"x\"\ndependencies = [\n  \"requests>=2\",  # http\n  'rich',\n]\n\n[project.optional-dependencies]\ndev = [\"pytest\"]\n";
        assert_eq!(pyproject_dependencies(t), vec!["requests>=2", "rich"]);
        assert_eq!(pyproject_dependencies("[project]\ndependencies = [\"a\", \"b==1\"]\nversion = \"1\"\n"), vec!["a", "b==1"]);
        assert!(pyproject_dependencies("[project]\nname = \"x\"\n[tool.x]\ndependencies = [\"no\"]\n").is_empty());
    }

    #[test]
    fn requirements_and_dir() {
        let dir = std::env::temp_dir().join(format!("brisk-venv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(requirements(&dir), None);
        assert_eq!(free_dir(&dir), Some(dir.join(".venv")));
        std::fs::write(dir.join("requirements.txt"), "# none yet\n\n").unwrap();
        assert_eq!(requirements(&dir), None);
        std::fs::write(dir.join("pyproject.toml"), "[project]\ndependencies = [\"rich\"]\n").unwrap();
        assert_eq!(requirements(&dir), Some(Requirements::Pyproject(vec!["rich".into()])));
        std::fs::write(dir.join("requirements.txt"), "rich\n").unwrap();
        assert_eq!(requirements(&dir), Some(Requirements::File(dir.join("requirements.txt"))));
        std::fs::create_dir_all(dir.join(".venv")).unwrap();
        assert_eq!(free_dir(&dir), Some(dir.join("venv")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
