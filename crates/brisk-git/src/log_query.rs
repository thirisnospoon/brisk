//! Git log filters (text, author, branch, date, path) and the lists the filter popups offer.

use crate::{parse_log, Commit, Repo, Result};

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct LogQuery {
    /// Part of the message (case-insensitive) or the start of a hash.
    pub text: String,
    pub author: Option<String>,
    /// Only commits reachable from this branch; `None`: all branches.
    pub branch: Option<String>,
    /// Only commits of the last N days (the Date filter).
    pub since_days: Option<u32>,
    /// Only commits that touch this file or folder (repository-relative; the Paths filter).
    pub path: Option<String>,
}

impl LogQuery {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.author.is_none() && self.branch.is_none() && self.since_days.is_none() && self.path.is_none()
    }

    /// The label of the Date filter: the name of the chosen [`DATES`] entry.
    pub fn date_label(&self) -> &'static str {
        DATES.iter().find(|d| d.1 == self.since_days).map_or("All", |d| d.0)
    }

    /// The label of the Paths filter: `All`, or the path; a long one shows its end.
    pub fn path_label(&self) -> String {
        match self.path.as_deref() {
            None => "All".to_string(),
            Some(p) if p.chars().count() > 28 => format!("…{}", p.chars().rev().take(27).collect::<Vec<_>>().into_iter().rev().collect::<String>()),
            Some(p) => p.to_string(),
        }
    }
}

/// The Date filter's choices (PyCharm's list), as days back.
pub const DATES: [(&str, Option<u32>); 6] =
    [("All", None), ("Last 24 hours", Some(1)), ("Last 7 days", Some(7)), ("Last 30 days", Some(30)), ("Last 90 days", Some(90)), ("Last year", Some(365))];

/// The top-level folders of `root` for the Paths filter (`name/`, hidden ones left out),
/// sorted case-insensitively.
pub fn top_level_dirs(root: &std::path::Path) -> Vec<String> {
    let mut dirs: Vec<String> = std::fs::read_dir(root)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| !n.starts_with('.'))
                .map(|n| format!("{n}/"))
                .collect()
        })
        .unwrap_or_default();
    dirs.sort_by_key(|d| d.to_lowercase());
    dirs
}

/// The entries of the Paths filter after `All`: the active file and its folder, the chosen
/// path when it is neither, then up to 30 of `dirs`. The number is where `dirs` start (the
/// menu puts a separator there).
pub fn path_choices(active_file: Option<String>, current: Option<&str>, mut dirs: Vec<String>) -> (Vec<String>, usize) {
    let mut shown: Vec<String> = Vec::new();
    if let Some(f) = active_file {
        if let Some((dir, _)) = f.rsplit_once('/') {
            shown.push(format!("{dir}/"));
        }
        shown.insert(0, f);
    }
    // The chosen path stays in the list even when it is none of the others.
    if let Some(c) = current.filter(|c| !shown.iter().any(|s| s == c)) {
        shown.push(c.to_string());
    }
    let n = shown.len();
    dirs.retain(|d| !shown.contains(d));
    shown.extend(dirs.into_iter().take(30));
    (shown, n)
}

use crate::LOG_FORMAT as FORMAT;

