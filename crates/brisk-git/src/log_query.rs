//! Git log filters (text, author, branch) and the lists the filter popups offer.

use crate::{parse_log, Commit, Repo, Result};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogQuery {
    /// Part of the message (case-insensitive) or the start of a hash.
    pub text: String,
    pub author: Option<String>,
    /// Only commits reachable from this branch; `None`: all branches.
    pub branch: Option<String>,
}

impl LogQuery {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.author.is_none() && self.branch.is_none()
    }
}

const FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%s%x1e";

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
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let mut v = match self.git(&refs) {
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
