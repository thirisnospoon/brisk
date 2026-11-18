//! Merge conflicts: the three versions of a conflicted file, a line-based three-way merge
//! (PyCharm's "Merge Revisions" dialog works on its chunks), and resolving files.

use crate::{GitError, Repo, Result};

/// A stretch of the merged file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chunk {
    /// The same on all three sides.
    Same(Vec<String>),
    /// Changed on at least one side, relative to the common base.
    Change { base: Vec<String>, ours: Vec<String>, theirs: Vec<String> },
}

impl Chunk {
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
fn lcs_match(a: &[String], b: &[String]) -> Vec<Option<usize>> {
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
    let (b, o, t) = (lines_of(base), lines_of(ours), lines_of(theirs));
    let mo = lcs_match(&b, &o);
    let mt = lcs_match(&b, &t);
    let mut out: Vec<Chunk> = Vec::new();
    let (mut bi, mut oi, mut ti) = (0usize, 0usize, 0usize);
    let push_same = |out: &mut Vec<Chunk>, line: String| match out.last_mut() {
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

/// The stage of a conflicted file in the index (`:1:` base, `:2:` ours, `:3:` theirs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConflictVersions {
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
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
            out.status.success().then(|| String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"))
        };
        Ok(ConflictVersions { base: get(1), ours: get(2), theirs: get(3) })
    }

    /// Accept Yours / Accept Theirs for whole files. A side that deleted the file deletes it.
    pub fn accept(&self, files: &[String], ours: bool) -> Result<()> {
        for f in files {
            let v = self.conflict_versions(f)?;
            let side = if ours { v.ours } else { v.theirs };
            match side {
                Some(text) => {
                    std::fs::write(self.abs(f), text).map_err(|e| GitError(format!("{f}: {e}")))?;
                    self.git(&["add", "--", f])?;
                }
                None => {
                    self.git(&["rm", "--quiet", "--", f])?;
                }
            }
        }
        Ok(())
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
}
