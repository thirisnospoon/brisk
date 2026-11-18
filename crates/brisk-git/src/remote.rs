//! Push dialog data (outgoing commits) and Update Project / Fetch with a summary.

use crate::{parse_log, Commit, GitError, Repo, Result};

const FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%s%x1e";

#[derive(Clone, Debug)]
pub struct Outgoing {
    pub branch: String,
    /// "origin/main"
    pub target: String,
    /// The branch is not on the remote yet: the push creates it and sets the upstream.
    pub new_branch: bool,
    /// Newest first.
    pub commits: Vec<Commit>,
}

impl Repo {
    pub fn remotes(&self) -> Result<Vec<String>> {
        Ok(self.git(&["remote"])?.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
    }

    /// What Push would send: commits of the current branch that its upstream (or, for a new
    /// branch, no remote branch) has.
    pub fn outgoing(&self) -> Result<Outgoing> {
        let st = self.status()?;
        let branch = st.branch.ok_or_else(|| GitError("Detached HEAD: check out a branch to push".into()))?;
        let remotes = self.remotes()?;
        let Some(remote) = remotes.iter().find(|r| *r == "origin").or(remotes.first()).cloned() else {
            return Err(GitError("No remote is configured: add one with `git remote add origin <url>`".into()));
        };
        let (target, new_branch, range) = match &st.upstream {
            Some(u) => (u.clone(), false, vec!["@{u}..HEAD".to_string()]),
            None => (format!("{remote}/{branch}"), true, vec!["HEAD".to_string(), "--not".to_string(), "--remotes".to_string()]),
        };
        let mut args = vec!["log".to_string(), "-n500".to_string(), FORMAT.to_string()];
        args.extend(range);
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let commits = parse_log(&self.git(&refs)?);
        Ok(Outgoing { branch, target, new_branch, commits })
    }

    /// Push the current branch as `Outgoing` describes it; returns the summary for the toast.
    pub fn push_outgoing(&self, o: &Outgoing) -> Result<String> {
        let (remote, _) = o.target.split_once('/').unwrap_or(("origin", ""));
        if o.new_branch {
            self.git(&["push", "--set-upstream", remote, &o.branch])?;
        } else {
            self.git(&["push"])?;
        }
        let n = o.commits.len();
        Ok(match (n, o.new_branch) {
            (_, true) => format!("Pushed {} to new branch {}", plural(n, "commit"), o.target),
            (0, false) => "Everything is up-to-date".to_string(),
            _ => format!("Pushed {} to {}", plural(n, "commit"), o.target),
        })
    }

    /// Update Project (pull with merge): how many commits and files came in.
    pub fn update_project(&self) -> Result<String> {
        let before = self.git(&["rev-parse", "HEAD"]).map(|s| s.trim().to_string()).ok();
        self.pull()?;
        let after = self.git(&["rev-parse", "HEAD"])?.trim().to_string();
        match before {
            Some(b) if b == after => Ok("All files are up-to-date".to_string()),
            Some(b) => {
                let n: usize = self.git(&["rev-list", "--count", &format!("{b}..{after}")])?.trim().parse().unwrap_or(0);
                let files = self.git(&["diff", "--name-only", &b, &after])?.lines().filter(|l| !l.is_empty()).count();
                Ok(format!("Updated: {}, {} changed", plural(n, "commit"), plural(files, "file")))
            }
            None => Ok("Project updated".to_string()),
        }
    }

    /// Fetch: all remotes; reports what the current branch's upstream has that it does not.
    pub fn fetch_summary(&self) -> Result<String> {
        self.fetch()?;
        let st = self.status()?;
        Ok(match (&st.upstream, st.behind) {
            (None, _) => "Fetched all remotes".to_string(),
            (Some(_), 0) => "Fetched: no incoming commits".to_string(),
            (Some(u), n) => format!("Fetched: {} incoming from {u}", plural(n as usize, "commit")),
        })
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}
