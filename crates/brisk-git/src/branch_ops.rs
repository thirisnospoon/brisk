//! Branches popup actions: Merge into Current and Delete.

use crate::{GitError, Repo, Result};

impl Repo {
    /// Merge `name` into the current branch (a merge commit, or a fast-forward). Conflicts stop
    /// it with an error; the Conflicts dialog takes over.
    pub fn merge_into_current(&self, name: &str) -> Result<String> {
        let out = self.git(&["merge", "--no-edit", name])?;
        Ok(if out.contains("Already up to date") {
            "Already up to date".to_string()
        } else if out.contains("Fast-forward") {
            format!("Fast-forwarded to {name}")
        } else {
            format!("Merged {name}")
        })
    }

    /// Whether every commit of `name` is in HEAD (`git branch -d` would succeed).
    pub fn is_merged(&self, name: &str) -> bool {
        self.git(&["merge-base", "--is-ancestor", name, "HEAD"]).is_ok()
    }

    /// `git rev-list --count <range>` (0 on errors).
    pub fn git_count(&self, range: &str) -> usize {
        self.git(&["rev-list", "--count", range]).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
    }

    /// Delete a local branch; `force` deletes one that is not merged (`-D`). Never the current one.
    pub fn delete_branch(&self, name: &str, force: bool) -> Result<()> {
        let current = self.git(&["branch", "--show-current"])?;
        if current.trim() == name {
            return Err(GitError(format!("{name} is the current branch: check out another one first")));
        }
        self.git(&["branch", if force { "-D" } else { "-d" }, name]).map(|_| ())
    }
}
