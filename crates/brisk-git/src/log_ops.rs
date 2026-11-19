//! The Log's commit actions (PyCharm's commit context menu): Cherry-Pick, Checkout Revision,
//! Reset Current Branch to Here, Revert Commit, Undo Commit.

use crate::{GitError, Repo, Result};
use std::path::PathBuf;

/// `git reset` modes, as PyCharm's Git Reset dialog names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
    Keep,
}

impl ResetMode {
    pub const ALL: [ResetMode; 4] = [ResetMode::Soft, ResetMode::Mixed, ResetMode::Hard, ResetMode::Keep];

    fn flag(self) -> &'static str {
        match self {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
            ResetMode::Keep => "--keep",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ResetMode::Soft => "Soft",
            ResetMode::Mixed => "Mixed",
            ResetMode::Hard => "Hard",
            ResetMode::Keep => "Keep",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            ResetMode::Soft => "Files won't change, differences will be staged for commit",
            ResetMode::Mixed => "Files won't change, differences won't be staged",
            ResetMode::Hard => "Files will be reverted to the state of the selected commit. Any local changes will be lost",
            ResetMode::Keep => "Files will be reverted to the state of the selected commit, local changes will be kept",
        }
    }
}

impl Repo {
    fn git_dir(&self) -> Option<PathBuf> {
        self.git(&["rev-parse", "--absolute-git-dir"]).ok().map(|s| PathBuf::from(s.trim()))
    }

    /// A merge, cherry-pick or revert that stopped and waits for its commit.
    pub fn pending_operation(&self) -> Option<&'static str> {
        let d = self.git_dir()?;
        [("MERGE_HEAD", "merge"), ("CHERRY_PICK_HEAD", "cherry-pick"), ("REVERT_HEAD", "revert")]
            .into_iter()
            .find(|(f, _)| d.join(f).exists())
            .map(|(_, op)| op)
    }

    /// A failed history command: on conflicts, say how to go on (refresh opens the dialog).
    fn stopped(&self, what: &str, e: GitError) -> GitError {
        let n = self.conflicts().map(|c| c.len()).unwrap_or(0);
        if n > 0 {
            GitError(format!("{what} stopped on conflicts in {n} file(s): resolve them, then commit"))
        } else {
            e
        }
    }

    fn is_merge_commit(&self, hash: &str) -> bool {
        self.git(&["rev-list", "--parents", "-n1", hash]).is_ok_and(|s| s.split_whitespace().count() > 2)
    }

    /// Cherry-Pick: apply `hash` on top of the current branch as a new commit.
    pub fn cherry_pick(&self, hash: &str) -> Result<String> {
        let mut args = vec!["cherry-pick"];
        if self.is_merge_commit(hash) {
            args.extend(["-m", "1"]);
        }
        args.push(hash);
        self.git(&args).map_err(|e| self.stopped("Cherry-pick", e))?;
        let short = self.git(&["rev-parse", "--short", "HEAD"])?;
        Ok(format!("Cherry-picked {} as {}", &hash[..hash.len().min(8)], short.trim()))
    }

    /// Revert Commit: a new commit that undoes `hash`.
    pub fn revert_commit(&self, hash: &str) -> Result<String> {
        let mut args = vec!["revert", "--no-edit"];
        if self.is_merge_commit(hash) {
            args.extend(["-m", "1"]);
        }
        args.push(hash);
        self.git(&args).map_err(|e| self.stopped("Revert", e))?;
        Ok(format!("Reverted {}", &hash[..hash.len().min(8)]))
    }

    /// Reset Current Branch to Here.
    pub fn reset_to(&self, hash: &str, mode: ResetMode) -> Result<String> {
        self.git(&["reset", "-q", mode.flag(), hash])?;
        Ok(format!("{} reset to {}", mode.label(), &hash[..hash.len().min(8)]))
    }

    /// Checkout Revision: a detached HEAD at `hash`.
    pub fn checkout_revision(&self, hash: &str) -> Result<String> {
        self.git(&["checkout", "-q", "--detach", hash])?;
        Ok(format!("Checked out {} (detached HEAD)", &hash[..hash.len().min(8)]))
    }

    /// Undo Commit: the last commit goes back to local changes (`reset --soft HEAD~1`).
    pub fn undo_commit(&self, hash: &str) -> Result<String> {
        let head = self.git(&["rev-parse", "HEAD"])?;
        if head.trim() != hash {
            return Err(GitError("Only the last commit can be undone".into()));
        }
        if self.git(&["rev-parse", "-q", "--verify", "HEAD~1"]).is_err() {
            return Err(GitError("The first commit of the repository can't be undone".into()));
        }
        self.git(&["reset", "-q", "--soft", "HEAD~1"])?;
        Ok(format!("Undid commit {}: its changes are local changes again", &hash[..hash.len().min(8)]))
    }
}
