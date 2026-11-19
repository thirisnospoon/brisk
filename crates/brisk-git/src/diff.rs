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
    /// The lines it replaces in the base (empty for an addition).
    pub base: Range<usize>,
}

/// Changes of `current` relative to `base`, for the editor gutter.
pub fn line_changes(base: &str, current: &str) -> Vec<LineChange> {
    let diff = TextDiff::configure().timeout(std::time::Duration::from_millis(200)).diff_lines(base, current);
    let mut out = Vec::new();
    for op in diff.ops() {
        let (tag, old, new) = op.as_tag_tuple();
        match tag {
            DiffTag::Equal => {}
            DiffTag::Insert => out.push(LineChange { kind: LineChangeKind::Added, lines: new, base: old }),
            DiffTag::Delete => out.push(LineChange { kind: LineChangeKind::Deleted, lines: new.start..new.start, base: old }),
            DiffTag::Replace => out.push(LineChange { kind: LineChangeKind::Modified, lines: new, base: old }),
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

/// Word tokens for an inline diff: identifier runs, whitespace runs, and single other chars
/// (PyCharm highlights `foo(bar)` → `foo(baz)` as just `bar`→`baz`, not the whole word).
/// The line-level edit from `old` to `new`, stretch by stretch: (old lines, new lines,
/// unchanged). The merge window follows its ranges through the user's edits with it.
pub fn line_ops(old: &str, new: &str) -> Vec<(Range<usize>, Range<usize>, bool)> {
    let diff = TextDiff::configure().timeout(std::time::Duration::from_millis(200)).diff_lines(old, new);
    diff.ops().iter().map(|op| (op.old_range(), op.new_range(), op.tag() == DiffTag::Equal)).collect()
}

pub(crate) fn tokens(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let class = |c: char| if c.is_alphanumeric() || c == '_' { 0 } else if c.is_whitespace() { 1 } else { 2 };
    let mut prev: Option<u8> = None;
    for (i, c) in s.char_indices() {
        let k = class(c);
        if let Some(p) = prev {
            if p != k || k == 2 {
                out.push(&s[start..i]);
                start = i;
            }
        }
        prev = Some(k);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// Changed byte ranges inside a modified line pair: (in `old`, in `new`). Adjacent changes
/// are merged, and a change separated from the next only by whitespace absorbs it, so a
/// reworded phrase reads as one highlight. Empty when the lines are equal.
pub fn inline_diff(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    if old == new {
        return (Vec::new(), Vec::new());
    }
    let a = tokens(old);
    let b = tokens(new);
    let offs = |t: &[&str]| {
        let mut v = Vec::with_capacity(t.len() + 1);
        let mut o = 0;
        v.push(0);
        for s in t {
            o += s.len();
            v.push(o);
        }
        v
    };
    let (oa, ob) = (offs(&a), offs(&b));
    let ops = similar::capture_diff_slices_deadline(
        similar::Algorithm::Myers,
        &a,
        &b,
        Some(std::time::Instant::now() + std::time::Duration::from_millis(20)),
    );
    let (mut l, mut r): (Vec<Range<usize>>, Vec<Range<usize>>) = (Vec::new(), Vec::new());
    let push = |v: &mut Vec<Range<usize>>, rg: Range<usize>, text: &str| {
        if rg.is_empty() {
            return;
        }
        if let Some(last) = v.last_mut() {
            if text[last.end..rg.start].chars().all(char::is_whitespace) {
                last.end = rg.end;
                return;
            }
        }
        v.push(rg);
    };
    for op in ops {
        let (tag, o, n) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            continue;
        }
        push(&mut l, oa[o.start]..oa[o.end], old);
        push(&mut r, ob[n.start]..ob[n.end], new);
    }
    (l, r)
}

/// One row of a unified (one-column) diff: old and new line numbers (0-based).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnifiedRow {
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub kind: RowKind,
    /// The aligned row this line comes from (for its inline highlights).
    pub aligned: usize,
}

/// The unified layout of aligned rows: a modified block lists its old lines, then its new
/// ones, as `git diff` and PyCharm's "Unified viewer" do.
pub fn unified(rows: &[AlignedRow]) -> Vec<UnifiedRow> {
    let mut out = Vec::with_capacity(rows.len());
    let mut i = 0;
    while i < rows.len() {
        if rows[i].kind == RowKind::Equal {
            out.push(UnifiedRow { old: rows[i].left, new: rows[i].right, kind: RowKind::Equal, aligned: i });
            i += 1;
            continue;
        }
        let mut j = i;
        while j < rows.len() && rows[j].kind != RowKind::Equal {
            j += 1;
        }
        for (k, r) in rows[i..j].iter().enumerate() {
            if let Some(l) = r.left {
                out.push(UnifiedRow { old: Some(l), new: None, kind: RowKind::Deleted, aligned: i + k });
            }
        }
        for (k, r) in rows[i..j].iter().enumerate() {
            if let Some(n) = r.right {
                out.push(UnifiedRow { old: None, new: Some(n), kind: RowKind::Added, aligned: i + k });
            }
        }
        i = j;
    }
    out
}

/// Runs of unchanged rows to collapse: every equal run longer than `2 * context + 1` keeps
/// `context` rows on each side (none at the file's start or end) and folds the middle.
/// `equal(i)` tells whether row `i` is unchanged. Returns the folded index ranges.
pub fn foldable(n: usize, context: usize, equal: impl Fn(usize) -> bool) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if !equal(i) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < n && equal(j) {
            j += 1;
        }
        let lo = if i == 0 { 0 } else { i + context };
        let hi = if j == n { n } else { j.saturating_sub(context) };
        if hi > lo && hi - lo > 1 {
            out.push(lo..hi);
        }
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_words() {
        let (l, r) = inline_diff("x = foo(bar, 1)", "x = foo(baz, 1)");
        assert_eq!((l, r), (vec![8..11], vec![8..11]));
        let (l, r) = inline_diff("print(a)", "print(a, b)");
        assert_eq!((l, r), (vec![], vec![7..10]));
        // Two changed words with a space between read as one highlight.
        let (l, r) = inline_diff("return old value", "return new thing");
        assert_eq!((l, r), (vec![7..16], vec![7..16]));
        assert_eq!(inline_diff("same", "same"), (vec![], vec![]));
    }

    #[test]
    fn unified_rows_and_folds() {
        let rows = align("a\nb\nc\n", "a\nx\ny\nc\n");
        let u = unified(&rows);
        let kinds: Vec<_> = u.iter().map(|r| (r.kind, r.old, r.new)).collect();
        assert_eq!(
            kinds,
            vec![
                (RowKind::Equal, Some(0), Some(0)),
                (RowKind::Deleted, Some(1), None),
                (RowKind::Added, None, Some(1)),
                (RowKind::Added, None, Some(2)),
                (RowKind::Equal, Some(2), Some(3)),
            ]
        );
        // 20 equal, 1 change, 20 equal, with 3 lines of context.
        let eq = |i: usize| i != 20;
        assert_eq!(foldable(41, 3, eq), vec![0..17, 24..41]);
        // A short equal run between changes is kept.
        assert_eq!(foldable(9, 3, |i| i != 0 && i != 8), Vec::<Range<usize>>::new());
    }

    #[test]
    fn gutter_changes() {
        let base = "a\nb\nc\nd\n";
        let cur = "a\nB\nc\nnew\nd\n";
        let ch = line_changes(base, cur);
        assert_eq!(
            ch,
            vec![
                LineChange { kind: LineChangeKind::Modified, lines: 1..2, base: 1..2 },
                LineChange { kind: LineChangeKind::Added, lines: 3..4, base: 3..3 },
            ]
        );
        let ch = line_changes("a\nb\nc\n", "a\nc\n");
        assert_eq!(ch, vec![LineChange { kind: LineChangeKind::Deleted, lines: 1..1, base: 1..2 }]);
    }

    #[test]
    fn aligned_rows() {
        let rows = align("a\nb\nc\n", "a\nx\ny\nc\n");
        let kinds: Vec<_> = rows.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, vec![RowKind::Equal, RowKind::Modified, RowKind::Modified, RowKind::Equal]);
        assert_eq!(rows[2], AlignedRow { left: None, right: Some(2), kind: RowKind::Modified });
    }
}
