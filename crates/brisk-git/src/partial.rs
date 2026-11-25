//! Partial commit by hunks (PyCharm: the checkboxes on the changes of a diff in the Commit
//! window). The commit is built in a temporary index: HEAD, plus the whole files, plus each
//! partial file as HEAD with just its included hunks applied. The working tree is not
//! touched, so the excluded hunks stay there as local changes.

use crate::{git_cmd, GitError, Repo, Result};
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::ops::Range;
use std::path::PathBuf;
use std::process::Stdio;

/// The changed stretches of `current` against `base` as (base lines, current lines), with
/// touching changes merged (one block of the diff window).
pub fn hunks(base: &str, current: &str) -> Vec<(Range<usize>, Range<usize>)> {
    let mut out: Vec<(Range<usize>, Range<usize>)> = Vec::new();
    for c in crate::diff::line_changes(base, current) {
        match out.last_mut() {
            Some((b, n)) if b.end == c.base.start && n.end == c.lines.start => {
                b.end = c.base.end;
                n.end = c.lines.end;
            }
            _ => out.push((c.base, c.lines)),
        }
    }
    out
}

/// A hunk's identity while the file is edited elsewhere: its base lines and its new text.
pub fn hunk_key<'a>(base: &Range<usize>, lines: impl IntoIterator<Item = &'a str>) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for l in lines {
        l.trim_end_matches(['\n', '\r']).hash(&mut h);
    }
    format!("{}:{}:{:x}", base.start, base.end, h.finish())
}

/// `base` with the hunks of `current` applied, except the `excluded` ones. Line endings are
/// kept as they are in each side.
pub fn compose(base: &str, current: &str, excluded: &HashSet<String>) -> String {
    let (b, c) = (base.replace("\r\n", "\n"), current.replace("\r\n", "\n"));
    let bl: Vec<&str> = base.split_inclusive('\n').collect();
    let cl: Vec<&str> = current.split_inclusive('\n').collect();
    let mut out = String::with_capacity(current.len());
    let mut at = 0;
    for (br, nr) in hunks(&b, &c) {
        out.extend(bl[at.min(bl.len())..br.start.min(bl.len())].iter().copied());
        let key = hunk_key(&br, cl.get(nr.clone()).unwrap_or(&[]).iter().copied());
        if excluded.contains(&key) {
            out.extend(bl.get(br.clone()).unwrap_or(&[]).iter().copied());
        } else {
            out.extend(cl.get(nr).unwrap_or(&[]).iter().copied());
        }
        at = br.end;
    }
    out.extend(bl.get(at..).unwrap_or(&[]).iter().copied());
    out
}

impl Repo {
    /// git with its own index file (and optional stdin).
    fn git_index(&self, index: &std::path::Path, args: &[&str], stdin: Option<&[u8]>) -> Result<String> {
        let mut c = git_cmd();
        c.arg("-C").arg(self.root()).args(args).env("GIT_INDEX_FILE", index).stdout(Stdio::piped()).stderr(Stdio::piped());
        if stdin.is_some() {
            c.stdin(Stdio::piped());
        }
        let mut child = c.spawn()?;
        brisk_platform::process::adopt(&child);
        if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
            let data = data.to_vec();
            std::thread::spawn(move || {
                let _ = pipe.write_all(&data);
            });
        }
        let out = child.wait_with_output()?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let err = if err.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { err };
            return Err(GitError(err));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// The bytes of `rel` at `rev` as stored (no line-ending change).
    fn blob_raw(&self, rev: &str, rel: &str) -> Option<String> {
        let out = self.run(&["show", &format!("{rev}:{rel}")]).ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Commit `files` whole and the `partial` files with only their included hunks (the
    /// keys in each set are excluded). Other staged changes stay out, as in `commit`.
    pub fn commit_partial(&self, message: &str, files: &[String], partial: &[(String, HashSet<String>)], amend: bool) -> Result<String> {
        if message.trim().is_empty() && !amend {
            return Err(GitError("Commit message is empty".into()));
        }
        let git_dir = PathBuf::from(self.git(&["rev-parse", "--absolute-git-dir"])?.trim());
        let index = git_dir.join("brisk-partial-index");
        let res = (|| {
            let has_head = self.run(&["rev-parse", "--verify", "-q", "HEAD"]).is_ok_and(|o| o.status.success());
            if has_head {
                self.git_index(&index, &["read-tree", "HEAD"], None)?;
            } else {
                self.git_index(&index, &["read-tree", "--empty"], None)?;
            }
            if !files.is_empty() {
                let mut args = vec!["add", "-A", "--"];
                args.extend(files.iter().map(String::as_str));
                self.git_index(&index, &args, None)?;
            }
            for (rel, excluded) in partial {
                let base = self.blob_raw("HEAD", rel).unwrap_or_default();
                let disk = std::fs::read(self.abs(rel)).map_err(|e| GitError(format!("{rel}: {e}")))?;
                let text = compose(&base, &String::from_utf8_lossy(&disk), excluded);
                let sha = self.git_index(&index, &["hash-object", "-w", "--stdin", "--path", rel], Some(text.as_bytes()))?;
                let staged = self.git_index(&index, &["ls-files", "-s", "--", rel], None)?;
                let mode = staged.split_whitespace().next().filter(|m| m.len() == 6).unwrap_or("100644").to_string();
                let info = format!("{mode},{},{rel}", sha.trim());
                self.git_index(&index, &["update-index", "--add", "--cacheinfo", &info], None)?;
            }
            let mut args = vec!["commit", "-m", message];
            if amend {
                args.push("--amend");
                if message.trim().is_empty() {
                    args = vec!["commit", "--amend", "--no-edit"];
                }
            }
            self.git_index(&index, &args, None)
        })();
        let _ = std::fs::remove_file(&index);
        res?;
        // The real index: the committed paths as they are in the new HEAD.
        let mut args = vec!["reset", "-q", "--"];
        args.extend(files.iter().map(String::as_str));
        args.extend(partial.iter().map(|p| p.0.as_str()));
        let _ = self.git(&args);
        self.git(&["rev-parse", "--short", "HEAD"]).map(|s| s.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_keeps_excluded_hunks_out() {
        let base = "a\nb\nc\nd\ne\n";
        let cur = "a\nB\nc\nd\nE\nf\n";
        let h = hunks(base, cur);
        assert_eq!(h, vec![(1..2, 1..2), (4..5, 4..6)]);
        let cl: Vec<&str> = cur.split_inclusive('\n').collect();
        let k2 = hunk_key(&h[1].0, cl[h[1].1.clone()].iter().copied());
        assert_eq!(compose(base, cur, &HashSet::new()), cur);
        assert_eq!(compose(base, cur, &HashSet::from([k2.clone()])), "a\nB\nc\nd\ne\n");
        let k1 = hunk_key(&h[0].0, ["B"]);
        assert_eq!(compose(base, cur, &HashSet::from([k1, k2])), base);
        // CRLF on disk: the included lines keep it, the base lines keep theirs.
        assert_eq!(compose(base, "a\r\nB\r\nc\r\nd\r\nE\r\nf\r\n", &HashSet::from([hunk_key(&(4..5), ["E", "f"])])), "a\nB\r\nc\nd\ne\n");
    }
}
