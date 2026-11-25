//! Where Python puts things. The layout is a value, not a `cfg`: an SSH or WSL interpreter
//! has the Unix layout on a Windows host, so both are plain path logic, tested on every OS.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// `Scripts\python.exe` in a venv, `python.exe` in the prefix of a base install or conda.
    Windows,
    /// `bin/python` everywhere.
    Unix,
}

impl Layout {
    /// The layout of interpreters on this machine.
    pub fn host() -> Self {
        if cfg!(windows) {
            Layout::Windows
        } else {
            Layout::Unix
        }
    }

    /// The file name of a program: `ruff.exe` or `ruff`.
    pub fn exe(self, name: &str) -> String {
        match self {
            Layout::Windows => format!("{name}.exe"),
            Layout::Unix => name.to_string(),
        }
    }

    /// The folder of a venv's interpreter and scripts: `<env>\Scripts` or `<env>/bin`.
    pub fn scripts_dir(self, env: &Path) -> PathBuf {
        env.join(match self {
            Layout::Windows => "Scripts",
            Layout::Unix => "bin",
        })
    }

    /// Where a venv's interpreter can be, most likely first.
    pub fn venv_pythons(self, env: &Path) -> Vec<PathBuf> {
        let dir = self.scripts_dir(env);
        match self {
            Layout::Windows => vec![dir.join("python.exe")],
            Layout::Unix => vec![dir.join("python"), dir.join("python3")],
        }
    }

    /// The interpreter of a base install or conda environment at `prefix`.
    pub fn prefix_python(self, prefix: &Path) -> PathBuf {
        match self {
            Layout::Windows => prefix.join("python.exe"),
            Layout::Unix => prefix.join("bin").join("python"),
        }
    }

    /// The prefix of a base install or conda environment, from its interpreter.
    pub fn prefix_of(self, python: &Path) -> Option<&Path> {
        match self {
            Layout::Windows => python.parent(),
            Layout::Unix => python.parent()?.parent(),
        }
    }

    /// The folders a conda activation puts first on `PATH`.
    pub fn conda_path_dirs(self, prefix: &Path) -> Vec<PathBuf> {
        match self {
            Layout::Windows => vec![
                prefix.to_path_buf(),
                prefix.join("Library").join("mingw-w64").join("bin"),
                prefix.join("Library").join("usr").join("bin"),
                prefix.join("Library").join("bin"),
                prefix.join("Scripts"),
            ],
            Layout::Unix => vec![prefix.join("bin")],
        }
    }

    /// The folders a tool installed for the interpreter in `python_dir` can be in: next to
    /// it, and on Windows also in `Scripts`, where a base install keeps them.
    pub fn tool_dirs(self, python_dir: &Path) -> Vec<PathBuf> {
        match self {
            Layout::Windows => vec![python_dir.to_path_buf(), python_dir.join("Scripts")],
            Layout::Unix => vec![python_dir.to_path_buf()],
        }
    }

    /// The interpreter file names to look for on `PATH`, preferred first.
    pub fn python_names(self) -> &'static [&'static str] {
        match self {
            Layout::Windows => &["python.exe", "python3.exe"],
            Layout::Unix => &["python3", "python"],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Layout::{Unix, Windows};
    use std::path::Path;

    #[test]
    fn windows_layout() {
        let env = Path::new("p").join(".venv");
        assert_eq!(Windows.exe("ruff"), "ruff.exe");
        assert_eq!(Windows.scripts_dir(&env), env.join("Scripts"));
        assert_eq!(Windows.venv_pythons(&env), vec![env.join("Scripts").join("python.exe")]);
        let prefix = Path::new("conda").join("envs").join("work");
        let python = Windows.prefix_python(&prefix);
        assert_eq!(python, prefix.join("python.exe"));
        assert_eq!(Windows.prefix_of(&python), Some(prefix.as_path()));
        assert_eq!(Windows.conda_path_dirs(&prefix).first(), Some(&prefix));
        assert_eq!(Windows.conda_path_dirs(&prefix).last(), Some(&prefix.join("Scripts")));
        assert_eq!(Windows.tool_dirs(&prefix), vec![prefix.clone(), prefix.join("Scripts")]);
    }

    #[test]
    fn unix_layout() {
        let env = Path::new("p").join(".venv");
        assert_eq!(Unix.exe("ruff"), "ruff");
        assert_eq!(Unix.scripts_dir(&env), env.join("bin"));
        assert_eq!(Unix.venv_pythons(&env), vec![env.join("bin").join("python"), env.join("bin").join("python3")]);
        let prefix = Path::new("conda").join("envs").join("work");
        let python = Unix.prefix_python(&prefix);
        assert_eq!(python, prefix.join("bin").join("python"));
        assert_eq!(Unix.prefix_of(&python), Some(prefix.as_path()));
        assert_eq!(Unix.conda_path_dirs(&prefix), vec![prefix.join("bin")]);
        assert_eq!(Unix.tool_dirs(&prefix), vec![prefix.clone()]);
        assert_eq!(Unix.python_names(), &["python3", "python"]);
    }
}
