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
