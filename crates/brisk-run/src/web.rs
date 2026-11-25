//! FastAPI (uvicorn) and Flask server configurations, like PyCharm's templates: finding the
//! application object in the project and the server's command line.

use crate::{split_args, RunConfig, RunKind};
use std::path::{Path, PathBuf};

/// The application in a module's source: `(kind, "app")`, or `"create_app()"` for a Flask
/// factory. Only top-level assignments count (`app = FastAPI(...)`, `app = Flask(__name__)`).
pub fn app_in_source(src: &str) -> Option<(RunKind, String)> {
    let mut factory = None;
    for line in src.lines() {
        if line.starts_with([' ', '\t']) {
            continue;
        }
        if let Some((lhs, rhs)) = line.split_once('=') {
            let var = lhs.trim().split(':').next().unwrap_or("").trim();
            if var.is_empty() || !var.chars().all(|c| c.is_alphanumeric() || c == '_') || rhs.starts_with('=') {
                continue;
            }
            let rhs = rhs.trim_start();
            let call = rhs.split('(').next().unwrap_or("");
            if rhs.contains('(') {
                match call.rsplit('.').next().unwrap_or("") {
                    "FastAPI" => return Some((RunKind::FastApi, var.to_string())),
                    "Flask" => return Some((RunKind::Flask, var.to_string())),
                    _ => {}
                }
            }
        }
        for f in ["create_app", "make_app"] {
            if line.starts_with(&format!("def {f}(")) && src.contains("flask") {
                factory = factory.or(Some((RunKind::Flask, format!("{f}()"))));
            }
        }
    }
    factory
}

/// `pkg/main.py` → `pkg.main` (`pkg/__init__.py` → `pkg`), relative to `root`.
pub fn module_of(root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(root).ok()?.with_extension("");
    let mut parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    if parts.last().is_some_and(|p| p == "__init__") {
        parts.pop();
    }
    (!parts.is_empty()).then(|| parts.join("."))
}

/// A file's application as a server target: `(kind, "pkg.main:app")`.
pub fn app_target(root: &Path, file: &Path, src: &str) -> Option<(RunKind, String)> {
    let (kind, var) = app_in_source(src)?;
    Some((kind, format!("{}:{var}", module_of(root, file)?)))
}

/// Folders never searched for an application.
const SKIP: &[&str] = &["venv", "env", "node_modules", "site-packages", "__pycache__", "build", "dist", "tests", "test"];

/// The project's application: the first found breadth-first (top-level `main.py` and
/// `app.py` before other files), at most three folders deep. `kind` narrows the search.
pub fn find_app(root: &Path, kind: Option<RunKind>) -> Option<(RunKind, PathBuf, String)> {
    let mut level = vec![root.to_path_buf()];
    for _ in 0..3 {
        let mut next = Vec::new();
        for dir in &level {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
            let rank = |p: &PathBuf| match p.file_name().and_then(|n| n.to_str()) {
                Some("main.py") => 0,
                Some("app.py") => 1,
                Some("__init__.py") => 3,
                _ => 2,
            };
            entries.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.cmp(b)));
            for p in entries {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if p.is_dir() {
                    if !name.starts_with('.') && !SKIP.contains(&name.as_str()) && !p.join("pyvenv.cfg").exists() {
                        next.push(p);
                    }
                } else if name.ends_with(".py") {
                    let Ok(src) = std::fs::read_to_string(&p) else { continue };
                    if let Some((k, target)) = app_target(root, &p, &src).filter(|(k, _)| kind.is_none_or(|w| w == *k)) {
                        return Some((k, p, target));
                    }
                }
            }
        }
        level = next;
    }
    None
}

impl RunConfig {
    /// The temporary configuration for a file that defines a FastAPI / Flask application.
    pub fn for_web(kind: RunKind, file: &Path, target: String) -> Self {
        let stem = file.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let name = format!("{} ({stem})", if kind == RunKind::Flask { "Flask" } else { "FastAPI" });
        RunConfig { name, kind, target, temporary: true, ..Default::default() }
    }
}

