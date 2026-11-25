//! Run with Coverage, like PyCharm's (coverage.py): the run's command goes through
//! `python -m coverage run`, then `coverage json` gives covered / missed lines per file.

use brisk_run::{split_args, Command, RunConfig};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileCoverage {
    pub path: PathBuf,
    /// 0-based lines.
    pub covered: Vec<usize>,
    pub missed: Vec<usize>,
    pub statements: usize,
    pub percent: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// Project files only, by path.
    pub files: Vec<FileCoverage>,
    pub percent: f64,
}

impl Report {
    #[cfg(test)]
    pub fn file(&self, path: &Path) -> Option<&FileCoverage> {
        self.files.iter().find(|f| f.path == path)
    }
}

/// The data file of a project's coverage runs.
pub fn data_file(root: &Path) -> PathBuf {
    root.join(".brisk").join("coverage.data")
}

/// `python [opts] -m coverage run --data-file=F <the rest>`: the run's own arguments follow
/// coverage's, so scripts, `-m module` and pytest all work.
pub fn wrap(cmd: &mut Command, cfg: &RunConfig, data: &Path) {
    let at = split_args(&cfg.interpreter_options).len().min(cmd.args.len());
    let extra = ["-m".to_string(), "coverage".into(), "run".into(), format!("--data-file={}", data.display())];
    cmd.args.splice(at..at, extra);
}

/// `python -m coverage json` for a finished run: run it in `cwd` (paths in the report are
/// relative to it).
pub fn json_command(python: &Path, data: &Path, out: &Path, cwd: &Path) -> std::process::Command {
    let mut c = std::process::Command::new(python);
    c.args(["-m", "coverage", "json", "-q", "--ignore-errors"]).arg(format!("--data-file={}", data.display())).arg("-o").arg(out).current_dir(cwd);
    c.env("PYTHONIOENCODING", "utf-8");
    brisk_platform::process::no_window(&mut c);
    c
}

/// Parse `coverage json` output; keep files under `root` (paths resolved against `cwd`).
pub fn parse(json: &str, root: &Path, cwd: &Path) -> Result<Report, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("Bad coverage report: {e}"))?;
    let lines = |x: &serde_json::Value| -> Vec<usize> {
        x.as_array().map(|a| a.iter().filter_map(|n| n.as_u64()).map(|n| (n as usize).saturating_sub(1)).collect()).unwrap_or_default()
    };
    let mut files = Vec::new();
    if let Some(map) = v.get("files").and_then(|f| f.as_object()) {
        for (name, f) in map {
            let p = PathBuf::from(name);
            let path = if p.is_absolute() { p } else { cwd.join(p) };
            let path = dunce(&path);
            if !path.starts_with(dunce(root)) {
                continue;
            }
            let s = &f["summary"];
            files.push(FileCoverage {
                path,
                covered: lines(&f["executed_lines"]),
                missed: lines(&f["missing_lines"]),
                statements: s["num_statements"].as_u64().unwrap_or(0) as usize,
                percent: s["percent_covered"].as_f64().unwrap_or(0.0),
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    // The total over the files shown, as the Coverage window lists them.
    let (stmts, covered) = files.iter().fold((0, 0), |(s, c), f| (s + f.statements, c + f.covered.len()));
    let percent = if stmts == 0 { 100.0 } else { covered as f64 * 100.0 / stmts as f64 };
    Ok(Report { files, percent })
}

/// `a/./b/../c` → `a/c`, without touching the disk (Windows `\\?\` stays out of the way).
fn dunce(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn wraps_after_interpreter_options() {
        let root = Path::new("/p");
        let cfg = RunConfig { target: "app.py".into(), parameters: "x".into(), interpreter_options: "-X dev".into(), ..Default::default() };
        let mut cmd =
            Command { program: "python".into(), args: vec!["-X".into(), "dev".into(), "/p/app.py".into(), "x".into()], cwd: root.into(), env: HashMap::new() };
        wrap(&mut cmd, &cfg, Path::new("/p/.brisk/coverage.data"));
        assert_eq!(cmd.args[..6], ["-X", "dev", "-m", "coverage", "run", "--data-file=/p/.brisk/coverage.data"]);
        assert_eq!(cmd.args[6..], ["/p/app.py", "x"]);
    }

    #[test]
    fn parses_project_files() {
        let root = Path::new("/proj");
        let json = r#"{"meta": {}, "files": {
            "app.py": {"executed_lines": [1, 2, 4], "missing_lines": [5], "summary": {"num_statements": 4, "percent_covered": 75.0}},
            "pkg/../lib.py": {"executed_lines": [1], "missing_lines": [], "summary": {"num_statements": 1, "percent_covered": 100.0}},
            "/elsewhere/site.py": {"executed_lines": [1], "missing_lines": [], "summary": {"num_statements": 1, "percent_covered": 100.0}}
        }, "totals": {}}"#;
        let r = parse(json, root, root).unwrap();
        assert_eq!(r.files.len(), 2);
        let app = r.file(&root.join("app.py")).unwrap();
        assert_eq!((app.covered.clone(), app.missed.clone(), app.statements), (vec![0, 1, 3], vec![4], 4));
        assert!(r.file(&root.join("lib.py")).is_some());
        assert!((r.percent - 80.0).abs() < 1e-9);
    }
}
