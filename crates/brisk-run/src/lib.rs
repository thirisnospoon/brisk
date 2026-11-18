//! Python run support, UI-free: interpreter discovery, PyCharm-style run configurations
//! (`.brisk/run.json`, imported from `.idea`), command lines and traceback links.

mod config;
mod interp;
mod links;
pub mod pytest;
pub mod envfile;
pub mod web;

pub use config::{compound_names, import_idea, load, save, BeforeStep, RunConfig, RunFile, RunKind, CONFIG_FILE, MAX_TEMPORARY};
pub use pytest::{is_test_file, split_nodeid, TestFeed, TestStatus, TestTree};
pub use interp::{discover_all, probe, project_venvs, venv_python, InterpKind, Interpreter};
pub use links::{find_links, find_urls, Link};

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A ready-to-spawn process.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
}

impl Command {
    /// The command line PyCharm prints as the first console line.
    pub fn display(&self) -> String {
        let mut s = self.program.display().to_string();
        for a in &self.args {
            s.push(' ');
            if a.is_empty() || a.contains([' ', '\t', '"']) {
                s.push('"');
                s.push_str(&a.replace('"', "\\\""));
                s.push('"');
            } else {
                s.push_str(a);
            }
        }
        s
    }

    pub fn to_std(&self) -> std::process::Command {
        let mut c = std::process::Command::new(&self.program);
        c.args(&self.args).current_dir(&self.cwd).envs(&self.env);
        c
    }
}

/// Expand `$PROJECT_DIR$` and resolve relative paths against the project root.
pub fn resolve(root: &Path, p: &str) -> PathBuf {
    let p = p.trim();
    let expanded = p.replace("$PROJECT_DIR$", &root.display().to_string());
    let pb = PathBuf::from(&expanded);
    if expanded.is_empty() {
        root.to_path_buf()
    } else if pb.is_absolute() {
        pb
    } else {
        root.join(pb)
    }
}

/// Path stored in configs: relative to the project with `/`, absolute outside it.
pub fn relativize(root: &Path, p: &Path) -> String {
    match p.strip_prefix(root) {
        Ok(r) => r.to_string_lossy().replace('\\', "/"),
        Err(_) => p.display().to_string(),
    }
}

/// Split a parameters string the way PyCharm does: whitespace separates, double quotes group,
/// `\"` is a literal quote.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut any = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
                any = true;
            }
            '"' => {
                in_q = !in_q;
                any = true;
            }
            c if c.is_whitespace() && !in_q => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

/// Build the process for a configuration, like PyCharm's Python run configuration:
/// unbuffered UTF-8 output, project root on `PYTHONPATH`, the virtualenv first on `PATH`.
/// A Before launch command line through the shell (`cmd /C` on Windows, `sh -c` elsewhere),
/// in the configuration's working directory and environment (so `pip` is the venv's).
pub fn shell_command(root: &Path, cfg: &RunConfig, python: &Path, line: &str) -> Command {
    let base = build(root, &RunConfig { kind: RunKind::Python, ..cfg.clone() }, python);
    let (program, flag) = if cfg!(windows) { ("cmd.exe", "/C") } else { ("sh", "-c") };
    Command { program: program.into(), args: vec![flag.into(), line.to_string()], cwd: base.cwd, env: base.env }
}

pub fn build(root: &Path, cfg: &RunConfig, python: &Path) -> Command {
    let mut args = split_args(&cfg.interpreter_options);
    if matches!(cfg.kind, RunKind::FastApi | RunKind::Flask) {
        // Servers import `module:app` from the working directory (the project root by default).
        args.extend(web::args(cfg));
        let cwd = if cfg.working_directory.trim().is_empty() { root.to_path_buf() } else { resolve(root, &cfg.working_directory) };
        return finish(root, cfg, python, args, cwd);
    }
    if cfg.kind == RunKind::Pytest {
        return build_pytest(root, cfg, python, args);
    }
    if cfg.module {
        args.push("-m".into());
        args.push(cfg.target.trim().to_string());
    } else {
        args.push(resolve(root, &cfg.target).display().to_string());
    }
    args.extend(split_args(&cfg.parameters));
    let cwd = if cfg.working_directory.trim().is_empty() {
        if cfg.module {
            root.to_path_buf()
        } else {
            resolve(root, &cfg.target).parent().map(Path::to_path_buf).unwrap_or_else(|| root.to_path_buf())
        }
    } else {
        resolve(root, &cfg.working_directory)
    };
    finish(root, cfg, python, args, cwd)
}

/// `python -m pytest <targets> [-k kw] <params>`; the reporting plugin is added per run by
/// [`pytest::attach`]. Working directory: the target folder, or the test file's folder.
fn build_pytest(root: &Path, cfg: &RunConfig, python: &Path, mut args: Vec<String>) -> Command {
    let targets: Vec<(PathBuf, String)> = split_args(&cfg.target)
        .into_iter()
        .map(|t| {
            let parts = pytest::split_nodeid(&t);
            let rest = t[parts[0].len()..].to_string();
            (resolve(root, parts[0]), rest)
        })
        .collect();
    let cwd = if !cfg.working_directory.trim().is_empty() {
        resolve(root, &cfg.working_directory)
    } else {
        match targets.first() {
            Some((p, _)) if p.is_dir() => p.clone(),
            Some((p, _)) => p.parent().map(Path::to_path_buf).unwrap_or_else(|| root.to_path_buf()),
            None => root.to_path_buf(),
        }
    };
    args.push("-m".into());
    args.push("pytest".into());
    for (p, rest) in targets {
        let shown = match p.strip_prefix(&cwd) {
            Ok(r) if r.as_os_str().is_empty() => ".".to_string(),
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => p.display().to_string(),
        };
        args.push(format!("{shown}{rest}"));
    }
    if !cfg.keywords.trim().is_empty() {
        args.push("-k".into());
        args.push(cfg.keywords.trim().to_string());
    }
    args.extend(split_args(&cfg.parameters));
    finish(root, cfg, python, args, cwd)
}

fn finish(root: &Path, cfg: &RunConfig, python: &Path, args: Vec<String>, cwd: PathBuf) -> Command {
    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut env = HashMap::new();
    env.insert("PYTHONUNBUFFERED".into(), "1".into());
    env.insert("PYTHONIOENCODING".into(), "UTF-8".into());
    let pp = match std::env::var("PYTHONPATH") {
        Ok(old) if !old.is_empty() => format!("{}{sep}{old}", root.display()),
        _ => root.display().to_string(),
    };
    env.insert("PYTHONPATH".into(), pp);
    if let Some(venv) = Interpreter::venv_dir_of(python) {
        let bin = python.parent().unwrap_or(&venv);
        env.insert("PATH".into(), format!("{}{sep}{}", bin.display(), std::env::var("PATH").unwrap_or_default()));
        env.insert("VIRTUAL_ENV".into(), venv.display().to_string());
    }
    // .env files first, the configuration's own variables over them (a missing file is
    // reported before the run by `envfile::load`).
    for (k, v) in envfile::load(root, cfg).unwrap_or_default() {
        env.insert(k, v);
    }
    for (k, v) in &cfg.env {
        env.insert(k.clone(), v.clone());
    }
    Command { program: python.to_path_buf(), args, cwd, env }
}

#[cfg(test)]
mod tests;