impl Repo {
    /// The Git log with filters; an empty query is the whole log (`Repo::log`).
    pub fn log_query(&self, limit: usize, q: &LogQuery) -> Result<Vec<Commit>> {
        if q.is_empty() {
            return self.log(limit);
        }
        let n = format!("-n{limit}");
        let mut args: Vec<String> = vec!["log".into(), "--date-order".into(), n, FORMAT.into(), "-i".into(), "--fixed-strings".into()];
        let text = q.text.trim();
        if !text.is_empty() {
            args.push(format!("--grep={text}"));
        }
        if let Some(a) = &q.author {
            args.push(format!("--author={a}"));
        }
        match &q.branch {
            Some(b) => args.push(b.clone()),
            None => args.extend(["--branches", "--remotes", "--tags", "HEAD"].map(String::from)),
        }
        args.push("--".into());
        if let Some(p) = q.path.as_ref().filter(|p| !p.is_empty()) {
            args.push(p.clone());
        }
        // The Date filter. `--since` stops walking a branch at its first older commit;
        // `--since-as-filter` (git 2.37+) filters, the plain cut-off is the fallback.
        let run = |flag: &str| {
            let mut a: Vec<&str> = args.iter().map(String::as_str).collect();
            let since = q.since_days.map(|d| format!("{flag}={d} days ago"));
            if let Some(s) = &since {
                a.insert(1, s);
            }
            self.git(&a)
        };
        let mut res = run("--since-as-filter");
        if q.since_days.is_some() && res.as_ref().is_err_and(|e| e.0.contains("since-as-filter")) {
            res = run("--since");
        }
        let mut v = match res {
            Ok(out) => parse_log(&out),
            Err(e) if e.0.contains("does not have any commits") || e.0.contains("unknown revision") => Vec::new(),
            Err(e) => return Err(e),
        };
        // A hash typed in the text filter: that commit too.
        let lower = text.to_lowercase();
        if text.len() >= 4 && text.chars().all(|c| c.is_ascii_hexdigit()) && !v.iter().any(|c| c.hash.starts_with(&lower)) {
            let rev = format!("{lower}^{{commit}}");
            if let Ok(out) = self.git(&["log", "-1", FORMAT, &rev, "--"]) {
                let hit: Vec<Commit> = parse_log(&out).into_iter().filter(|c| c.hash.starts_with(&lower)).collect();
                v.extend(hit);
                v.sort_by_key(|c| std::cmp::Reverse(c.time));
            }
        }
        Ok(v)
    }

    /// Authors for the User filter, the most active first.
    pub fn authors(&self) -> Result<Vec<String>> {
        let out = self.git(&["shortlog", "-sn", "--all", "--no-merges", "HEAD"])?;
        Ok(out.lines().filter_map(|l| l.trim().split_once('\t').map(|(_, n)| n.trim().to_string())).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        let mut q = LogQuery::default();
        assert_eq!((q.date_label(), q.path_label().as_str()), ("All", "All"));
        q.since_days = Some(7);
        q.path = Some("src/app.py".into());
        assert_eq!((q.date_label(), q.path_label().as_str()), ("Last 7 days", "src/app.py"));
        q.since_days = Some(3);
        q.path = Some("very/long/path/to/some/module/file.py".into());
        assert_eq!(q.date_label(), "All");
        assert_eq!(q.path_label(), "…path/to/some/module/file.py");
        assert_eq!(q.path_label().chars().count(), 28);
    }

    #[test]
    fn path_choices_order() {
        let dirs = vec!["app/".to_string(), "docs/".into(), "tests/".into()];
        let (v, n) = path_choices(Some("app/main.py".into()), None, dirs.clone());
        assert_eq!(v, ["app/main.py", "app/", "docs/", "tests/"]);
        assert_eq!(n, 2);
        let (v, n) = path_choices(None, Some("lib/x.py"), dirs.clone());
        assert_eq!(v, ["lib/x.py", "app/", "docs/", "tests/"]);
        assert_eq!(n, 1);
        let (v, n) = path_choices(Some("main.py".into()), Some("docs/"), dirs);
        assert_eq!(v, ["main.py", "docs/", "app/", "tests/"]);
        assert_eq!(n, 2);
        let many: Vec<String> = (0..40).map(|i| format!("d{i:02}/")).collect();
        assert_eq!(path_choices(None, None, many).0.len(), 30);
    }

    #[test]
    fn top_level_dirs_skip_hidden_and_files() {
        let dir = std::env::temp_dir().join(format!("brisk-git-dirs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for d in ["src", "Docs", ".git", "build"] {
            std::fs::create_dir_all(dir.join(d)).expect("create a folder");
        }
        std::fs::write(dir.join("README.md"), "x").expect("write a file");
        assert_eq!(top_level_dirs(&dir), ["build/", "Docs/", "src/"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
