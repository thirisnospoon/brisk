//! Node.js runs, as PyCharm's npm and Node.js configurations: a `package.json` script through
//! the project's package manager (npm, pnpm, yarn or bun, chosen by the lock file), and a
//! JS / TS file with node (TypeScript through the project's `tsx` when it has one, else
//! node's own type stripping). No Node code ships with Brisk: the tools come from the project
//! (`node_modules/.bin`) or PATH.

use crate::config::{RunConfig, RunKind};
use crate::{envfile, resolve, split_args, Command};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Manager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

impl Manager {
    pub fn name(self) -> &'static str {
        match self {
            Manager::Npm => "npm",
            Manager::Pnpm => "pnpm",
            Manager::Yarn => "yarn",
            Manager::Bun => "bun",
        }
    }

    /// The manager of the package in `dir`: `packageManager` in its `package.json`, else the
    /// first lock file found from `dir` up (a workspace keeps it at the top), else npm.
    pub fn detect(dir: &Path) -> Manager {
        let declared = std::fs::read_to_string(dir.join("package.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v.get("packageManager").and_then(|p| p.as_str()).map(str::to_string));
        if let Some(m) = declared.as_deref().and_then(|p| Manager::from_name(p.split('@').next().unwrap_or(""))) {
            return m;
        }
        for d in dir.ancestors() {
            for (lock, m) in [
                ("pnpm-lock.yaml", Manager::Pnpm),
                ("yarn.lock", Manager::Yarn),
                ("bun.lock", Manager::Bun),
                ("bun.lockb", Manager::Bun),
                ("package-lock.json", Manager::Npm),
            ] {
                if d.join(lock).is_file() {
                    return m;
                }
            }
        }
        Manager::Npm
    }

    fn from_name(s: &str) -> Option<Manager> {
        Some(match s {
            "npm" => Manager::Npm,
            "pnpm" => Manager::Pnpm,
            "yarn" => Manager::Yarn,
            "bun" => Manager::Bun,
            _ => return None,
        })
    }
}

/// One entry of `"scripts"`, with the 0-based line of its key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Script {
    pub name: String,
    pub command: String,
    pub line: usize,
}

/// The scripts of a `package.json` text in file order. A broken file gives what can be read.
pub fn scripts(text: &str) -> Vec<Script> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let (mut i, mut depth, mut line) = (0, 0usize, 0usize);
    // Inside the top-level "scripts" object: its depth.
    let mut in_scripts: Option<usize> = None;
    let mut pending_key: Option<(String, usize)> = None;
    let mut last_string: Option<(String, usize)> = None;
    while i < b.len() {
        match b[i] {
            b'\n' => line += 1,
            b'"' => {
                let (s, end, lines) = read_string(text, i);
                last_string = Some((s, line));
                line += lines;
                i = end;
                // A value string of a script entry.
                if let (Some(d), Some((k, l))) = (in_scripts, pending_key.take()) {
                    if depth == d {
                        out.push(Script { name: k, command: last_string.take().map(|s| s.0).unwrap_or_default(), line: l });
                    }
                }
                continue;
            }
            b':' => {
                if let Some((k, l)) = last_string.take() {
                    if depth == 1 && k == "scripts" {
                        in_scripts = Some(2);
                    } else if in_scripts == Some(depth) {
                        pending_key = Some((k, l));
                    }
                }
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                if in_scripts == Some(depth) {
                    in_scripts = None;
                }
                depth = depth.saturating_sub(1);
            }
            b',' => last_string = None,
            _ => {}
        }
        i += 1;
    }
    out
}

