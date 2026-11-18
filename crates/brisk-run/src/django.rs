//! Django configurations, like PyCharm's Django Server and Django tests: `manage.py
//! runserver` / `manage.py test`, with `manage.py` and the settings module found in the project.

use crate::{relativize, resolve, split_args, RunConfig, RunKind};
use std::path::{Path, PathBuf};

/// The settings module `manage.py` sets: `os.environ.setdefault("DJANGO_SETTINGS_MODULE", "x.settings")`.
pub fn settings_in_manage(src: &str) -> Option<String> {
    let at = src.find("DJANGO_SETTINGS_MODULE")?;
    let rest = &src[at + "DJANGO_SETTINGS_MODULE".len()..];
    let rest = rest.trim_start_matches(['"', '\'']).trim_start();
    let rest = rest.strip_prefix(',')?.trim_start();
    let q = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let body = &rest[1..];
    Some(body[..body.find(q)?].to_string())
}

pub const SETTINGS_VAR: &str = "DJANGO_SETTINGS_MODULE";

/// Folders never searched for `manage.py`.
const SKIP: &[&str] = &["venv", "env", "node_modules", "site-packages", "__pycache__", "build", "dist"];

/// The project's `manage.py` (a Django one: it mentions django), breadth-first, at most three
/// folders deep, with its settings module when it names one.
pub fn find_manage(root: &Path) -> Option<(PathBuf, Option<String>)> {
    let mut level = vec![root.to_path_buf()];
    for _ in 0..3 {
        let mut next = Vec::new();
        for dir in &level {
            let m = dir.join("manage.py");
            if let Ok(src) = std::fs::read_to_string(&m) {
                if src.contains("django") {
                    return Some((m, settings_in_manage(&src)));
                }
            }
            let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
            dirs.sort();
            for p in dirs {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if !name.starts_with('.') && !SKIP.contains(&name.as_str()) && !p.join("pyvenv.cfg").exists() {
                    next.push(p);
                }
            }
        }
        level = next;
    }
    None
}

impl RunConfig {
    /// A Django Server / Django tests configuration for a `manage.py`, with
    /// `DJANGO_SETTINGS_MODULE` set to the settings module it names (as PyCharm's Django
    /// support does).
    pub fn for_django(root: &Path, kind: RunKind, manage: &Path, temporary: bool) -> Self {
        let name = if kind == RunKind::DjangoTests { "Django tests" } else { "Django server" };
        let mut cfg = RunConfig { name: name.into(), kind, target: relativize(root, manage), temporary, ..Default::default() };
        if let Some(s) = std::fs::read_to_string(manage).ok().and_then(|src| settings_in_manage(&src)) {
            cfg.env.insert(SETTINGS_VAR.into(), s);
        }
        cfg
    }
}

/// `manage.py runserver …` / `manage.py test …` (after the interpreter options) and the
/// working directory: the folder of `manage.py` unless the configuration sets one.
pub fn command(root: &Path, cfg: &RunConfig) -> (Vec<String>, PathBuf) {
    // Components re-joined: `site/manage.py` gets the platform's separators.
    let manage: PathBuf = resolve(root, cfg.target.trim()).components().collect();
    let sub = if cfg.kind == RunKind::DjangoTests { "test" } else { "runserver" };
    let mut v = vec![manage.display().to_string(), sub.to_string()];
    v.extend(split_args(&cfg.parameters));
    let cwd = if cfg.working_directory.trim().is_empty() { manage.parent().map(Path::to_path_buf).unwrap_or_else(|| root.to_path_buf()) } else { resolve(root, &cfg.working_directory) };
    (v, cwd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_manage_and_builds_commands() {
        let src = "import os\n\ndef main():\n    os.environ.setdefault('DJANGO_SETTINGS_MODULE', 'mysite.settings')\n    from django.core.management import execute_from_command_line\n";
        assert_eq!(settings_in_manage(src).as_deref(), Some("mysite.settings"));
        assert_eq!(settings_in_manage("os.environ.setdefault(\"DJANGO_SETTINGS_MODULE\", \"a.b\")").as_deref(), Some("a.b"));
        let root = std::env::temp_dir().join(format!("brisk-django-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("tools")).unwrap();
        std::fs::create_dir_all(root.join("backend/mysite")).unwrap();
        std::fs::write(root.join("tools/manage.py"), "print('not django')\n").unwrap();
        std::fs::write(root.join("backend/manage.py"), src).unwrap();
        let (m, settings) = find_manage(&root).unwrap();
        assert_eq!((m, settings.as_deref()), (root.join("backend/manage.py"), Some("mysite.settings")));
        let cfg = RunConfig { parameters: "127.0.0.1:8001 --noreload".into(), ..RunConfig::for_django(&root, RunKind::Django, &root.join("backend/manage.py"), false) };
        assert_eq!(cfg.target, "backend/manage.py");
        assert_eq!(cfg.env.get(SETTINGS_VAR).map(String::as_str), Some("mysite.settings"));
        let (args, cwd) = command(&root, &cfg);
        assert_eq!(args[1..], ["runserver", "127.0.0.1:8001", "--noreload"]);
        assert_eq!(cwd, root.join("backend"));
        let t = RunConfig { parameters: "polls -v 2".into(), ..RunConfig::for_django(&root, RunKind::DjangoTests, &root.join("backend/manage.py"), true) };
        assert_eq!((t.name.as_str(), command(&root, &t).0[1..].to_vec()), ("Django tests", vec!["test".to_string(), "polls".into(), "-v".into(), "2".into()]));
        let _ = std::fs::remove_dir_all(&root);
    }
}
