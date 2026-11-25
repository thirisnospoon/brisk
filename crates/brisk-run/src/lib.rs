//! Python run support, UI-free: interpreter discovery, PyCharm-style run configurations
//! (`.brisk/run.json`, imported from `.idea`), command lines and traceback links.

mod config;
pub mod django;
pub mod envfile;
mod interp;
mod links;
pub mod project_files;
pub mod pyproj;
pub mod pytest;
pub mod ssh;
pub mod web;

pub use config::{
    compound_names, idea_test_runner, import_idea, load, save, BeforeStep, OnStart, RunConfig, RunFile, RunKind, TestRunner, CONFIG_FILE, MAX_TEMPORARY,
    TEMPLATE_KINDS,
};
pub use interp::{discover_all, probe, project_venvs, venv_python, venv_version, InterpKind, Interpreter};
pub use links::{find_links, find_urls, Link};
pub use pytest::{is_test_file, split_nodeid, TestFeed, TestStatus, TestTree, TreeView};

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

    /// "Open in Terminal": one line for the project terminal that runs this command — into
    /// the working directory, with the run's variables (PATH and the venv's are the
    /// terminal's own), then the program. PowerShell on Windows, a POSIX shell elsewhere.
    pub fn shell_line(&self, powershell: bool) -> String {
        let q = |s: &str| if powershell { format!("'{}'", s.replace('\'', "''")) } else { format!("'{}'", s.replace('\'', "'\\''")) };
        let mut env: Vec<(&String, &String)> = self.env.iter().filter(|(k, _)| !matches!(k.to_ascii_uppercase().as_str(), "PATH" | "VIRTUAL_ENV")).collect();
        env.sort();
        let program = q(&self.program.display().to_string());
        let args: Vec<String> = self.args.iter().map(|a| q(a)).collect();
        let cwd = q(&self.cwd.display().to_string());
        if powershell {
            let mut s = format!("Set-Location -LiteralPath {cwd}; ");
            for (k, v) in env {
                s.push_str(&format!("{}={}; ", ps_env_var(k), q(v)));
            }
            format!("{s}& {program} {}", args.join(" ")).trim_end().to_string()
        } else {
            let vars: Vec<String> = env.iter().map(|(k, v)| format!("{k}={}", q(v))).collect();
            format!("cd {cwd} && {} {program} {}", vars.join(" "), args.join(" ")).replace("  ", " ").trim_end().to_string()
        }
    }

    pub fn to_std(&self) -> std::process::Command {
        let mut c = std::process::Command::new(&self.program);
        c.current_dir(&self.cwd).envs(&self.env);
        #[cfg(windows)]
        if let Some(line) = self.cmd_line() {
            // cmd.exe does not parse MSVC escaping (an inner `"` would arrive as `\"`): hand it
            // the line as is. With /S, cmd strips exactly the outer pair of quotes.
            use std::os::windows::process::CommandExt;
            c.raw_arg(format!("/S /C \"{line}\""));
            return c;
        }
        c.args(&self.args);
        c
    }

    /// The line of a `cmd.exe /C <line>` command (see [`shell_command`]).
    pub fn cmd_line(&self) -> Option<&str> {
        let stem = self.program.file_stem()?.to_str()?;
        match self.args.as_slice() {
            [flag, line] if stem.eq_ignore_ascii_case("cmd") && flag.eq_ignore_ascii_case("/C") => Some(line),
            _ => None,
        }
    }
}

/// `${env:NAME}`: the braced form takes any name (`MY-VAR`, `ProgramFiles(x86)`); inside the
/// braces only `{`, `}` and the backtick need escaping.
fn ps_env_var(name: &str) -> String {
    let mut s = String::from("${env:");
    for c in name.chars() {
        if matches!(c, '{' | '}' | '`') {
            s.push('`');
        }
        s.push(c);
    }
    s.push('}');
    s
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
    let (program, args) = brisk_platform::shell::command_line(line);
    Command { program: program.into(), args, cwd: base.cwd, env: base.env }
}

