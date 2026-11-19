//! `git blame` (Annotate with Git Blame) and a file's history.

use crate::{git_cmd, Commit, GitError, Repo, Result};
use std::collections::HashMap;
use std::io::Write;
use std::process::Stdio;

/// The commit that last touched one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameLine {
    /// Full hash; all zeros for lines not committed yet.
    pub hash: String,
    pub author: String,
    pub email: String,
    /// Author time, Unix seconds.
    pub time: i64,
    pub summary: String,
}

impl BlameLine {
    pub fn is_uncommitted(&self) -> bool {
        self.hash.bytes().all(|b| b == b'0')
    }
}

/// Parse `git blame --porcelain`: one entry per line of the file.
pub fn parse_blame(out: &str) -> Vec<BlameLine> {
    #[derive(Default, Clone)]
    struct Info {
        author: String,
        email: String,
        time: i64,
        summary: String,
    }
    let mut infos: HashMap<String, Info> = HashMap::new();
    let mut lines = Vec::new();
    let mut cur: Option<String> = None;
    for l in out.split('\n') {
        if let Some(_text) = l.strip_prefix('\t') {
            if let Some(h) = cur.take() {
                let i = infos.get(&h).cloned().unwrap_or_default();
                lines.push(BlameLine { hash: h, author: i.author, email: i.email, time: i.time, summary: i.summary });
            }
            continue;
        }
        let mut parts = l.splitn(2, ' ');
        let key = parts.next().unwrap_or("");
        let val = parts.next().unwrap_or("");
        if key.len() == 40 && key.bytes().all(|b| b.is_ascii_hexdigit()) {
            infos.entry(key.to_string()).or_default();
            cur = Some(key.to_string());
            continue;
        }
        let Some(h) = cur.as_ref() else { continue };
        let i = infos.entry(h.clone()).or_default();
        match key {
            "author" => i.author = val.to_string(),
            "author-mail" => i.email = val.trim_matches(['<', '>']).to_string(),
            "author-time" => i.time = val.trim().parse().unwrap_or(0),
            "summary" => i.summary = val.to_string(),
            _ => {}
        }
    }
    lines
}

impl Repo {
    /// Blame `rel`; `contents` blames the editor's text instead of the file on disk, so
    /// unsaved lines show as not committed.
    pub fn blame(&self, rel: &str, contents: Option<&str>) -> Result<Vec<BlameLine>> {
        let mut c = git_cmd();
        c.arg("-C").arg(self.root()).args(["blame", "--porcelain"]);
        if contents.is_some() {
            c.args(["--contents", "-"]);
        }
        c.args(["--", rel]).stdout(Stdio::piped()).stderr(Stdio::piped());
        if contents.is_some() {
            c.stdin(Stdio::piped());
        }
        let mut child = c.spawn()?;
        brisk_proc::adopt(&child);
        if let (Some(text), Some(mut stdin)) = (contents, child.stdin.take()) {
            // A separate thread: a big file may fill the pipe before git starts reading.
            let text = text.to_string();
            std::thread::spawn(move || {
                let _ = stdin.write_all(text.as_bytes());
            });
        }
        let out = child.wait_with_output()?;
        if !out.status.success() {
            return Err(GitError(String::from_utf8_lossy(&out.stderr).trim().to_string()));
        }
        Ok(parse_blame(&String::from_utf8_lossy(&out.stdout)))
    }

    /// Commits that changed `rel` (following renames), newest first: Show History.
    pub fn file_log(&self, rel: &str, limit: usize) -> Result<Vec<Commit>> {
        let n = format!("-n{limit}");
        let out = self.git(&["log", "--follow", &n, "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%s%x1e", "--", rel])?;
        Ok(crate::parse_log(&out))
    }

    /// Blame of `rel` as of revision `rev` (Annotate Previous Revision).
    pub fn blame_at(&self, rev: &str, rel: &str) -> Result<Vec<BlameLine>> {
        Ok(parse_blame(&self.git(&["blame", "--porcelain", rev, "--", rel])?))
    }

    /// The revision before `hash` for a file: (parent commit, the file's path there). `rel`
    /// is the file's path at `from` (HEAD when None). None when `hash` added the file.
    pub fn previous_revision(&self, hash: &str, rel: &str, from: Option<&str>) -> Option<(String, String)> {
        let path = self.path_at(from, hash, rel);
        let change = self.commit_files(hash).ok()?.into_iter().find(|c| c.path == path);
        if change.as_ref().is_some_and(|c| c.status == crate::FileStatus::Added) {
            return None;
        }
        let prev = change.and_then(|c| c.old_path).unwrap_or(path);
        let parent = self.git(&["rev-parse", "--verify", "-q", &format!("{hash}^")]).ok()?.trim().to_string();
        (!parent.is_empty()).then_some((parent, prev))
    }

    /// The path `rel` had in `hash` (it may have been renamed since), for Show History.
    pub fn path_in_commit(&self, hash: &str, rel: &str) -> String {
        self.path_at(None, hash, rel)
    }

    /// The same for `rel` as named at revision `from`.
    pub fn path_at(&self, from: Option<&str>, hash: &str, rel: &str) -> String {
        let mut args = vec!["log", "--follow", "--format=%H", "--name-only"];
        args.extend(from);
        args.extend(["--", rel]);
        let out = self.git(&args).unwrap_or_default();
        let mut lines = out.lines().filter(|l| !l.trim().is_empty());
        while let Some(h) = lines.next() {
            let path = lines.next().unwrap_or(rel);
            if h.trim() == hash {
                return path.trim().to_string();
            }
        }
        rel.to_string()
    }
    /// The patch `hash` made to `path` (the path in that commit), for the history preview.
    pub fn file_patch(&self, hash: &str, path: &str) -> Result<String> {
        self.git(&["show", "--format=", "--no-color", "-M", hash, "--", path])
    }

    /// Show History for Selection: commits that touched lines `start..=end` (1-based) of
    /// `rel`, newest first, each with the patch of just those lines (`git log -L`).
    pub fn line_log(&self, rel: &str, start: usize, end: usize, limit: usize) -> Result<Vec<(Commit, String)>> {
        let range = format!("-L{start},{end}:{rel}");
        let n = format!("-n{limit}");
        let out = self.git(&["log", "--no-color", &n, &range, "--format=%x1e%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%s%x1d"])?;
        Ok(out
            .split('\x1e')
            .filter_map(|rec| {
                let (head, patch) = rec.split_once('\x1d')?;
                let c = crate::parse_log(&format!("{head}\x1e")).pop()?;
                Some((c, patch.trim_start_matches('\n').to_string()))
            })
            .collect())
    }
}
