//! Run Anything (double Ctrl): a typed command becomes a run in the Run window, like
//! PyCharm's. `python x.py …`, `python -m mod …`, `pytest …` and `pip …` run with the project
//! interpreter as Python configurations (pytest gets the test tree); the name of a run
//! configuration runs it; anything else runs through the shell with the project venv
//! activated. Typed commands are remembered per project.

use crate::{split_args, Command, RunConfig, RunKind};
use std::collections::HashMap;
use std::path::Path;

/// The history file, next to the run configurations.
pub const HISTORY_FILE: &str = ".brisk/run-anything.txt";
const HISTORY_MAX: usize = 30;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parsed {
    /// Runs with the project interpreter.
    Config(Box<RunConfig>),
    /// A shell command line.
    Shell(String),
}

impl Parsed {
    /// What the popup says the command will do.
    pub fn describe(&self) -> &'static str {
        match self {
            Parsed::Config(c) if c.kind == RunKind::Pytest => "pytest with the project interpreter",
            Parsed::Config(c) if c.module && c.target == "pip" => "pip of the project interpreter",
            Parsed::Config(c) if c.module => "a module with the project interpreter",
            Parsed::Config(_) => "a script with the project interpreter",
            Parsed::Shell(_) => "a shell command, project venv activated",
        }
    }
}

fn quote(t: &str) -> String {
    if t.is_empty() || t.contains(char::is_whitespace) {
        format!("\"{t}\"")
    } else {
        t.to_string()
    }
}

fn is_python(t: &str) -> bool {
    let t = t.to_ascii_lowercase();
    let t = t.strip_suffix(".exe").unwrap_or(&t);
    t == "python" || t == "python3" || t == "py" || t.strip_prefix("python3.").is_some_and(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()))
}

/// How a typed command runs. `None` for a blank line.
pub fn parse(root: &Path, text: &str) -> Option<Parsed> {
    let text = text.trim();
    let tokens = split_args(text);
    let first = tokens.first()?;
    let config = |kind: RunKind, module: bool, target: &str, params: &[String]| {
        Parsed::Config(Box::new(RunConfig {
            name: text.to_string(),
            kind,
            module,
            target: target.to_string(),
            keywords: String::new(),
            parameters: params.iter().map(|t| quote(t)).collect::<Vec<_>>().join(" "),
            interpreter_options: String::new(),
            // PyCharm's Run Anything runs in the project root.
            working_directory: root.display().to_string(),
            env: Default::default(),
            env_files: String::new(),
            interpreter: None,
            temporary: false,
            ..Default::default()
        }))
    };
    let rest = &tokens[1..];
    Some(if is_python(first) {
        match rest {
            [m, module, args @ ..] if m == "-m" && module == "pytest" => config(RunKind::Pytest, false, "", args),
            [m, module, args @ ..] if m == "-m" => config(RunKind::Python, true, module, args),
            [script, args @ ..] if !script.starts_with('-') => config(RunKind::Python, false, script, args),
            // `python`, `python -c …`, `python -V`: the shell (the venv's python is first on PATH).
            _ => Parsed::Shell(text.to_string()),
        }
    } else if first == "pytest" || first == "py.test" {
        config(RunKind::Pytest, false, "", rest)
    } else if first == "pip" || first == "pip3" {
        config(RunKind::Python, true, "pip", rest)
    } else {
        Parsed::Shell(text.to_string())
    })
}

/// The shell process for a command line, with `activation` (the project interpreter's venv
/// or conda environment, see `env::terminal`) like the Terminal.
pub fn shell_command(root: &Path, line: &str, activation: HashMap<String, String>) -> Command {
    let mut env = HashMap::new();
    env.insert("PYTHONUNBUFFERED".into(), "1".into());
    env.insert("PYTHONIOENCODING".into(), "UTF-8".into());
    env.extend(activation);
    let (program, args) = brisk_platform::shell::command_line(line);
    Command { program: program.into(), args, cwd: root.to_path_buf(), env }
}

/// Typed commands, newest first.
pub fn load_history(root: &Path) -> Vec<String> {
    std::fs::read_to_string(root.join(HISTORY_FILE)).map(|t| t.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect()).unwrap_or_default()
}

/// Put `line` first (once) and save.
pub fn remember(root: &Path, history: &mut Vec<String>, line: &str) {
    let line = line.trim().to_string();
    history.retain(|h| *h != line);
    history.insert(0, line);
    history.truncate(HISTORY_MAX);
    let path = root.join(HISTORY_FILE);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, history.join("\n") + "\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(p: Option<Parsed>) -> RunConfig {
        match p {
            Some(Parsed::Config(c)) => *c,
            other => panic!("not a configuration: {other:?}"),
        }
    }

    #[test]
    fn commands() {
        let root = Path::new("/proj");
        let c = cfg(parse(root, "python hello.py a \"b c\""));
        assert_eq!((c.kind, c.module, c.target.as_str(), c.parameters.as_str()), (RunKind::Python, false, "hello.py", "a \"b c\""));
        assert_eq!(c.name, "python hello.py a \"b c\"");
        let c = cfg(parse(root, "python3.12 -m http.server 8000"));
        assert_eq!((c.module, c.target.as_str(), c.parameters.as_str()), (true, "http.server", "8000"));
        let c = cfg(parse(root, "pytest -q tests/test_x.py"));
        assert_eq!((c.kind, c.target.as_str(), c.parameters.as_str()), (RunKind::Pytest, "", "-q tests/test_x.py"));
        assert_eq!(cfg(parse(root, "python -m pytest -x")).kind, RunKind::Pytest);
        let c = cfg(parse(root, "pip install requests==2.31"));
        assert_eq!((c.module, c.target.as_str(), c.parameters.as_str()), (true, "pip", "install requests==2.31"));
        assert_eq!(parse(root, "python -c \"print(1)\""), Some(Parsed::Shell("python -c \"print(1)\"".into())));
        assert_eq!(parse(root, "git status"), Some(Parsed::Shell("git status".into())));
        assert_eq!(parse(root, "   "), None);
    }

    #[test]
    fn history_is_newest_first_and_unique() {
        let dir = std::env::temp_dir().join(format!("brisk-ra-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut h = load_history(&dir);
        remember(&dir, &mut h, "pytest");
        remember(&dir, &mut h, "pip list");
        remember(&dir, &mut h, " pytest ");
        assert_eq!(load_history(&dir), vec!["pytest", "pip list"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