pub fn build(root: &Path, cfg: &RunConfig, python: &Path) -> Command {
    let mut args = split_args(&cfg.interpreter_options);
    if matches!(cfg.kind, RunKind::FastApi | RunKind::Flask) {
        // Servers import `module:app` from the working directory (the project root by default).
        args.extend(web::args(cfg));
        let cwd = if cfg.working_directory.trim().is_empty() { root.to_path_buf() } else { resolve(root, &cfg.working_directory) };
        return finish(root, cfg, python, args, cwd);
    }
    if matches!(cfg.kind, RunKind::Django | RunKind::DjangoTests) {
        let (rest, cwd) = django::command(root, cfg);
        args.extend(rest);
        return finish(root, cfg, python, args, cwd);
    }
    if cfg.kind.is_tests() {
        return build_pytest(root, cfg, python, args);
    }
    if cfg.python_console {
        args.push("-i".into());
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
/// unittest and doctest take the same targets: `python -m unittest pkg.test_x.TestA.test_b`
/// (`discover` for a folder or nothing), `python -m doctest file.py`.
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
    if cfg.kind == RunKind::Unittest {
        args.extend(["-m".to_string(), "unittest".into()]);
        let dirs = targets.iter().any(|(p, _)| p.is_dir());
        if targets.is_empty() || dirs {
            // A folder: discovery from it (the working directory is the top level).
            args.push("discover".into());
            if let Some((p, _)) = targets.iter().find(|(p, _)| p.is_dir() && *p != cwd) {
                args.extend([
                    "-s".to_string(),
                    p.strip_prefix(&cwd).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| p.display().to_string()),
                    "-t".into(),
                    ".".into(),
                ]);
            }
        } else {
            for (p, rest) in &targets {
                args.push(unittest_name(&cwd, p, rest));
            }
        }
        if !cfg.keywords.trim().is_empty() {
            args.push("-k".into());
            args.push(cfg.keywords.trim().to_string());
        }
        args.extend(split_args(&cfg.parameters));
        return finish(root, cfg, python, args, cwd);
    }
    if cfg.kind == RunKind::Doctest {
        args.extend(["-m".to_string(), "doctest".into()]);
        args.extend(split_args(&cfg.parameters));
        for (p, rest) in &targets {
            let shown = p.strip_prefix(&cwd).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| p.display().to_string());
            args.push(format!("{shown}{rest}"));
        }
        return finish(root, cfg, python, args, cwd);
    }
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

/// `tests/test_x.py` + `::TestA::test_b` as unittest's dotted name relative to `cwd`
/// (`tests.test_x.TestA.test_b`); a file outside it stays a path.
fn unittest_name(cwd: &Path, p: &Path, rest: &str) -> String {
    let Ok(rel) = p.strip_prefix(cwd) else { return p.display().to_string() };
    let rel = rel.to_string_lossy().replace('\\', "/");
    let module = rel.strip_suffix(".py").unwrap_or(&rel).replace('/', ".");
    let tail: String = rest.split("::").filter(|s| !s.is_empty()).map(|s| format!(".{s}")).collect();
    format!("{module}{tail}")
}

fn finish(root: &Path, cfg: &RunConfig, python: &Path, args: Vec<String>, cwd: PathBuf) -> Command {
    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut env = HashMap::new();
    env.insert("PYTHONUNBUFFERED".into(), "1".into());
    env.insert("PYTHONIOENCODING".into(), "UTF-8".into());
    // The project root, then the folders marked as source and test roots (as PyCharm's
    // "Add source roots to PYTHONPATH").
    let mut pp: Vec<String> =
        std::iter::once(root.to_path_buf()).chain(brisk_project::Structure::load(root).import_roots(root)).map(|p| p.display().to_string()).collect();
    if let Ok(old) = std::env::var("PYTHONPATH") {
        if !old.is_empty() {
            pp.push(old);
        }
    }
    let pp = pp.join(sep);
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
