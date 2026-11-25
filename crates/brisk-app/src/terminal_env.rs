//! The Terminal with the project interpreter's environment activated, like PyCharm's: a venv
//! (wherever it lives) or a conda environment goes first on PATH with VIRTUAL_ENV /
//! CONDA_PREFIX set, and PowerShell's prompt starts with `(name)` as after `activate`.

use brisk_platform::python::Layout;
use brisk_platform::shell::path_separator as sep;
use brisk_run::{InterpKind, Interpreter};
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Activation {
    pub env: HashMap<String, String>,
    /// The `(name)` the prompt shows; `None` for a plain interpreter.
    pub prompt: Option<String>,
}

/// `prompt = …` of pyvenv.cfg (`venv --prompt`), else the folder name, like activate scripts.
fn venv_prompt(venv: &Path) -> String {
    std::fs::read_to_string(venv.join("pyvenv.cfg"))
        .ok()
        .and_then(|t| {
            t.lines().find_map(|l| l.split_once('=').filter(|(k, _)| k.trim() == "prompt").map(|(_, v)| v.trim().trim_matches(['\'', '"']).to_string()))
        })
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| venv.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
}

/// The environment for shells of a project whose interpreter is `python` (of `kind`).
pub fn activation(python: &Path, kind: InterpKind) -> Activation {
    let old = std::env::var("PATH").unwrap_or_default();
    let mut env = HashMap::new();
    let prompt = if let Some(venv) = Interpreter::venv_dir_of(python) {
        let bin = python.parent().unwrap_or(&venv);
        let name = venv_prompt(&venv);
        env.insert("PATH".into(), format!("{}{}{old}", bin.display(), sep()));
        env.insert("VIRTUAL_ENV".into(), venv.display().to_string());
        env.insert("VIRTUAL_ENV_PROMPT".into(), name.clone());
        Some(name)
    } else if kind == InterpKind::Conda {
        let Some(prefix) = Layout::host().prefix_of(python) else { return Activation::default() };
        let dirs = Layout::host().conda_path_dirs(prefix);
        let name = prefix.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let dirs: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
        env.insert("PATH".into(), format!("{}{}{old}", dirs.join(sep()), sep()));
        env.insert("CONDA_PREFIX".into(), prefix.display().to_string());
        env.insert("CONDA_DEFAULT_ENV".into(), name.clone());
        Some(name)
    } else {
        None
    };
    Activation { env, prompt }
}

/// PowerShell arguments that keep the profile's prompt and prefix it with `(name) `.
pub fn powershell_args(prompt: &str) -> Vec<String> {
    let name: String = prompt.chars().filter(|c| c.is_alphanumeric() || "._- ".contains(*c)).collect();
    vec![
        "-NoLogo".into(),
        "-NoExit".into(),
        "-Command".into(),
        format!("$global:__brisk_prompt = $function:prompt; function global:prompt {{ '({name}) ' + (& $global:__brisk_prompt) }}"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn venv_and_conda() {
        let dir = std::env::temp_dir().join(format!("brisk-tenv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let venv = dir.join("envs").join("work");
        let bin = Layout::host().scripts_dir(&venv);
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), "home = x\nprompt = 'proj'\n").unwrap();
        let a = activation(&bin.join("python.exe"), InterpKind::Venv);
        assert_eq!(a.prompt.as_deref(), Some("proj"));
        assert_eq!(a.env["VIRTUAL_ENV"], venv.display().to_string());
        assert!(a.env["PATH"].starts_with(&bin.display().to_string()));
        std::fs::write(venv.join("pyvenv.cfg"), "home = x\n").unwrap();
        assert_eq!(activation(&bin.join("python.exe"), InterpKind::Venv).prompt.as_deref(), Some("work"));

        let conda = dir.join("miniconda3").join("envs").join("ds");
        let py = Layout::host().prefix_python(&conda);
        let a = activation(&py, InterpKind::Conda);
        assert_eq!(a.prompt.as_deref(), Some("ds"));
        assert_eq!(a.env["CONDA_PREFIX"], conda.display().to_string());
        assert!(!a.env.contains_key("VIRTUAL_ENV"));

        assert_eq!(activation(Path::new("/usr/bin/python3"), InterpKind::System), Activation::default());
        assert!(powershell_args("a'b; rm").last().unwrap().contains("'(ab rm) '"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