/// The package a server kind needs in the interpreter.
pub fn server_package(kind: RunKind) -> Option<&'static str> {
    match kind {
        RunKind::FastApi => Some("uvicorn"),
        RunKind::Flask => Some("flask"),
        RunKind::Django | RunKind::DjangoTests => Some("django"),
        _ => None,
    }
}

/// The arguments after the interpreter options: `-m uvicorn main:app …` (a factory target
/// `mod:create_app()` adds `--factory`) or `-m flask --app app run …`.
pub fn args(cfg: &RunConfig) -> Vec<String> {
    let target = cfg.target.trim();
    let mut v: Vec<String> = match cfg.kind {
        RunKind::Flask => vec!["-m".into(), "flask".into(), "--app".into(), target.into(), "run".into()],
        _ => match target.strip_suffix("()") {
            Some(f) => vec!["-m".into(), "uvicorn".into(), f.into(), "--factory".into()],
            None => vec!["-m".into(), "uvicorn".into(), target.into()],
        },
    };
    v.extend(split_args(&cfg.parameters));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_apps_and_builds_commands() {
        assert_eq!(app_in_source("from fastapi import FastAPI\n\napp = FastAPI(title='x')\n"), Some((RunKind::FastApi, "app".into())));
        assert_eq!(app_in_source("import flask\nserver: flask.Flask = flask.Flask(__name__)\n"), Some((RunKind::Flask, "server".into())));
        assert_eq!(
            app_in_source("from flask import Flask\n\ndef create_app():\n    app = Flask(__name__)\n    return app\n"),
            Some((RunKind::Flask, "create_app()".into()))
        );
        assert_eq!(app_in_source("def f():\n    app = FastAPI()\nx == FastAPI()\n"), None);
        let root = std::env::temp_dir().join(format!("brisk-web-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("svc/api")).unwrap();
        std::fs::create_dir_all(root.join(".venv")).unwrap();
        std::fs::write(root.join(".venv/fake.py"), "app = FastAPI()\n").unwrap();
        std::fs::write(root.join("util.py"), "x = 1\n").unwrap();
        std::fs::write(root.join("svc/api/server.py"), "from fastapi import FastAPI\napp = FastAPI()\n").unwrap();
        std::fs::write(root.join("svc/web.py"), "from flask import Flask\nweb = Flask(__name__)\n").unwrap();
        let found = find_app(&root, None).unwrap();
        assert_eq!((found.0, found.2.as_str()), (RunKind::Flask, "svc.web:web"));
        assert_eq!(find_app(&root, Some(RunKind::FastApi)).unwrap().2, "svc.api.server:app");
        let _ = std::fs::remove_dir_all(&root);
        let cfg = RunConfig { kind: RunKind::FastApi, target: "main:app".into(), parameters: "--reload --port 8001".into(), ..Default::default() };
        assert_eq!(args(&cfg), ["-m", "uvicorn", "main:app", "--reload", "--port", "8001"]);
        let cfg = RunConfig { kind: RunKind::FastApi, target: "main:create_app()".into(), ..Default::default() };
        assert_eq!(args(&cfg), ["-m", "uvicorn", "main:create_app", "--factory"]);
        let cfg = RunConfig { kind: RunKind::Flask, target: "app:create_app()".into(), parameters: "--debug".into(), ..Default::default() };
        assert_eq!(args(&cfg), ["-m", "flask", "--app", "app:create_app()", "run", "--debug"]);
    }

    #[test]
    fn urls_in_output() {
        let line = "INFO:     Uvicorn running on http://127.0.0.1:8000 (Press CTRL+C to quit)";
        let u = crate::find_urls(line);
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].1, "http://127.0.0.1:8000");
        assert_eq!(&line[u[0].0.clone()], "http://127.0.0.1:8000");
        let u = crate::find_urls(" * Running on http://0.0.0.0:5000/. Done, see <https://x.org/a?b=1>");
        assert_eq!(u.iter().map(|u| u.1.as_str()).collect::<Vec<_>>(), ["http://0.0.0.0:5000/", "https://x.org/a?b=1"]);
    }
}
