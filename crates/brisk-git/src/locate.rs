//! Finding a repository and a cheap stamp of its refs without starting git. The stamp lets a
//! refresh skip `for-each-ref` and `git log` when no ref moved.

use crate::Repo;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// The git directory of a working tree whose top folder is `top`: `top/.git` when it is a
/// repository folder, or the folder a `.git` file points to (`gitdir: …`, as in worktrees and
/// submodules).
pub(crate) fn git_dir_of(top: &Path) -> Option<PathBuf> {
    let dot = top.join(".git");
    let meta = std::fs::metadata(&dot).ok()?;
    let dir = if meta.is_dir() {
        dot
    } else {
        let text = std::fs::read_to_string(&dot).ok()?;
        let target = text.lines().next()?.strip_prefix("gitdir:")?.trim();
        if target.is_empty() {
            return None;
        }
        top.join(target)
    };
    dir.join("HEAD").is_file().then_some(dir)
}

/// The folder shared by all worktrees (`commondir`), where refs and packed-refs live.
fn common_dir(git_dir: &Path) -> PathBuf {
    std::fs::read_to_string(git_dir.join("commondir"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|s| git_dir.join(s))
        .unwrap_or_else(|| git_dir.to_path_buf())
}

fn stamp_file(p: &Path, h: &mut impl Hasher) {
    match std::fs::metadata(p) {
        Ok(m) => (m.len(), m.modified().ok()).hash(h),
        Err(_) => 0u8.hash(h),
    }
}

/// Names, sizes and times of everything under `dir` (git writes a ref through a lock file and
/// a rename, which changes the folder too).
fn stamp_tree(dir: &Path, h: &mut impl Hasher) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        e.file_name().hash(h);
        let Ok(m) = e.metadata() else { continue };
        (m.len(), m.modified().ok()).hash(h);
        if m.is_dir() {
            stamp_tree(&e.path(), h);
        }
    }
}

impl Repo {
    /// Find the repository containing `path` by looking for `.git` in it and its parents. No
    /// process is started (`git rev-parse` costs 25-60 ms on Windows, too much before the
    /// first frame); [`Repo::discover`] asks git itself, which knows about `GIT_DIR`,
    /// `safe.directory` and the like.
    pub fn find(path: &Path) -> Option<Repo> {
        path.ancestors().find_map(|top| Some(Repo { git_dir: Some(git_dir_of(top)?), root: top.to_path_buf() }))
    }

    /// The same working tree as `other` (git spells the root with '/' and the long name).
    pub fn same_root(&self, other: &Repo) -> bool {
        let norm = |p: &Path| {
            let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
            p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase()
        };
        norm(&self.root) == norm(&other.root)
    }

    /// A hash of HEAD, packed-refs and the loose refs as files on disk: equal stamps mean no
    /// branch, tag or remote branch moved. None when the git directory is unknown.
    pub fn refs_stamp(&self) -> Option<u64> {
        let git_dir = self.git_dir.as_ref()?;
        let common = common_dir(git_dir);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        stamp_file(&git_dir.join("HEAD"), &mut h);
        stamp_file(&common.join("packed-refs"), &mut h);
        stamp_tree(&common.join("refs"), &mut h);
        stamp_tree(&common.join("reftable"), &mut h);
        Some(h.finish())
    }
}
