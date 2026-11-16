//! Line diffs: gutter change markers (editor vs HEAD) and aligned rows for the diff viewer.

use similar::{DiffTag, TextDiff};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineChangeKind {
    Added,
    Modified,
    /// Lines were removed just above `lines.start` (lines is empty).
    Deleted,
}

/// A change in the *current* text, in 0-based line numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineChange {
    pub kind: LineChangeKind,
    pub lines: Range<usize>,
}

/// Changes of `current` relative to `base`, for the editor gutter.
pub fn line_changes(base: &str, current: &str) -> Vec<LineChange> {
    let diff = TextDiff::configure().timeout(std::time::Duration::from_millis(200)).diff_lines(base, current);
    let mut out = Vec::new();
    for op in diff.ops() {
        let (tag, old, new) = op.as_tag_tuple();
        match tag {
            DiffTag::Equal => {}
            DiffTag::Insert => out.push(LineChange { kind: LineChangeKind::Added, lines: new }),
            DiffTag::Delete => out.push(LineChange { kind: LineChangeKind::Deleted, lines: new.start..new.start }),
            DiffTag::Replace => {
                let _ = old;
                out.push(LineChange { kind: LineChangeKind::Modified, lines: new });
            }
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Equal,
    Added,
    Deleted,
    Modified,
}

/// One row of a side-by-side diff. Line numbers are 0-based; `None` = filler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlignedRow {
    pub left: Option<usize>,
    pub right: Option<usize>,
    pub kind: RowKind,
}

pub fn align(left: &str, right: &str) -> Vec<AlignedRow> {
    let diff = TextDiff::configure().timeout(std::time::Duration::from_millis(500)).diff_lines(left, right);
    let mut rows = Vec::new();
    for op in diff.ops() {
        let (tag, old, new) = op.as_tag_tuple();
        match tag {
            DiffTag::Equal => {
                for (l, r) in old.zip(new) {
                    rows.push(AlignedRow { left: Some(l), right: Some(r), kind: RowKind::Equal });
                }
            }
            DiffTag::Delete => rows.extend(old.map(|l| AlignedRow { left: Some(l), right: None, kind: RowKind::Deleted })),
            DiffTag::Insert => rows.extend(new.map(|r| AlignedRow { left: None, right: Some(r), kind: RowKind::Added })),
            DiffTag::Replace => {
                let n = old.len().max(new.len());
                for i in 0..n {
                    let l = (i < old.len()).then(|| old.start + i);
                    let r = (i < new.len()).then(|| new.start + i);
                    rows.push(AlignedRow { left: l, right: r, kind: RowKind::Modified });
                }
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gutter_changes() {
        let base = "a\nb\nc\nd\n";
        let cur = "a\nB\nc\nnew\nd\n";
        let ch = line_changes(base, cur);
        assert_eq!(
            ch,
            vec![
                LineChange { kind: LineChangeKind::Modified, lines: 1..2 },
                LineChange { kind: LineChangeKind::Added, lines: 3..4 },
            ]
        );
        let ch = line_changes("a\nb\nc\n", "a\nc\n");
        assert_eq!(ch, vec![LineChange { kind: LineChangeKind::Deleted, lines: 1..1 }]);
    }

    #[test]
    fn aligned_rows() {
        let rows = align("a\nb\nc\n", "a\nx\ny\nc\n");
        let kinds: Vec<_> = rows.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, vec![RowKind::Equal, RowKind::Modified, RowKind::Modified, RowKind::Equal]);
        assert_eq!(rows[2], AlignedRow { left: None, right: Some(2), kind: RowKind::Modified });
    }
}
