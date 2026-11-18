//! Stash Changes / Unstash Changes.

use crate::{FileChange, Repo, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StashEntry {
    /// `stash@{0}`
    pub name: String,
    /// "On main: message" / "WIP on main: …"
    pub message: String,
    pub time: i64,
}

impl Repo {
    pub fn stash_list(&self) -> Result<Vec<StashEntry>> {
        let out = self.git(&["stash", "list", "--format=%gd%x1f%gs%x1f%ct"])?;
        Ok(out
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\x1f').collect();
                (f.len() >= 3).then(|| StashEntry { name: f[0].to_string(), message: f[1].to_string(), time: f[2].trim().parse().unwrap_or(0) })
            })
            .collect())
    }

    /// Stash Changes: local changes (optionally untracked files too) go to a new stash.
    pub fn stash_push(&self, message: &str, untracked: bool) -> Result<String> {
        let mut args = vec!["stash", "push"];
        if untracked {
            args.push("--include-untracked");
        }
        if !message.trim().is_empty() {
            args.extend(["-m", message]);
        }
        self.git(&args)
    }

    /// Unstash: apply (`pop` also drops it when it applied cleanly).
    pub fn stash_apply(&self, name: &str, pop: bool) -> Result<String> {
        self.git(&["stash", if pop { "pop" } else { "apply" }, name])
    }

    pub fn stash_drop(&self, name: &str) -> Result<String> {
        self.git(&["stash", "drop", name])
    }

    /// Files a stash changes (relative to the commit it was made on).
    pub fn stash_files(&self, name: &str) -> Result<Vec<FileChange>> {
        let mut v = self.commit_files(name)?;
        // Untracked files live in the stash's third parent.
        let untracked = format!("{name}^3");
        if let Ok(out) = self.git(&["show", "--format=", "--name-only", "-z", &untracked]) {
            for p in out.split('\0').filter(|s| !s.is_empty()) {
                if !v.iter().any(|c| c.path == p) {
                    v.push(FileChange { path: p.to_string(), old_path: None, status: crate::FileStatus::Added });
                }
            }
        }
        Ok(v)
    }

    /// (before, after) of a file in a stash, for the diff preview.
    pub fn stash_file_versions(&self, name: &str, c: &FileChange) -> (String, String) {
        let base = c.old_path.clone().unwrap_or_else(|| c.path.clone());
        let before = self.file_at(&format!("{name}^1"), &base).ok().flatten().unwrap_or_default();
        let after = self
            .file_at(name, &c.path)
            .ok()
            .flatten()
            .or_else(|| self.file_at(&format!("{name}^3"), &c.path).ok().flatten())
            .unwrap_or_default();
        (before, after)
    }
}
