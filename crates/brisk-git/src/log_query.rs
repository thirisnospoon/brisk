//! Git log filters (text, author, branch, date, path) and the lists the filter popups offer.

use crate::{parse_log, Commit, Repo, Result};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
                v.sort_by(|a, b| b.time.cmp(&a.time));
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