/// The JSON string starting at the quote at `start`: its text (escapes read), the index
/// after its closing quote, and how many line breaks it spans (only a broken file has any).
fn read_string(text: &str, start: usize) -> (String, usize, usize) {
    let mut s = String::new();
    let mut lines = 0;
    let mut it = text[start + 1..].char_indices();
    while let Some((j, c)) = it.next() {
        match c {
            '"' => return (s, start + 1 + j + 1, lines),
            '\\' => match it.next() {
                Some((_, 'n')) => s.push('\n'),
                Some((_, 't')) => s.push('\t'),
                Some((_, 'u')) => {
                    let hex: String = (0..4).filter_map(|_| it.next().map(|(_, c)| c)).collect();
                    if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        s.push(c);
                    }
                }
                Some((_, c)) => s.push(c),
                None => break,
            },
            '\n' => {
                lines += 1;
                s.push(c);
            }
            c => s.push(c),
        }
    }
    (s, text.len(), lines)
}

/// A JS / TS file node can run.
pub fn is_script(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("js" | "mjs" | "cjs" | "ts" | "mts" | "cts"))
}

fn is_typescript(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("ts" | "mts" | "cts" | "tsx"))
}

/// `name` on PATH (`name.cmd` / `.exe` on Windows, as npm installs them).
pub fn on_path(name: &str) -> Option<PathBuf> {
    let exts: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ".bat"] } else { &[""] };
    std::env::split_paths(&std::env::var_os("PATH")?).flat_map(|d| exts.iter().map(move |e| d.join(format!("{name}{e}")))).find(|p| p.is_file())
}

/// `node_modules/.bin/<name>` in `dir` or a folder above it, up to `root`.
fn local_bin(dir: &Path, root: &Path, name: &str) -> Option<PathBuf> {
    let exts: &[&str] = if cfg!(windows) { &[".cmd", ".exe"] } else { &[""] };
    dir.ancestors()
        .take_while(|d| d.starts_with(root) || *d == dir)
        .flat_map(|d| exts.iter().map(move |e| d.join("node_modules").join(".bin").join(format!("{name}{e}"))))
        .find(|p| p.is_file())
}

/// The nearest `node_modules/.bin` folders from `dir` up to `root`, nearest first.
fn bin_dirs(dir: &Path, root: &Path) -> Vec<PathBuf> {
    dir.ancestors().take_while(|d| d.starts_with(root) || *d == dir).map(|d| d.join("node_modules").join(".bin")).filter(|p| p.is_dir()).collect()
}

impl RunConfig {
    /// The temporary configuration of a script's ▶ in `package.json`: named after the script.
    pub fn for_npm(root: &Path, package_json: &Path, script: &str) -> Self {
        RunConfig {
            name: script.to_string(),
            kind: RunKind::Npm,
            target: crate::relativize(root, package_json),
            script: script.to_string(),
            temporary: true,
            ..Default::default()
        }
    }

    /// The temporary Node.js configuration of a JS / TS file.
    pub fn for_node(root: &Path, file: &Path) -> Self {
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "script".into());
        RunConfig { name, kind: RunKind::Node, target: crate::relativize(root, file), temporary: true, ..Default::default() }
    }
}

