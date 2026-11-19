//! The Find tool window's results as a tree, as in PyCharm: for Find Usages first the usage
//! groups ("Import statements", "Value write", "Usages in code", and for a rename preview
//! "Usages in comments and strings"), then files, then lines.
//! Groups and files fold with a click on the arrow, a double click, Enter, or Left / Right.

use super::{FindWindow, WRow};

/// Find Usages groups in the order the tree shows them.
pub(super) const USAGE_GROUPS: [&str; 4] = ["Import statements", "Value write", "Usages in code", "Usages in comments and strings"];

/// Which of `USAGE_GROUPS` a usage on `line` at char columns `col` belongs to.
pub(super) fn usage_group(line: &str, col: std::ops::Range<usize>) -> u8 {
    let t = line.trim_start();
    if t.starts_with("import ") || t.starts_with("from ") {
        return 0;
    }
    // Rename's "Search in comments and strings": after a `#`, or inside quotes.
    let before: String = line.chars().take(col.start).collect();
    let code = before.split('#').next().unwrap_or("");
    let odd = |s: &str, q: char| s.matches(q).count() % 2 == 1;
    let in_comment = code.len() < before.len() && !odd(code, '"') && !odd(code, '\'');
    if in_comment || odd(&before, '"') || odd(&before, '\'') {
        return 3;
    }
    let after: String = line.chars().skip(col.end).collect();
    let a = after.trim_start();
    let augmented = ["+=", "-=", "*=", "/=", "//=", "%=", "**=", "|=", "&=", "^=", ">>=", "<<=", "@="];
    // `x = …`, `x += …`, `for x in`, `with … as x`: the name is written.
    let b = before.trim_end();
    if (a.starts_with('=') && !a.starts_with("==")) || augmented.iter().any(|o| a.starts_with(o)) || b.ends_with(" as") || (b.trim_start() == "for" && a.starts_with("in ")) {
        return 1;
    }
    2
}

impl FindWindow {
    fn grouped(&self) -> bool {
        !self.group_of.is_empty()
    }

    fn node_key(r: WRow) -> Option<(u8, usize)> {
        match r {
            WRow::Group(g, _) => Some((g, usize::MAX)),
            WRow::File(g, f, _) => Some((g, f)),
            WRow::Match(_) => None,
        }
    }

    /// Rows for the current folding; the selection stays on the same node when it is visible.
    pub(super) fn rebuild_rows(&mut self) {
        let keep = self.rows.get(self.selected).copied();
        let m = &self.results.matches;
        let groups: Vec<u8> = if self.grouped() { (0..USAGE_GROUPS.len() as u8).collect() } else { vec![0] };
        let mut rows = Vec::new();
        for g in groups {
            let idx: Vec<usize> = (0..m.len()).filter(|&i| self.group_of.get(i).copied().unwrap_or(0) == g).collect();
            if idx.is_empty() {
                continue;
            }
            if self.grouped() {
                rows.push(WRow::Group(g, idx.len()));
                if self.collapsed.contains(&(g, usize::MAX)) {
                    continue;
                }
            }
            let mut i = 0;
            while i < idx.len() {
                let file = m[idx[i]].file;
                let n = idx[i..].iter().take_while(|&&j| m[j].file == file).count();
                rows.push(WRow::File(g, file, n));
                if !self.collapsed.contains(&(g, file)) {
                    rows.extend(idx[i..i + n].iter().map(|&j| WRow::Match(j)));
                }
                i += n;
            }
        }
        self.rows = rows;
        self.selected = keep.and_then(|k| self.rows.iter().position(|r| *r == k)).unwrap_or(self.selected.min(self.rows.len().saturating_sub(1)));
    }

    pub(super) fn is_collapsed(&self, r: WRow) -> bool {
        Self::node_key(r).is_some_and(|k| self.collapsed.contains(&k))
    }

    /// Fold or unfold the group or file at `row`; false on a line.
    pub(super) fn toggle(&mut self, row: usize) -> bool {
        let Some(k) = self.rows.get(row).copied().and_then(Self::node_key) else { return false };
        if !self.collapsed.remove(&k) {
            self.collapsed.insert(k);
        }
        self.selected = row;
        self.rebuild_rows();
        true
    }

    pub(super) fn depth(&self, r: WRow) -> usize {
        let g = usize::from(self.grouped());
        match r {
            WRow::Group(..) => 0,
            WRow::File(..) => g,
            WRow::Match(_) => g + 1,
        }
    }

    /// Left: fold the node, or go to its parent.
    pub(super) fn left(&mut self) {
        let Some(r) = self.rows.get(self.selected).copied() else { return };
        if Self::node_key(r).is_some() && !self.is_collapsed(r) {
            self.toggle(self.selected);
            return;
        }
        let d = self.depth(r);
        if let Some(p) = (0..self.selected).rev().find(|&i| self.depth(self.rows[i]) < d) {
            self.selected = p;
        }
    }

    /// Right: unfold the node, or go to its first child.
    pub(super) fn right(&mut self) {
        let Some(r) = self.rows.get(self.selected).copied() else { return };
        if self.is_collapsed(r) {
            self.toggle(self.selected);
        } else if Self::node_key(r).is_some() && self.selected + 1 < self.rows.len() {
            self.selected += 1;
        }
    }

    /// The visible tree as indented text, for smokes.
    pub fn rows_text(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|&r| {
                let pad = "  ".repeat(self.depth(r));
                let arrow = if self.is_collapsed(r) { "▸ " } else { "▾ " };
                match r {
                    WRow::Group(g, n) => format!("{pad}{arrow}{} ({n})", USAGE_GROUPS[g as usize]),
                    WRow::File(_, f, n) => format!("{pad}{arrow}{} ({n})", self.files.files[f]),
                    WRow::Match(i) => {
                        let m = &self.results.matches[i];
                        format!("{pad}{}: {}", m.line + 1, m.text.trim())
                    }
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::usage_group;

    #[test]
    fn usage_groups() {
        assert_eq!(usage_group("from pkg.util import add", 21..24), 0);
        assert_eq!(usage_group("    import os", 11..13), 0);
        assert_eq!(usage_group("x = 1", 0..1), 1);
        assert_eq!(usage_group("    x += 1", 4..5), 1);
        assert_eq!(usage_group("for x in y:", 4..5), 1);
        assert_eq!(usage_group("with open(p) as f:", 16..17), 1);
        assert_eq!(usage_group("if x == 1:", 3..4), 2);
        assert_eq!(usage_group("print(add(1, 2))", 6..9), 2);
        assert_eq!(usage_group("    # greet the user", 6..11), 3);
        assert_eq!(usage_group("G = \"greet\"", 5..10), 3);
        assert_eq!(usage_group("print('#', greet)", 11..16), 2);
    }
}
