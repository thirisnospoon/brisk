//! Merge conflicts: the three versions of a conflicted file, a line-based three-way merge
//! (PyCharm's "Merge Revisions" dialog works on its chunks), and resolving files.

use crate::{GitError, Repo, Result};

/// A stretch of the merged file (of lines; of words in [`resolve_simple`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chunk<T = String> {
    /// The same on all three sides.
    Same(Vec<T>),
    /// Changed on at least one side, relative to the common base.
    Change { base: Vec<T>, ours: Vec<T>, theirs: Vec<T> },
}

impl<T: PartialEq> Chunk<T> {
    /// Changed on both sides, differently: needs a decision.
    pub fn is_conflict(&self) -> bool {
        match self {
            Chunk::Same(_) => false,
            Chunk::Change { base, ours, theirs } => ours != base && theirs != base && ours != theirs,
        }
    }
}

/// Lines of a text; a final newline does not make an empty last line.
pub fn lines_of(s: &str) -> Vec<String> {
    let s = s.replace("\r\n", "\n");
    let mut v: Vec<String> = s.split('\n').map(str::to_string).collect();
    if v.last().is_some_and(|l| l.is_empty()) {
        v.pop();
    }
    v
}

/// For each line of `a`, the index of the matching line of `b` (longest common subsequence).
fn lcs_match<T: PartialEq>(a: &[T], b: &[T]) -> Vec<Option<usize>> {
    let (n, m) = (a.len(), b.len());
    // Trim the common prefix and suffix: conflicts are usually small.
    let mut pre = 0;
    while pre < n && pre < m && a[pre] == b[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < n - pre && suf < m - pre && a[n - 1 - suf] == b[m - 1 - suf] {
        suf += 1;
    }
    let (a2, b2) = (&a[pre..n - suf], &b[pre..m - suf]);
    let (n2, m2) = (a2.len(), b2.len());
    let mut out = vec![None; n];
    for i in 0..pre {
        out[i] = Some(i);
    }
    for k in 0..suf {
        out[n - 1 - k] = Some(m - 1 - k);
    }
    if n2 > 0 && m2 > 0 {
        let mut dp = vec![vec![0u32; m2 + 1]; n2 + 1];
        for i in (0..n2).rev() {
            for j in (0..m2).rev() {
                dp[i][j] = if a2[i] == b2[j] { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n2 && j < m2 {
            if a2[i] == b2[j] {
                out[pre + i] = Some(pre + j);
                i += 1;
                j += 1;
            } else if dp[i + 1][j] >= dp[i][j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    out
}

/// Three-way merge of whole lines (diff3): unchanged stretches and changed chunks.
pub fn merge3(base: &str, ours: &str, theirs: &str) -> Vec<Chunk> {
    merge_seq(&lines_of(base), &lines_of(ours), &lines_of(theirs))
}

/// PyCharm's "Resolve Simple Conflicts": a conflict whose sides change different words of
/// it merges word by word. `None` when the changes touch the same words (a real conflict).
pub fn resolve_simple(base: &[String], ours: &[String], theirs: &[String]) -> Option<Vec<String>> {
    let join = |v: &[String]| v.iter().map(|l| format!("{l}\n")).collect::<String>();
    let (b, o, t) = (join(base), join(ours), join(theirs));
    let (tb, to, tt) = (crate::diff::tokens(&b), crate::diff::tokens(&o), crate::diff::tokens(&t));
    // The word match is quadratic: big conflicts stay manual.
    if tb.len().max(to.len()).max(tt.len()) > 4000 {
        return None;
    }
    let mut out = String::new();
    for c in merge_seq(&tb, &to, &tt) {
        match c {
            Chunk::Same(v) => out.extend(v),
            Chunk::Change { base, ours, theirs } => {
                if ours != base && theirs != base && ours != theirs {
                    return None;
                }
                out.extend(if ours == base { theirs } else { ours });
            }
        }
    }
    Some(lines_of(&out))
}

/// Three-way merge of any sequence (diff3): unchanged stretches and changed chunks.
fn merge_seq<T: PartialEq + Clone>(b: &[T], o: &[T], t: &[T]) -> Vec<Chunk<T>> {
    let mo = lcs_match(b, o);
    let mt = lcs_match(b, t);
    let mut out: Vec<Chunk<T>> = Vec::new();
    let (mut bi, mut oi, mut ti) = (0usize, 0usize, 0usize);
    let push_same = |out: &mut Vec<Chunk<T>>, line: T| match out.last_mut() {
        Some(Chunk::Same(v)) => v.push(line),
        _ => out.push(Chunk::Same(vec![line])),
    };
    loop {
        // A base line kept at the current position on both sides is stable.
        if bi < b.len() && mo[bi] == Some(oi) && mt[bi] == Some(ti) {
            push_same(&mut out, b[bi].clone());
            bi += 1;
            oi += 1;
            ti += 1;
            continue;
        }
        if bi >= b.len() && oi >= o.len() && ti >= t.len() {
            break;
        }
        // Find the next base line that is stable on both sides (in order).
        let mut end = bi;
        while end < b.len() {
            if let (Some(x), Some(y)) = (mo[end], mt[end]) {
                if x >= oi && y >= ti {
                    break;
                }
            }
            end += 1;
        }
        let (oe, te) = if end < b.len() { (mo[end].unwrap_or(o.len()), mt[end].unwrap_or(t.len())) } else { (o.len(), t.len()) };
        let chunk = Chunk::Change { base: b[bi..end].to_vec(), ours: o[oi..oe].to_vec(), theirs: t[ti..te].to_vec() };
        if let Chunk::Change { base, ours, theirs } = &chunk {
            if base == ours && base == theirs {
                for l in base.clone() {
                    push_same(&mut out, l);
                }
            } else {
                out.push(chunk);
            }
        }
        bi = end;
        oi = oe;
        ti = te;
    }
    out
}

/// The stage of a conflicted file in the index (`:1:` base, `:2:` ours, `:3:` theirs), with
/// `\n` line endings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConflictVersions {
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
    /// The file uses `\r\n`: the merged text is written back with it.
    pub crlf: bool,
}

/// Mostly `\r\n` line endings? None when the text has no line break to tell.
fn uses_crlf(text: &str) -> Option<bool> {
    let lf = text.matches('\n').count();
    (lf > 0).then(|| text.matches("\r\n").count() * 2 > lf)
}

impl Repo {
    /// Paths with unresolved conflicts.
    pub fn conflicts(&self) -> Result<Vec<String>> {
        let out = self.git(&["diff", "--name-only", "--diff-filter=U", "-z"])?;
        Ok(out.split('\0').filter(|s| !s.is_empty()).map(str::to_string).collect())
    }

    pub fn conflict_versions(&self, rel: &str) -> Result<ConflictVersions> {
        let get = |n: u8| -> Option<String> {
            let spec = format!(":{n}:{rel}");
            let out = self.run(&["show", &spec]).ok()?;
            out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
        };
        let (base, ours, theirs) = (get(1), get(2), get(3));
        // The file on disk first: git wrote it with the checkout's line endings (autocrlf
        // keeps `\n` in the stages); then the sides.
        let disk = std::fs::read(self.abs(rel)).ok().map(|b| String::from_utf8_lossy(&b).into_owned());
        let crlf = [&disk, &ours, &theirs].into_iter().flatten().find_map(|t| uses_crlf(t)).unwrap_or(false);
        let lf = |t: Option<String>| t.map(|t| t.replace("\r\n", "\n"));
        Ok(ConflictVersions { base: lf(base), ours: lf(ours), theirs: lf(theirs), crlf })
    }

    /// Accept Yours / Accept Theirs for whole files. A side that deleted the file deletes it.
    /// git checks the side out itself, so binary files and line endings stay byte-exact.
    pub fn accept(&self, files: &[String], ours: bool) -> Result<()> {
        let stage = if ours { "2" } else { "3" };
        for f in files {
            // `<mode> <object> <stage>\t<path>` per stage present; a deleted side has none.
            let ls = self.git(&["ls-files", "-u", "-z", "--", f])?;
            let present = ls.split('\0').any(|rec| rec.split('\t').next().and_then(|h| h.split(' ').nth(2)) == Some(stage));
            if present {
                self.git(&["checkout", if ours { "--ours" } else { "--theirs" }, "--", f])?;
                self.git(&["add", "--", f])?;
            } else {
                self.git(&["rm", "--quiet", "--", f])?;
            }
        }
        Ok(())
    }

    /// The branch being merged in, for the merge window's "Changes from <branch>".
    pub fn merge_source(&self) -> Option<String> {
        let out = self.git(&["name-rev", "--name-only", "--exclude=tags/*", "MERGE_HEAD"]).ok()?;
        let n = out.trim();
        (!n.is_empty() && n != "undefined").then(|| n.to_string())
    }

    /// Merge…'s Apply: write the merged text and mark the file resolved.
    pub fn resolve_with(&self, rel: &str, text: &str) -> Result<()> {
        std::fs::write(self.abs(rel), text).map_err(|e| GitError(format!("{rel}: {e}")))?;
        self.git(&["add", "--", rel]).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn merge_chunks() {
        let base = "a\nb\nc\nd\ne\n";
        // Ours changes b, theirs changes d: two separate non-conflicting chunks.
        let m = merge3(base, "a\nB\nc\nd\ne\n", "a\nb\nc\nD\ne\n");
        assert_eq!(
            m,
            vec![
                Chunk::Same(s(&["a"])),
                Chunk::Change { base: s(&["b"]), ours: s(&["B"]), theirs: s(&["b"]) },
                Chunk::Same(s(&["c"])),
                Chunk::Change { base: s(&["d"]), ours: s(&["d"]), theirs: s(&["D"]) },
                Chunk::Same(s(&["e"])),
            ]
        );
        assert!(!m.iter().any(Chunk::is_conflict));
        // Both change c differently: a conflict.
        let m = merge3(base, "a\nb\nC1\nd\ne\n", "a\nb\nC2\nd\ne\n");
        assert_eq!(m.iter().filter(|c| c.is_conflict()).count(), 1);
        assert_eq!(m[1], Chunk::Change { base: s(&["c"]), ours: s(&["C1"]), theirs: s(&["C2"]) });
        // Insertions at the end on both sides.
        let m = merge3("x\n", "x\ny\n", "x\nz\n");
        assert_eq!(m, vec![Chunk::Same(s(&["x"])), Chunk::Change { base: vec![], ours: s(&["y"]), theirs: s(&["z"]) }]);
        // Identical changes are not a conflict.
        let m = merge3("x\n", "y\n", "y\n");
        assert!(!m[0].is_conflict());
    }

    #[test]
    fn line_endings() {
        assert_eq!(uses_crlf("a\r\nb\r\n"), Some(true));
        assert_eq!(uses_crlf("a\nb\n"), Some(false));
        assert_eq!(uses_crlf("a\r\nb\nc\r\n"), Some(true));
        assert_eq!(uses_crlf("no break"), None);
    }

    #[test]
    fn simple_conflicts() {
        // Different words of the same line: merges.
        let r = resolve_simple(&s(&["x = foo(bar, 1)"]), &s(&["x = foo(baz, 1)"]), &s(&["x = foo(bar, 2)"]));
        assert_eq!(r, Some(s(&["x = foo(baz, 2)"])));
        // The same word, changed differently: stays a conflict.
        assert_eq!(resolve_simple(&s(&["a = 1"]), &s(&["a = 2"]), &s(&["a = 3"])), None);
        // A line added on each side around an unchanged one.
        let r = resolve_simple(&s(&["mid"]), &s(&["top", "mid"]), &s(&["mid", "end"]));
        assert_eq!(r, Some(s(&["top", "mid", "end"])));
    }
}