/// The process of an npm or Node.js configuration, or why it cannot start (a tool missing).
pub fn build(root: &Path, cfg: &RunConfig) -> Result<Command, String> {
    let custom = cfg.interpreter.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|p| resolve(root, p));
    let (program, args, cwd) = match cfg.kind {
        RunKind::Npm => {
            let pj = resolve(root, if cfg.target.trim().is_empty() { "package.json" } else { cfg.target.trim() });
            let dir = pj.parent().map(Path::to_path_buf).unwrap_or_else(|| root.to_path_buf());
            if cfg.script.trim().is_empty() {
                return Err(format!("'{}': no script chosen", cfg.name));
            }
            let m = Manager::detect(&dir);
            let program = match custom {
                Some(p) => p,
                None => on_path(m.name()).ok_or_else(|| missing(m.name()))?,
            };
            let mut args = vec!["run".to_string(), cfg.script.trim().to_string()];
            let params = split_args(&cfg.parameters);
            // npm passes arguments to the script only after `--`; the others take them as is.
            if m == Manager::Npm && !params.is_empty() && params[0] != "--" {
                args.push("--".into());
            }
            args.extend(params);
            (program, args, dir)
        }
        RunKind::Node => {
            let file = resolve(root, &cfg.target);
            let dir = file.parent().map(Path::to_path_buf).unwrap_or_else(|| root.to_path_buf());
            let tsx = if is_typescript(&file) && custom.is_none() { local_bin(&dir, root, "tsx") } else { None };
            let program = match (custom, tsx) {
                (Some(p), _) => p,
                (None, Some(t)) => t,
                (None, None) => on_path("node").ok_or_else(|| missing("node"))?,
            };
            let mut args = split_args(&cfg.interpreter_options);
            args.push(file.display().to_string());
            args.extend(split_args(&cfg.parameters));
            (program, args, dir)
        }
        _ => return Err(format!("'{}' is not a Node.js configuration", cfg.name)),
    };
    let cwd = if cfg.working_directory.trim().is_empty() { cwd } else { resolve(root, &cfg.working_directory) };
    // npm already puts the package's .bin on PATH; a node run gets it too.
    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut env = HashMap::new();
    let mut path: Vec<String> = bin_dirs(&cwd, root).iter().map(|p| p.display().to_string()).collect();
    path.push(std::env::var("PATH").unwrap_or_default());
    env.insert("PATH".into(), path.join(sep));
    // The Run console shows ANSI colors.
    env.insert("FORCE_COLOR".into(), "1".into());
    for (k, v) in envfile::load(root, cfg).unwrap_or_default() {
        env.insert(k, v);
    }
    for (k, v) in &cfg.env {
        env.insert(k.clone(), v.clone());
    }
    Ok(Command { program, args, cwd, env })
}

fn missing(tool: &str) -> String {
    match tool {
        "node" | "npm" => format!("{tool} is not on PATH: install Node.js from nodejs.org, or set the path in the configuration"),
        _ => format!("{tool} is not on PATH: install it (npm i -g {tool}) or set its path in the configuration"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_with_lines() {
        let t = r#"{
  "name": "app",
  "scripts": {
    "dev": "vite",
    "build": "tsc -b && vite build",
    "q": "echo \"hi\""
  },
  "dependencies": { "dev": "1" }
}"#;
        let s = scripts(t);
        let got: Vec<(&str, &str, usize)> = s.iter().map(|s| (s.name.as_str(), s.command.as_str(), s.line)).collect();
        assert_eq!(got, [("dev", "vite", 3), ("build", "tsc -b && vite build", 4), ("q", "echo \"hi\"", 5)]);
    }

    #[test]
    fn nested_scripts_key_is_not_top_level() {
        assert!(scripts(r#"{"config": {"scripts": {"a": "b"}}}"#).is_empty());
        assert_eq!(scripts(r#"{"scripts":{"a":"b"}"#).len(), 1);
    }

    #[test]
    fn manager_from_lock_and_field() {
        let d = std::env::temp_dir().join(format!("brisk-node-{}", std::process::id()));
        let sub = d.join("pkg");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(Manager::detect(&sub), Manager::Npm);
        std::fs::write(d.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(Manager::detect(&sub), Manager::Pnpm);
        std::fs::write(sub.join("package.json"), r#"{"packageManager": "yarn@4.1.0"}"#).unwrap();
        assert_eq!(Manager::detect(&sub), Manager::Yarn);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn npm_args_go_after_double_dash() {
        let d = std::env::temp_dir().join(format!("brisk-node-npm-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("package.json"), "{}").unwrap();
        let mut c = RunConfig::for_npm(&d, &d.join("package.json"), "dev");
        c.parameters = "--port 3000".into();
        c.interpreter = Some("npm-test".into());
        let cmd = build(&d, &c).unwrap();
        assert_eq!(cmd.args, ["run", "dev", "--", "--port", "3000"]);
        assert_eq!(cmd.cwd, d);
        let _ = std::fs::remove_dir_all(&d);
    }
}
