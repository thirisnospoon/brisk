//! Rename (Shift+F6) and Find Usages (Alt+F7) without ty: within this file, from the syntax
//! tree's scopes (`brisk_syntax::NameRefs`). Rename covers local variables and parameters (all
//! their uses are in the function); Find Usages also module-level names, marked "file only".

use super::Workspace;
use crate::editor::Editor;
use gpui::{Context, Entity, Window};
use std::path::PathBuf;

impl Workspace {
    /// Alt+F7 without ty: usages in this file.
    pub(super) fn local_usages(&mut self, ed: Entity<Editor>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(refs) = ed.update(cx, |e, _| e.name_refs_at_caret()) else { return self.no_ty("Find Usages") };
        let e = ed.read(cx);
        let path = e.path.clone().unwrap_or_else(|| PathBuf::from(e.title()));
        let rope = e.buffer.rope();
        let usages: Vec<_> = refs
            .ranges
            .iter()
            .map(|r| {
                let (a, z) = (rope.byte_to_char(r.start), rope.byte_to_char(r.end));
                let line = rope.char_to_line(a);
                let col = a - rope.line_to_char(line);
                (path.clone(), line, col..col + (z - a))
            })
            .collect();
        let name = refs.name;
        let n = self.show_usages(format!("Usages of '{name}' (file only, without ty)"), usages, window, cx);
        self.status = format!("{n} usage(s) of '{name}' in this file (file only: without ty other files are not searched)");
    }

    /// Shift+F6 without ty: a local variable or parameter is renamed in place, every use
    /// selected (multiple carets) so typing replaces them together, as PyCharm does for locals.
    pub(super) fn local_rename(&mut self, ed: Entity<Editor>, cx: &mut Context<Self>) {
        let Some(refs) = ed.update(cx, |e, _| e.name_refs_at_caret()) else { return self.no_ty("Rename") };
        if !refs.local {
            self.status = format!("Rename without ty works for local variables and parameters; '{}' is not local (other files may use it)", refs.name);
            return;
        }
        let n = refs.ranges.len();
        ed.update(cx, |e, cx| {
            let rope = e.buffer.rope();
            let head = e.buffer.primary().head;
            let mut sels: Vec<brisk_text::Selection> = refs
                .ranges
                .iter()
                .map(|r| brisk_text::Selection { anchor: rope.byte_to_char(r.start), head: rope.byte_to_char(r.end), goal_col: None })
                .collect();
            // The occurrence at the caret is the primary (last) selection.
            if let Some(i) = sels.iter().position(|s| s.anchor <= head && head <= s.head) {
                let s = sels.remove(i);
                sels.push(s);
            }
            e.buffer.selections = sels;
            cx.notify();
        });
        self.status = format!("Rename '{}': type the new name ({n} occurrence(s) in the function), Esc when done", refs.name);
    }
}
