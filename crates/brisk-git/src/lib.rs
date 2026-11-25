//! Git integration via the `git` CLI (porcelain formats). UI-free.
//!
//! Using the CLI keeps us in sync with whatever git the user has (credentials helpers, hooks,
//! LFS, config) — the same approach PyCharm takes.

mod blame;
mod branch_ops;
mod diff;
mod graph;
mod log_query;
mod log_ops;
mod merge;
mod partial;
mod remote;
mod stash;

pub use blame::{parse_blame, BlameLine};
pub use diff::{align, foldable, inline_diff, line_changes, line_ops, unified, AlignedRow, LineChange, LineChangeKind, RowKind, UnifiedRow};
pub use stash::StashEntry;
pub use log_ops::ResetMode;
pub use partial::{compose, hunk_key, hunks};
pub use remote::Outgoing;
pub use log_query::LogQuery;
pub use merge::{lines_of, merge3, resolve_simple, Chunk, ConflictVersions};
pub use graph::{layout, GraphRow, Segment};

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflict,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// Path relative to the repo root, '/' separated.
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    /// HEAD commit (None before the first commit).
    pub head: Option<String>,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<FileChange>,
}

impl Status {
    pub fn tracked(&self) -> impl Iterator<Item = &FileChange> {
        self.changes.iter().filter(|c| c.status != FileStatus::Untracked)
    }
    pub fn untracked(&self) -> impl Iterator<Item = &FileChange> {
        self.changes.iter().filter(|c| c.status == FileStatus::Untracked)
    }
    pub fn by_path(&self) -> HashMap<String, FileStatus> {
        self.changes.iter().map(|c| (c.path.clone(), c.status)).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub remote: bool,
    pub current: bool,
    pub upstream: Option<String>,
    pub hash: String,
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub refs: Vec<String>,
    pub subject: String,
}

impl Commit {
    pub fn short(&self) -> &str {
        &self.hash[..self.hash.len().min(8)]
    }
}

#[derive(Debug)]
pub struct GitError(pub String);

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for GitError {}

impl From<io::Error> for GitError {
    fn from(e: io::Error) -> Self {
        GitError(format!("cannot run git: {e}"))
    }
}

pub type Result<T> = std::result::Result<T, GitError>;

#[derive(Clone, Debug)]
pub struct Repo {
    root: PathBuf,
}

fn git_cmd() -> Command {
    let mut c = Command::new("git");
    c.env("GIT_OPTIONAL_LOCKS", "0") // never fight the user's own git for index.lock
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0") // no hanging on credential prompts
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

impl Repo {
    /// Find the repository containing `path`.
    pub fn discover(path: &Path) -> Option<Repo> {
        let out = git_cmd().arg("-C").arg(path).args(["rev-parse", "--show-toplevel"]).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Some(Repo { root: PathBuf::from(root) })
    }

    /// "Create Git Repository" (VCS → Enable Version Control Integration).
    pub fn init(path: &Path) -> Result<Repo> {
        let out = git_cmd().arg("-C").arg(path).args(["init", "-q"]).output()?;
        if !out.status.success() {
            return Err(GitError(String::from_utf8_lossy(&out.stderr).trim().to_string()));
        }
        Repo::discover(path).ok_or_else(|| GitError("git init succeeded but the repository was not found".into()))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path relative to the repo root with '/' separators.
    pub fn rel(&self, abs: &Path) -> Option<String> {
        let abs = abs.to_string_lossy().replace('\\', "/");
        let root = self.root.to_string_lossy().replace('\\', "/");
        let root = root.trim_end_matches('/');
        // Byte-wise compare (no slicing inside a multi-byte character), and only at a path
        // separator: `C:/proj` is not a prefix of `C:/proj2/x`.
        let (head, rest) = (abs.as_bytes().get(..root.len())?, abs.get(root.len()..)?);
        if !head.eq_ignore_ascii_case(root.as_bytes()) || !rest.starts_with('/') {
            return None;
        }
        let rest = rest.trim_start_matches('/');
        (!rest.is_empty()).then(|| rest.to_string())
    }

    pub fn abs(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn run(&self, args: &[&str]) -> Result<Output> {
        Ok(git_cmd().arg("-C").arg(&self.root).args(args).output()?)
    }

    fn git(&self, args: &[&str]) -> Result<String> {
        let out = self.run(args)?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let err = if err.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { err };
            return Err(GitError(err));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn status(&self) -> Result<Status> {
        let out = self.git(&["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"])?;
        Ok(parse_status(&out))
    }

    pub fn branches(&self) -> Result<Vec<Branch>> {
        let out = self.git(&[
            "for-each-ref",
            "--format=%(refname)%1f%(HEAD)%1f%(upstream:short)%1f%(objectname)",
            "refs/heads",
            "refs/remotes",
        ])?;
        let mut v: Vec<Branch> = out
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\x1f').collect();
                if f.len() < 4 {
                    return None;
                }
                let (remote, name) = if let Some(n) = f[0].strip_prefix("refs/heads/") {
                    (false, n)
                } else {
                    (true, f[0].strip_prefix("refs/remotes/")?)
                };
                if remote && name.ends_with("/HEAD") {
                    return None;
                }
                Some(Branch {
                    name: name.to_string(),
                    remote,
                    current: f[1] == "*",
                    upstream: (!f[2].is_empty()).then(|| f[2].to_string()),
                    hash: f[3].to_string(),
                })
            })
            .collect();
        v.sort_by(|a, b| a.remote.cmp(&b.remote).then(b.current.cmp(&a.current)).then(a.name.cmp(&b.name)));
        Ok(v)
    }

    /// Commits of all branches, newest first (like PyCharm's Log tab).
    pub fn log(&self, limit: usize) -> Result<Vec<Commit>> {
        let n = format!("-n{limit}");
        let out = match self.git(&[
            "log",
            "--date-order",
            "--branches",
            "--remotes",
            "--tags",
            "HEAD",
            &n,
            LOG_FORMAT,
        ]) {
            Ok(o) => o,
            Err(e) if e.0.contains("does not have any commits") || e.0.contains("unknown revision") => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        Ok(parse_log(&out))
    }

    pub fn commit_message(&self, hash: &str) -> Result<String> {
        self.git(&["show", "-s", "--format=%B", hash]).map(|s| s.trim_end().to_string())
    }
}

/// The `git log` format [`parse_log`] reads.
pub(crate) const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%s%x1e";

/// Records of `git log` with [`LOG_FORMAT`].
fn parse_log(out: &str) -> Vec<Commit> {
    out.split('\x1e')
        .filter_map(|rec| {
            let rec = rec.trim_start_matches(['\n', '\r']);
            let f: Vec<&str> = rec.split('\x1f').collect();
            if f.len() < 7 {
                return None;
            }
            Some(Commit {
                hash: f[0].to_string(),
                parents: f[1].split_whitespace().map(str::to_string).collect(),
                author: f[2].to_string(),
                email: f[3].to_string(),
                time: f[4].parse().unwrap_or(0),
                refs: f[5].split(", ").filter(|s| !s.is_empty()).map(str::to_string).collect(),
                subject: f[6].to_string(),
            })
        })
        .collect()
}

impl Repo {
    /// Files changed by a commit (vs its first parent).
    pub fn commit_files(&self, hash: &str) -> Result<Vec<FileChange>> {
        let out = self.git(&["show", "--format=", "--name-status", "-z", "--find-renames", "--first-parent", "-m", hash])?;
        let mut parts = out.split('\0').filter(|s| !s.is_empty());
        let mut v = Vec::new();
        while let Some(code) = parts.next() {
            let code = code.trim();
            let status = match code.chars().next() {
                Some('A') => FileStatus::Added,
                Some('D') => FileStatus::Deleted,
                Some('R') | Some('C') => FileStatus::Renamed,
                Some('U') => FileStatus::Conflict,
                Some(_) => FileStatus::Modified,
                None => continue,
            };
            if status == FileStatus::Renamed {
                let old = parts.next().unwrap_or_default().to_string();
                let new = parts.next().unwrap_or_default().to_string();
                v.push(FileChange { path: new, old_path: Some(old), status });
            } else if let Some(p) = parts.next() {
                v.push(FileChange { path: p.to_string(), old_path: None, status });
            }
        }
        Ok(v)
    }

    /// File contents at a revision ("HEAD", a hash, "hash^"). Missing file → None.
    pub fn file_at(&self, rev: &str, rel: &str) -> Result<Option<String>> {
        let spec = format!("{rev}:{rel}");
        let out = self.run(&["show", &spec])?;
        if !out.status.success() {
            return Ok(None);
        }
        Ok(Some(String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n")))
    }

    /// Commit exactly `files` (PyCharm semantics: other staged changes stay out).
    pub fn commit(&self, message: &str, files: &[String], amend: bool, untracked: &[String]) -> Result<String> {
        if message.trim().is_empty() && !amend {
            return Err(GitError("Commit message is empty".into()));
        }
        if files.is_empty() && !amend {
            return Err(GitError("No files selected".into()));
        }
        if !untracked.is_empty() {
            let mut args = vec!["add", "--"];
            args.extend(untracked.iter().map(String::as_str));
            self.git(&args)?;
        }
        // During a merge, cherry-pick or revert git refuses a commit of some paths: stage
        // them and commit the index (the operation's own changes are staged already).
        if !files.is_empty() && !amend && self.pending_operation().is_some() {
            let mut args = vec!["add", "-A", "--"];
            args.extend(files.iter().map(String::as_str));
            self.git(&args)?;
            self.git(&["commit", "-m", message])?;
            return self.git(&["rev-parse", "--short", "HEAD"]).map(|s| s.trim().to_string());
        }
        let mut args = vec!["commit", "-m", message];
        if amend {
            args.push("--amend");
            if message.trim().is_empty() {
                args.retain(|a| *a != "-m" && *a != message);
                args.push("--no-edit");
            }
            // Amend with no files only rewords: `--only` keeps other staged changes out.
            if files.is_empty() {
                args.push("--only");
            }
        }
        if !files.is_empty() {
            args.push("--");
            args.extend(files.iter().map(String::as_str));
        }
        self.git(&args)?;
        self.git(&["rev-parse", "--short", "HEAD"]).map(|s| s.trim().to_string())
    }

    /// Ctrl+Alt+Z: discard changes. Tracked files are restored from HEAD; added files are unstaged
    /// and kept on disk (like PyCharm's Rollback); untracked files are left alone.
    pub fn rollback(&self, changes: &[FileChange]) -> Result<()> {
        // A rename is undone by restoring both names: the new one goes (it is not in HEAD),
        // the old one comes back.
        let restore: Vec<&str> = changes
            .iter()
            .filter(|c| matches!(c.status, FileStatus::Modified | FileStatus::Deleted | FileStatus::Renamed | FileStatus::Conflict))
            .flat_map(|c| std::iter::once(c.path.as_str()).chain(c.old_path.as_deref().filter(|_| c.status == FileStatus::Renamed)))
            .collect();
        if !restore.is_empty() {
            let mut args = vec!["restore", "--staged", "--worktree", "--source=HEAD", "--"];
            args.extend(restore);
            self.git(&args)?;
        }
        let added: Vec<&str> = changes.iter().filter(|c| c.status == FileStatus::Added).map(|c| c.path.as_str()).collect();
        if !added.is_empty() {
            let mut args = vec!["rm", "--cached", "--quiet", "--"];
            args.extend(added);
            self.git(&args)?;
        }
        Ok(())
    }

    pub fn checkout(&self, branch: &Branch) -> Result<()> {
        if branch.remote {
            // origin/feature → local "feature" tracking it (PyCharm "Checkout" on a remote branch).
            let local = branch.name.split_once('/').map(|(_, n)| n).unwrap_or(&branch.name);
            if self.run(&["rev-parse", "--verify", "--quiet", &format!("refs/heads/{local}")])?.status.success() {
                self.git(&["switch", local])?;
            } else {
                self.git(&["switch", "--track", "-c", local, &branch.name])?;
            }
        } else {
            self.git(&["switch", &branch.name])?;
        }
        Ok(())
    }

    pub fn create_branch(&self, name: &str) -> Result<()> {
        self.git(&["switch", "-c", name]).map(|_| ())
    }

    pub fn fetch(&self) -> Result<String> {
        self.git(&["fetch", "--all", "--prune"])
    }

    /// Ctrl+T "Update Project": pull with merge (PyCharm's default).
    pub fn pull(&self) -> Result<String> {
        self.git(&["pull", "--no-rebase", "--no-edit"])
    }

    /// Ctrl+Shift+K: push the current branch; sets upstream on first push.
    pub fn push(&self) -> Result<String> {
        let st = self.status()?;
        let branch = st.branch.ok_or_else(|| GitError("Detached HEAD — nothing to push".into()))?;
        if st.upstream.is_some() {
            self.git(&["push"])
        } else {
            self.git(&["push", "--set-upstream", "origin", &branch])
        }
    }
}

fn parse_status(out: &str) -> Status {
    let mut st = Status::default();
    let mut it = out.split('\0').filter(|s| !s.is_empty());
    while let Some(e) = it.next() {
        if let Some(h) = e.strip_prefix("# ") {
            if let Some(o) = h.strip_prefix("branch.oid ") {
                st.head = (o != "(initial)").then(|| o.to_string());
            } else if let Some(b) = h.strip_prefix("branch.head ") {
                st.branch = (b != "(detached)").then(|| b.to_string());
            } else if let Some(u) = h.strip_prefix("branch.upstream ") {
                st.upstream = Some(u.to_string());
            } else if let Some(ab) = h.strip_prefix("branch.ab ") {
                for p in ab.split_whitespace() {
                    if let Some(a) = p.strip_prefix('+') {
                        st.ahead = a.parse().unwrap_or(0);
                    } else if let Some(b) = p.strip_prefix('-') {
                        st.behind = b.parse().unwrap_or(0);
                    }
                }
            }
            continue;
        }
        let kind = e.as_bytes()[0];
        match kind {
            b'?' => st.changes.push(FileChange { path: e[2..].to_string(), old_path: None, status: FileStatus::Untracked }),
            b'1' | b'2' | b'u' => {
                let fields = match kind {
                    b'1' => 8,
                    b'2' => 9,
                    _ => 10,
                };
                let parts: Vec<&str> = e.splitn(fields + 1, ' ').collect();
                let xy = parts.get(1).copied().unwrap_or("..").as_bytes();
                let path = parts.get(fields).copied().unwrap_or_default().to_string();
                let status = if kind == b'u' {
                    FileStatus::Conflict
                } else if kind == b'2' {
                    FileStatus::Renamed
                } else if xy.contains(&b'A') {
                    FileStatus::Added
                } else if xy.contains(&b'D') {
                    FileStatus::Deleted
                } else {
                    FileStatus::Modified
                };
                let old_path = (kind == b'2').then(|| it.next().unwrap_or_default().to_string());
                st.changes.push(FileChange { path, old_path, status });
            }
            _ => {}
        }
    }
    st.changes.sort_by(|a, b| a.path.cmp(&b.path));
    st
}

#[cfg(test)]
mod tests;
