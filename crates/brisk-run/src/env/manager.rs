//! Running uv and Poetry for the project: syncing its environment and changing its
//! dependencies. Every call blocks: run them in the background.

use crate::pyproj::Manager;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The tool for the project, or a message saying how to get it.
pub fn tool(manager: Manager, project_python: Option<&Path>) -> Result<PathBuf, String> {
    crate::pyproj::find_tool(manager, project_python).ok_or_else(|| match manager {
        Manager::Uv => "uv is not installed: see docs.astral.sh/uv, or pip install uv".to_string(),
        Manager::Poetry => "Poetry is not installed: see python-poetry.org, or pipx install poetry".to_string(),
    })
}

fn command(tool: &Path, root: &Path) -> Command {
    let mut c = Command::new(tool);
    c.current_dir(root)
        .env("PYTHONIOENCODING", "utf-8")
        .env("NO_COLOR", "1")
        // An environment activated in the shell Brisk was started from is not the project's.
        .env_remove("VIRTUAL_ENV")
        .env_remove("CONDA_PREFIX");
    brisk_platform::process::no_window(&mut c);
    c
}

/// The line of the output that says what went wrong.
fn error_line(stderr: &str, stdout: &str) -> String {
    let pick = |t: &str| {
        let lines: Vec<&str> = t.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        lines.iter().rev().find(|l| l.to_lowercase().starts_with("error") || l.contains("error:")).or(lines.last()).map(|l| l.to_string())
    };
    pick(stderr).or_else(|| pick(stdout)).unwrap_or_else(|| "failed".into())
}

/// `tool args…` in the project folder; the error line on failure.
pub fn run(manager: Manager, tool: &Path, root: &Path, args: &[String]) -> Result<(), String> {
    let mut c = command(tool, root);
    c.args(args);
    if manager == Manager::Poetry {
        // uv never asks; Poetry would wait for an answer nobody can give.
        c.arg("--no-interaction");
    }
    let out = c.output().map_err(|e| format!("Cannot run {}: {e}", tool.display()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(error_line(&String::from_utf8_lossy(&out.stderr), &String::from_utf8_lossy(&out.stdout)))
    }
}

/// Syncs the environment from the lock file; its interpreter.
pub fn sync(manager: Manager, tool: &Path, root: &Path) -> Result<PathBuf, String> {
    // `poetry install` refuses a lock file older than pyproject.toml; `uv sync` locks itself.
    if manager == Manager::Poetry && crate::pyproj::lock_outdated(root, manager) {
        run(manager, tool, root, &manager.lock_args())?;
    }
    run(manager, tool, root, &manager.sync_args())?;
    match manager {
        Manager::Uv => crate::venv_python(&root.join(".venv")),
        Manager::Poetry => crate::pyproj::poetry_env_python(tool, root),
    }
    .ok_or_else(|| format!("{} finished, but its environment was not found", manager.name()))
}

/// uv or Poetry managing the dependencies of the project whose environment the Packages
/// window shows: installs and removals then go through `pyproject.toml` and the lock file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectTool {
    pub manager: Manager,
    pub tool: PathBuf,
    pub root: PathBuf,
}

fn same_path(a: &Path, b: &Path) -> bool {
    let n = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    n(a) == n(b)
}

/// The project's manager, when `python` is the project's own environment (`.venv` for uv,
/// Poetry's environment for Poetry) and the tool is installed. Can start Poetry: call it off
/// the UI thread.
pub fn project_tool(root: &Path, python: &Path) -> Option<ProjectTool> {
    let manager = crate::pyproj::detect(root)?;
    let tool = crate::pyproj::find_tool(manager, Some(python))?;
    let in_dot_venv = crate::Interpreter::venv_dir_of(python).is_some_and(|d| same_path(&d, &root.join(".venv")));
    let ours = in_dot_venv || (manager == Manager::Poetry && crate::pyproj::poetry_env_python(&tool, root).is_some_and(|p| same_path(&p, python)));
    ours.then(|| ProjectTool { manager, tool, root: root.to_path_buf() })
}

impl ProjectTool {
    pub fn label(&self) -> String {
        format!("{} · pyproject.toml", self.manager.name())
    }

    fn is_dev(&self, name: &str) -> bool {
        let text = std::fs::read_to_string(self.root.join("pyproject.toml")).unwrap_or_default();
        crate::pyproj::dev_dependencies(&text).contains(&crate::pyproj::normalize(name))
    }

    pub fn install(&self, requirement: &str) -> Result<(), String> {
        run(self.manager, &self.tool, &self.root, &self.manager.add_args(requirement, false))
    }

    pub fn uninstall(&self, name: &str) -> Result<(), String> {
        run(self.manager, &self.tool, &self.root, &self.manager.remove_args(name, self.is_dev(name)))
    }

    pub fn upgrade(&self, name: &str) -> Result<(), String> {
        run(self.manager, &self.tool, &self.root, &self.manager.upgrade_args(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_error() {
        assert_eq!(error_line("Resolving...\nerror: No solution found\n  hint: x\n", ""), "error: No solution found");
        assert_eq!(error_line("", "Updating\n\nBecause x depends on y, version solving failed.\n"), "Because x depends on y, version solving failed.");
        assert_eq!(error_line("", ""), "failed");
    }
}
