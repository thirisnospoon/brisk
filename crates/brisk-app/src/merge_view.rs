//! The merge window, as in PyCharm's "Merge Revisions for <file>": Your version | Result |
//! Changes from <branch>, left to right. The result is an editor you can type in; every
//! change has » / « to accept that side (on the other side afterwards: append it) and × to
//! ignore it. "Apply non-conflicting changes" (left, all, right) and "Resolve simple
//! conflicts" (two sides changing different words of the same lines) work in bulk. The three
//! panes scroll together, with curved connectors between a change on a side and its place in
//! the result; F7 / Shift+F7 walk the changes, Alt+Shift+→ / ← accept a side.

use crate::diff_view::{line_text, right_to_left, side, tool, Block, Side};
use crate::editor::Editor;
use crate::settings::{font_size, line_height};
use crate::theme::{mono_font, ui_font, ui_px, Theme};
use brisk_git::Chunk;
use gpui::prelude::*;
use gpui::{
    canvas, div, point, px, uniform_list, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla, Subscription, UniformListScrollHandle, Window,
};
use std::ops::Range;
use std::path::PathBuf;

/// What one side did with a chunk, and what the user did with that change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SideState {
    /// The side left the chunk as in the base.
    Unchanged,
    Pending,
    Applied,
    Ignored,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChangeState {
    pub ours: SideState,
    pub theirs: SideState,
    /// Both sides applied: ours first.
    pub ours_first: bool,
    /// Typed in, or resolved as a simple conflict: the result is whatever the editor holds.
    pub edited: bool,
}

/// The decisions of a merge, without the UI.
pub struct MergeModel {
    pub chunks: Vec<Chunk>,
    pub st: Vec<ChangeState>,
}

impl MergeModel {
    pub fn new(base: &str, ours: &str, theirs: &str) -> Self {
        let chunks = brisk_git::merge3(base, ours, theirs);
        let side = |changed: bool| if changed { SideState::Pending } else { SideState::Unchanged };
        let st = chunks
            .iter()
            .map(|c| match c {
                Chunk::Same(_) => ChangeState { ours: SideState::Unchanged, theirs: SideState::Unchanged, ours_first: true, edited: false },
                Chunk::Change { base, ours, theirs } => ChangeState { ours: side(ours != base), theirs: side(theirs != base), ours_first: true, edited: false },
            })
            .collect();
        Self { chunks, st }
    }

    pub fn is_change(&self, i: usize) -> bool {
        matches!(self.chunks.get(i), Some(Chunk::Change { .. }))
    }

    pub fn is_conflict(&self, i: usize) -> bool {
        self.chunks.get(i).is_some_and(|c| c.is_conflict())
    }

    pub fn resolved(&self, i: usize) -> bool {
        let s = self.st[i];
        let pending = |x: SideState| x == SideState::Pending;
        !self.is_change(i) || s.edited || s.ours == SideState::Applied || s.theirs == SideState::Applied || (!pending(s.ours) && !pending(s.theirs))
    }

    pub fn changes(&self) -> usize {
        (0..self.chunks.len()).filter(|&i| self.is_change(i)).count()
    }

    pub fn unresolved(&self) -> usize {
        (0..self.chunks.len()).filter(|&i| !self.resolved(i)).count()
    }

    pub fn unresolved_conflicts(&self) -> usize {
        (0..self.chunks.len()).filter(|&i| self.is_conflict(i) && !self.resolved(i)).count()
    }

    /// (base, ours, theirs) lines of chunk `i`.
    pub fn parts(&self, i: usize) -> (&[String], &[String], &[String]) {
        match &self.chunks[i] {
            Chunk::Same(v) => (v, v, v),
            Chunk::Change { base, ours, theirs } => (base, ours, theirs),
        }
    }

    /// » (`ours`) or «: take that side; with the other side already taken, append this one.
    pub fn apply(&mut self, i: usize, ours: bool) {
        if !self.is_change(i) {
            return;
        }
        let identical = {
            let (_, o, t) = self.parts(i);
            o == t
        };
        let s = &mut self.st[i];
        let other = if ours { s.theirs } else { s.ours };
        if ours {
            s.ours = SideState::Applied;
        } else {
            s.theirs = SideState::Applied;
        }
        // The same change on both sides is one change.
        if identical {
            s.ours = SideState::Applied;
            s.theirs = SideState::Applied;
        }
        if other != SideState::Applied {
            s.ours_first = ours;
        }
        s.edited = false;
    }

    /// × on a side: leave that side's change out.
    pub fn ignore(&mut self, i: usize, ours: bool) {
        if !self.is_change(i) {
            return;
        }
        let s = &mut self.st[i];
        let x = if ours { &mut s.ours } else { &mut s.theirs };
        if *x != SideState::Unchanged {
            *x = SideState::Ignored;
        }
        s.edited = false;
    }

    /// Accept Left / Accept Right: the result becomes that side's file. Returns the changes.
    pub fn accept_all(&mut self, ours: bool) -> Vec<usize> {
        let idx: Vec<usize> = (0..self.chunks.len()).filter(|&i| self.is_change(i)).collect();
        for &i in &idx {
            let s = &mut self.st[i];
            let (me, other) = if ours { (&mut s.ours, &mut s.theirs) } else { (&mut s.theirs, &mut s.ours) };
            if *me != SideState::Unchanged {
                *me = SideState::Applied;
            }
            if *other != SideState::Unchanged {
                *other = SideState::Ignored;
            }
            s.ours_first = ours;
            s.edited = false;
        }
        idx
    }

    /// Apply non-conflicting changes: from the left (`Some(true)`), the right, or both (`None`).
    pub fn apply_non_conflicting(&mut self, from: Option<bool>) -> Vec<usize> {
        let mut out = Vec::new();
        for i in 0..self.chunks.len() {
            if !self.is_change(i) || self.is_conflict(i) || self.resolved(i) {
                continue;
            }
            let s = self.st[i];
            if s.ours == SideState::Pending && from != Some(false) {
                self.apply(i, true);
                out.push(i);
            } else if s.theirs == SideState::Pending && from != Some(true) {
                self.apply(i, false);
                out.push(i);
            }
        }
        out
    }

    /// The lines of chunk `i` in the result, from the decisions (not from typing).
    pub fn result_lines(&self, i: usize) -> Vec<String> {
        let (b, o, t) = self.parts(i);
        let s = self.st[i];
        match (s.ours == SideState::Applied, s.theirs == SideState::Applied) {
            (true, true) if o == t => o.to_vec(),
            (true, true) if s.ours_first => o.iter().chain(t).cloned().collect(),
            (true, true) => t.iter().chain(o).cloned().collect(),
            (true, false) => o.to_vec(),
            (false, true) => t.to_vec(),
            _ => b.to_vec(),
        }
    }

    pub fn result_text(&self, final_newline: bool) -> String {
        let lines: Vec<String> = (0..self.chunks.len()).flat_map(|i| self.result_lines(i)).collect();
        let mut s = lines.join("\n");
        if final_newline && !s.is_empty() {
            s.push('\n');
        }
        s
    }
}

/// Where old line boundary `p` lands after an edit (`line_ops`). Lines inserted right at `p`
/// go after it (`hi` = false: the change starting at `p` takes them) or before it.
fn map_line(ops: &[(Range<usize>, Range<usize>, bool)], p: usize, hi: bool) -> usize {
    for (o, n, eq) in ops {
        if *eq {
            if o.start <= p && p < o.end {
                return n.start + (p - o.start);
            }
        } else if o.is_empty() {
            if o.start == p {
                return if hi { n.end } else { n.start };
            }
        } else if o.start == p {
            return n.start;
        } else if o.start < p && p < o.end {
            return if hi { n.end } else { n.start };
        }
    }
    ops.last().map_or(p, |(_, n, _)| n.end)
}

/// Changed words of `side` against `base`, per line of `side` (byte ranges in the line).
fn words_per_line(base: &[String], side: &[String]) -> Vec<Vec<Range<usize>>> {
    let mut out = vec![Vec::new(); side.len()];
    if base.is_empty() || side.is_empty() || base == side || side.len() > 60 || base.len() > 60 {
        return out;
    }
    let (b, s) = (base.join("\n"), side.join("\n"));
    let (_, ranges) = brisk_git::inline_diff(&b, &s);
    let mut start = 0;
    for (k, line) in side.iter().enumerate() {
        let end = start + line.len();
        if !line.contains('\t') {
            for r in &ranges {
                let (a, z) = (r.start.max(start), r.end.min(end));
                if a < z {
                    out[k].push(a - start..z - start);
                }
            }
        }
        start = end + 1;
    }
    out
}

/// `text` with `\r\n` line endings when `crlf`, else as it is.
fn with_eol(text: &str, crlf: bool) -> String {
    if crlf {
        text.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        text.to_string()
    }
}

pub enum MergeEvent {
    /// Apply: write this text and mark the file resolved.
    Apply(String),
    Cancel,
}

pub struct MergeView {
    pub title: String,
    pub rel: String,
    pub path: PathBuf,
    pub theirs_label: String,
    pub model: MergeModel,
    final_newline: bool,
    /// The file uses `\r\n`; the panes work with `\n`, Apply writes it back as it was.
    crlf: bool,
    left: Side,
    right: Side,
    /// Per chunk: its lines in Yours, Theirs and the result (the result's follow the typing).
    ours_at: Vec<Range<usize>>,
    theirs_at: Vec<Range<usize>>,
    res_at: Vec<Range<usize>>,
    /// Per line of Yours / Theirs: its chunk, and its changed words against the base.
    ours_chunk: Vec<usize>,
    theirs_chunk: Vec<usize>,
    ours_words: Vec<Vec<Range<usize>>>,
    theirs_words: Vec<Vec<Range<usize>>>,
    editor: Entity<Editor>,
    /// The editor's text as last seen.
    text: String,
    pub current: Option<usize>,
    /// Apply was pressed with changes left: the next press applies anyway.
    confirm_apply: bool,
    left_scroll: UniformListScrollHandle,
    right_scroll: UniformListScrollHandle,
    focus: FocusHandle,
    _sub: Subscription,
}

impl EventEmitter<MergeEvent> for MergeView {}

impl MergeView {
    pub fn new(rel: String, path: PathBuf, theirs_label: String, v: brisk_git::ConflictVersions, cx: &mut Context<Self>) -> Self {
        let base = v.base.unwrap_or_default();
        let ours = v.ours.unwrap_or_default();
        let theirs = v.theirs.unwrap_or_default();
        let model = MergeModel::new(&base, &ours, &theirs);
        let final_newline = ours.is_empty() || ours.ends_with('\n');
        let crlf = v.crlf;
        let lang = brisk_syntax::Lang::from_path(&path).filter(|_| ours.len().max(theirs.len()) <= 2_000_000);
        let (left, right) = (Side::new(&ours, lang), Side::new(&theirs, lang));
        let (mut ours_at, mut theirs_at, mut res_at) = (Vec::new(), Vec::new(), Vec::new());
        let (mut ours_chunk, mut theirs_chunk) = (Vec::new(), Vec::new());
        let (mut ours_words, mut theirs_words) = (Vec::new(), Vec::new());
        let (mut o, mut t, mut r) = (0, 0, 0);
        for i in 0..model.chunks.len() {
            let (b, ol, tl) = model.parts(i);
            ours_at.push(o..o + ol.len());
            theirs_at.push(t..t + tl.len());
            let n = model.result_lines(i).len();
            res_at.push(r..r + n);
            ours_chunk.extend(std::iter::repeat_n(i, ol.len()));
            theirs_chunk.extend(std::iter::repeat_n(i, tl.len()));
            ours_words.extend(words_per_line(b, ol));
            theirs_words.extend(words_per_line(b, tl));
            (o, t, r) = (o + ol.len(), t + tl.len(), r + n);
        }
        let text = model.result_text(final_newline);
        let editor = cx.new(|cx| {
            let mut e = Editor::scratch(&text, cx);
            e.set_language(lang);
            e.show_gutter = true;
            e
        });
        let sub = cx.observe(&editor, |this: &mut Self, _, cx| this.editor_changed(cx));
        let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
        let mut v = Self {
            title: format!("Merge Revisions for {name}"),
            rel,
            path,
            theirs_label,
            model,
            final_newline,
            crlf,
            left,
            right,
            ours_at,
            theirs_at,
            res_at,
            ours_chunk,
            theirs_chunk,
            ours_words,
            theirs_words,
            editor,
            text,
            current: None,
            confirm_apply: false,
            left_scroll: UniformListScrollHandle::new(),
            right_scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            _sub: sub,
        };
        // Start at the first conflict (else the first change), as PyCharm does.
        let first = (0..v.model.chunks.len()).find(|&i| v.model.is_conflict(i)).or_else(|| (0..v.model.chunks.len()).find(|&i| v.model.is_change(i)));
        if let Some(i) = first {
            v.current = Some(i);
            let line = v.res_at[i].start;
            v.editor.update(cx, |e, cx| e.go_to_line(line, cx));
        }
        v
    }

    pub fn editor(&self) -> Entity<Editor> {
        self.editor.clone()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The user typed: follow the chunks through the edit; a change typed in is resolved.
    fn editor_changed(&mut self, cx: &mut Context<Self>) {
        let new = self.editor.read(cx).text_string();
        if new == self.text {
            return;
        }
        let ops = brisk_git::line_ops(&self.text, &new);
        let old_lines: Vec<&str> = self.text.split('\n').collect();
        let new_lines: Vec<&str> = new.split('\n').collect();
        let n = self.res_at.len();
        let mut bounds = Vec::with_capacity(n + 1);
        for k in 0..=n {
            let p = if k < n { self.res_at[k].start } else { self.res_at.last().map_or(0, |r| r.end) };
            let ends_change = k > 0 && self.model.is_change(k - 1);
            let starts_change = k < n && self.model.is_change(k);
            let q = map_line(&ops, p, ends_change && !starts_change);
            bounds.push(q.max(bounds.last().copied().unwrap_or(0)));
        }
        let slice = |v: &[&str], r: &Range<usize>| -> Vec<String> { v[r.start.min(v.len())..r.end.min(v.len())].iter().map(|s| s.to_string()).collect() };
        for i in 0..n {
            let r = bounds[i]..bounds[i + 1];
            if self.model.is_change(i) && !self.model.st[i].edited && slice(&old_lines, &self.res_at[i]) != slice(&new_lines, &r) {
                self.model.st[i].edited = true;
            }
            self.res_at[i] = r;
        }
        self.text = new;
        self.confirm_apply = false;
        cx.notify();
    }

    /// Write chunks into the result editor (one undoable edit): (chunk, its new lines).
    fn write(&mut self, mut parts: Vec<(usize, Vec<String>)>, cx: &mut Context<Self>) {
        if parts.is_empty() {
            return;
        }
        parts.sort_by(|a, b| b.0.cmp(&a.0));
        let mut all: Vec<String> = self.text.split('\n').map(str::to_string).collect();
        let mut lens: Vec<usize> = self.res_at.iter().map(|r| r.len()).collect();
        for (i, lines) in &parts {
            let r = self.res_at[*i].clone();
            let (a, z) = (r.start.min(all.len()), r.end.min(all.len()));
            all.splice(a..z, lines.iter().cloned());
            lens[*i] = lines.len();
        }
        let mut at = 0;
        for (i, len) in lens.into_iter().enumerate() {
            self.res_at[i] = at..at + len;
            at += len;
        }
        let new = all.join("\n");
        self.text = new.clone();
        self.confirm_apply = false;
        self.editor.update(cx, |e, cx| e.replace_all_text(&new, cx));
        cx.notify();
    }

    fn rewrite(&mut self, idx: Vec<usize>, cx: &mut Context<Self>) {
        let parts = idx.into_iter().map(|i| (i, self.model.result_lines(i))).collect();
        self.write(parts, cx);
    }

    pub fn apply(&mut self, i: usize, ours: bool, cx: &mut Context<Self>) {
        self.model.apply(i, ours);
        self.current = Some(i);
        self.rewrite(vec![i], cx);
    }

    pub fn ignore(&mut self, i: usize, ours: bool, cx: &mut Context<Self>) {
        self.model.ignore(i, ours);
        self.current = Some(i);
        self.rewrite(vec![i], cx);
    }

    pub fn accept_all(&mut self, ours: bool, cx: &mut Context<Self>) {
        let idx = self.model.accept_all(ours);
        self.rewrite(idx, cx);
    }

    pub fn apply_non_conflicting(&mut self, from: Option<bool>, cx: &mut Context<Self>) {
        let idx = self.model.apply_non_conflicting(from);
        self.rewrite(idx, cx);
    }

    /// Resolve Simple Conflicts: conflicts whose sides change different words merge word by word.
    pub fn resolve_simple(&mut self, cx: &mut Context<Self>) -> usize {
        let mut parts = Vec::new();
        for i in 0..self.model.chunks.len() {
            if self.model.is_conflict(i) && !self.model.resolved(i) {
                let (b, o, t) = self.model.parts(i);
                if let Some(lines) = brisk_git::resolve_simple(b, o, t) {
                    parts.push((i, lines));
                }
            }
        }
        for (i, _) in &parts {
            self.model.st[*i].edited = true;
        }
        let n = parts.len();
        self.write(parts, cx);
        n
    }

    /// The current change for Alt+Shift+→ / ←: the one the caret is in, else the selected one.
    fn change_at_caret(&self, cx: &App) -> Option<usize> {
        let line = self.editor.read(cx).caret_line_col().0;
        (0..self.res_at.len())
            .find(|&i| self.model.is_change(i) && (self.res_at[i].contains(&line) || (self.res_at[i].is_empty() && self.res_at[i].start == line)))
            .or(self.current)
    }

    pub fn accept_current(&mut self, ours: bool, cx: &mut Context<Self>) {
        if let Some(i) = self.change_at_caret(cx) {
            self.apply(i, ours, cx);
        }
    }

    /// F7 / Shift+F7: the next / previous change.
    pub fn jump(&mut self, forward: bool, cx: &mut Context<Self>) {
        let n = self.model.chunks.len();
        let line = self.editor.read(cx).caret_line_col().0;
        let next = if forward {
            (0..n).find(|&i| self.model.is_change(i) && self.res_at[i].start > line)
        } else {
            (0..n).rev().find(|&i| self.model.is_change(i) && self.res_at[i].start < line)
        };
        if let Some(i) = next {
            self.current = Some(i);
            let l = self.res_at[i].start;
            self.editor.update(cx, |e, cx| e.go_to_line(l, cx));
            cx.notify();
        }
    }

    pub fn press_apply(&mut self, cx: &mut Context<Self>) {
        if self.model.unresolved() > 0 && !self.confirm_apply {
            self.confirm_apply = true;
            cx.notify();
            return;
        }
        let mut text = self.editor.read(cx).text_string();
        if self.final_newline && !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        cx.emit(MergeEvent::Apply(with_eol(&text, self.crlf)));
    }

    /// A change's color: conflicts red, else by what the side did (added, deleted, modified).
    fn color(&self, i: usize, lines: &[String]) -> Hsla {
        let (b, _, _) = self.model.parts(i);
        if self.model.is_conflict(i) {
            Theme::diff_conflict_bg()
        } else if b.is_empty() {
            Theme::diff_added_bg()
        } else if lines.is_empty() {
            Theme::diff_deleted_bg()
        } else {
            Theme::diff_modified_bg()
        }
    }

    /// The changed side of a chunk as shown: its lines, whether it is still to be decided.
    fn side_state(&self, i: usize, ours: bool) -> SideState {
        let s = self.model.st[i];
        if ours {
            s.ours
        } else {
            s.theirs
        }
    }

    fn render_side_row(&self, ours: bool, l: usize) -> gpui::AnyElement {
        let (sd, chunk_of, words, at) = if ours {
            (&self.left, &self.ours_chunk, &self.ours_words, &self.ours_at)
        } else {
            (&self.right, &self.theirs_chunk, &self.theirs_words, &self.theirs_at)
        };
        let Some(&i) = chunk_of.get(l) else {
            return div().h(px(line_height())).into_any_element();
        };
        let st = self.side_state(i, ours);
        let changed = self.model.is_change(i) && st != SideState::Unchanged;
        let lines = {
            let (_, o, t) = self.model.parts(i);
            if ours {
                o
            } else {
                t
            }
        };
        let bg = if !changed {
            Theme::editor_bg()
        } else {
            let c = self.color(i, lines);
            if st == SideState::Pending && !self.model.resolved(i) {
                c
            } else {
                c.opacity(0.45)
            }
        };
        let edges = if changed { (l == at[i].start, l + 1 == at[i].end) } else { (false, false) };
        let word_bg = if self.model.is_conflict(i) { Theme::diff_conflict_word_bg() } else { Theme::diff_modified_word_bg() };
        let none: &[Range<usize>] = &[];
        let w = if changed { words.get(l).map(|w| w.as_slice()).unwrap_or(none) } else { none };
        let (t, c) = sd.line(Some(l));
        let row = side(Some(l), line_text(t, c, w, word_bg), bg, edges);
        if ours {
            div().w_full().flex().pr(px(6.)).child(row).into_any_element()
        } else {
            div().w_full().flex().pl(px(6.)).child(row).into_any_element()
        }
    }

    /// A side pane: its lines, scrolled with the result; the wheel scrolls the result.
    fn render_pane(&self, ours: bool, y: f32, cx: &mut Context<Self>) -> gpui::AnyElement {
        let (n, scroll, id) =
            if ours { (self.left.lines.len(), &self.left_scroll, "merge-ours") } else { (self.right.lines.len(), &self.right_scroll, "merge-theirs") };
        scroll.0.borrow().base_handle.set_offset(point(px(0.), px(-y)));
        let ed = self.editor.clone();
        let lh = line_height();
        div()
            .id(if ours { "merge-ours-pane" } else { "merge-theirs-pane" })
            .flex_1()
            .min_w_0()
            .h_full()
            .on_scroll_wheel(move |ev, _, cx| {
                let d = ev.delta.pixel_delta(px(lh));
                ed.update(cx, |e, cx| {
                    let y = e.scroll_top() - f32::from(d.y);
                    e.set_scroll_top(y, cx);
                });
                cx.stop_propagation();
            })
            .child(
                // Blank rows past the end: the result scrolls its last line to the top.
                uniform_list(id, n + 150, cx.processor(move |this, range: Range<usize>, _, _| range.map(|l| this.render_side_row(ours, l)).collect()))
                    .track_scroll(scroll.clone())
                    .size_full()
                    .font_family(mono_font())
                    .text_size(px(font_size()))
                    .text_color(Theme::text()),
            )
            .into_any_element()
    }

    /// The strip between a side and the result: connectors, and » × (« ×) on each open change.
    fn render_strip(&self, ours: bool, side_y: f32, res_y: f32, cx: &mut Context<Self>) -> gpui::AnyElement {
        let lh = line_height();
        let at = if ours { &self.ours_at } else { &self.theirs_at };
        let mut shapes = Vec::new();
        let mut buttons: Vec<gpui::AnyElement> = Vec::new();
        for i in 0..self.model.chunks.len() {
            let st = self.side_state(i, ours);
            if !self.model.is_change(i) || st == SideState::Unchanged {
                continue;
            }
            let (s0, s1) = (at[i].start as f32 * lh - side_y, at[i].end as f32 * lh - side_y);
            let (r0, r1) = (self.res_at[i].start as f32 * lh - res_y, self.res_at[i].end as f32 * lh - res_y);
            if s1.max(r1) < -lh || s0.min(r0) > 4000. {
                continue;
            }
            let lines = {
                let (_, o, t) = self.model.parts(i);
                if ours {
                    o.to_vec()
                } else {
                    t.to_vec()
                }
            };
            let open = st == SideState::Pending && !self.model.resolved(i);
            let c = self.color(i, &lines);
            shapes.push((s0, s1, r0, r1, if open { c } else { c.opacity(0.45) }));
            // Decided, typed in or resolved as a simple conflict: no buttons.
            if st != SideState::Pending || self.model.st[i].edited {
                continue;
            }
            let other_applied = self.side_state(i, !ours) == SideState::Applied;
            let accept_tip = if other_applied { "Append" } else { "Accept" };
            let accept = mini(("merge-accept", i * 2 + ours as usize), if ours { "chevrons-right" } else { "chevrons-left" }, Theme::accent(), accept_tip)
                .on_click(cx.listener(move |this, _, _, cx| this.apply(i, ours, cx)));
            let ignore = mini(("merge-ignore", i * 2 + ours as usize), "x", Theme::text_muted(), "Ignore")
                .on_click(cx.listener(move |this, _, _, cx| this.ignore(i, ours, cx)));
            let pair = div().absolute().top(px(s0.max(0.) + 1.)).flex().gap(px(2.));
            buttons.push(if ours { pair.left(px(2.)).child(accept).child(ignore) } else { pair.right(px(2.)).child(ignore).child(accept) }.into_any_element());
        }
        div()
            .relative()
            .w(px(STRIP))
            .h_full()
            .flex_shrink_0()
            .overflow_hidden()
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let top = f32::from(bounds.top());
                        let (x0, x1) = (bounds.left(), bounds.right());
                        let mid = (x0 + x1) / 2.;
                        for (s0, s1, r0, r1, color) in &shapes {
                            // Left strip: side on the left; right strip: side on the right.
                            let (l0, l1, rr0, rr1) = if ours { (*s0, *s1, *r0, *r1) } else { (*r0, *r1, *s0, *s1) };
                            let (l1, rr1) = (l1.max(l0 + 2.), rr1.max(rr0 + 2.));
                            let mut pb = gpui::PathBuilder::fill();
                            pb.move_to(point(x0, px(top + l0)));
                            pb.cubic_bezier_to(point(x1, px(top + rr0)), point(mid, px(top + l0)), point(mid, px(top + rr0)));
                            pb.line_to(point(x1, px(top + rr1)));
                            pb.cubic_bezier_to(point(x0, px(top + l1)), point(mid, px(top + rr1)), point(mid, px(top + l1)));
                            pb.close();
                            if let Ok(path) = pb.build() {
                                window.paint_path(path, *color);
                            }
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .children(buttons)
            .into_any_element()
    }
}

/// The width of the strips between the panes (buttons, then the connectors).
const STRIP: f32 = 60.;

/// A small button on a change: accept / ignore.
fn mini(id: (&'static str, usize), icon: &'static str, color: Hsla, tip: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(ui_px(18.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .bg(Theme::panel())
        .border_1()
        .border_color(Theme::divider())
        .cursor_pointer()
        .hover(|s| s.bg(Theme::hover_row()))
        .tooltip(move |_, cx| cx.new(|_| crate::workspace::Tip(tip.into())).into())
        .child(crate::icons::ui_icon(icon, 12., color))
}

/// A dialog-style button at the bottom.
fn button(id: &'static str, label: &'static str, primary: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px(px(14.))
        .h(ui_px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .cursor_pointer()
        .when(primary, |b| b.bg(Theme::accent()).text_color(gpui::white()))
        .when(!primary, |b| b.border_1().border_color(Theme::divider()).hover(|s| s.bg(Theme::hover_row())))
        .active(|s| s.bg(Theme::press()))
        .child(label)
}

impl Focusable for MergeView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MergeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lh = line_height();
        let res_y = self.editor.read(cx).scroll_top();
        let blocks = |at: &[Range<usize>]| -> Vec<Block> {
            (0..self.model.chunks.len())
                .filter(|&i| self.model.is_change(i))
                .map(|i| Block { l0: at[i].start, l1: at[i].end, r0: self.res_at[i].start, r1: self.res_at[i].end })
                .collect()
        };
        let ours_y = right_to_left(&blocks(&self.ours_at), res_y / lh) * lh;
        let theirs_y = right_to_left(&blocks(&self.theirs_at), res_y / lh) * lh;
        // The result's change blocks, by state.
        let res_blocks: Vec<(Range<usize>, Hsla)> = (0..self.model.chunks.len())
            .filter(|&i| self.model.is_change(i))
            .map(|i| {
                let lines = self.model.result_lines(i);
                let c = if self.model.st[i].edited { Theme::diff_modified_bg() } else { self.color(i, &lines) };
                (self.res_at[i].clone(), if self.model.resolved(i) { c.opacity(0.45) } else { c })
            })
            .collect();
        self.editor.update(cx, |e, _| {
            e.diff_bg = true;
            e.diff_blocks = res_blocks;
        });
        let changes = self.model.changes();
        let left = self.model.unresolved();
        let conflicts = self.model.unresolved_conflicts();
        let status =
            if left == 0 { "All changes have been processed".to_string() } else { format!("{left} of {changes} change(s) left, {conflicts} conflict(s)") };
        let label = |t: String| div().flex_1().min_w_0().px(px(12.)).overflow_hidden().whitespace_nowrap().child(t);
        let ours_pane = self.render_pane(true, ours_y, cx);
        let theirs_pane = self.render_pane(false, theirs_y, cx);
        let strip_l = self.render_strip(true, ours_y, res_y, cx);
        let strip_r = self.render_strip(false, theirs_y, res_y, cx);
        let confirm = self.confirm_apply;
        div()
            .track_focus(&self.focus)
            .key_context("MergeView")
            .size_full()
            .flex()
            .flex_col()
            .bg(Theme::editor_bg())
            .child(
                div()
                    .h(ui_px(32.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(8.))
                    .bg(Theme::panel())
                    .border_b_1()
                    .border_color(Theme::divider())
                    .font_family(ui_font())
                    .text_size(ui_px(12.))
                    .text_color(Theme::text_muted())
                    .child(tool("merge-prev", "arrow-up", "Previous Difference (Shift+F7)", false).on_click(cx.listener(|this, _, _, cx| this.jump(false, cx))))
                    .child(tool("merge-next", "arrow-down", "Next Difference (F7)", false).on_click(cx.listener(|this, _, _, cx| this.jump(true, cx))))
                    .child(div().w(px(1.)).h(ui_px(16.)).mx(px(4.)).bg(Theme::divider()))
                    .child(
                        tool("merge-nc-left", "arrow-right-to-line", "Apply Non-Conflicting Changes from the Left Side", false)
                            .on_click(cx.listener(|this, _, _, cx| this.apply_non_conflicting(Some(true), cx))),
                    )
                    .child(
                        tool("merge-nc-all", "check-check", "Apply All Non-Conflicting Changes", false)
                            .on_click(cx.listener(|this, _, _, cx| this.apply_non_conflicting(None, cx))),
                    )
                    .child(
                        tool("merge-nc-right", "arrow-left-to-line", "Apply Non-Conflicting Changes from the Right Side", false)
                            .on_click(cx.listener(|this, _, _, cx| this.apply_non_conflicting(Some(false), cx))),
                    )
                    .child(div().w(px(1.)).h(ui_px(16.)).mx(px(4.)).bg(Theme::divider()))
                    .child(tool("merge-simple", "wand-sparkles", "Resolve Simple Conflicts", false).on_click(cx.listener(|this, _, _, cx| {
                        this.resolve_simple(cx);
                    })))
                    .child(div().flex_1())
                    .child(div().text_color(if left == 0 { Theme::file_added() } else { Theme::text_muted() }).child(status)),
            )
            .child(
                div()
                    .h(ui_px(24.))
                    .flex()
                    .items_center()
                    .font_family(ui_font())
                    .text_size(ui_px(12.))
                    .text_color(Theme::text_muted())
                    .border_b_1()
                    .border_color(Theme::divider())
                    .child(label("Your version".into()))
                    .child(div().w(px(STRIP)))
                    .child(label("Result".into()))
                    .child(div().w(px(STRIP)))
                    .child(label(self.theirs_label.clone())),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(ours_pane)
                    .child(strip_l)
                    .child(div().flex_1().min_w_0().h_full().child(self.editor.clone()))
                    .child(strip_r)
                    .child(theirs_pane),
            )
            .child(
                div()
                    .h(ui_px(44.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .bg(Theme::panel())
                    .border_t_1()
                    .border_color(Theme::divider())
                    .font_family(ui_font())
                    .text_size(ui_px(13.))
                    .text_color(Theme::text())
                    .child(button("merge-accept-left", "Accept Left", false).on_click(cx.listener(|this, _, _, cx| this.accept_all(true, cx))))
                    .child(button("merge-accept-right", "Accept Right", false).on_click(cx.listener(|this, _, _, cx| this.accept_all(false, cx))))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(ui_px(12.))
                            .text_color(if confirm { Theme::error() } else { Theme::text_dim() })
                            .child(if confirm {
                                format!("{left} change(s) are not resolved. Apply again to save the result as it is.")
                            } else {
                                "» / « accept a side · × ignore it · F7 / Shift+F7 changes · Alt+Shift+→ / ← · Ctrl+Enter Apply".to_string()
                            }),
                    )
                    .child(button("merge-cancel", "Cancel", false).on_click(cx.listener(|_, _, _, cx| cx.emit(MergeEvent::Cancel))))
                    .child(button("merge-apply", "Apply", true).on_click(cx.listener(|this, _, _, cx| this.press_apply(cx)))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_keeps_crlf() {
        assert_eq!(with_eol("a\nb\n", true), "a\r\nb\r\n");
        assert_eq!(with_eol("a\r\nb\n", true), "a\r\nb\r\n");
        assert_eq!(with_eol("a\nb\n", false), "a\nb\n");
    }

    #[test]
    fn decisions_and_result() {
        let mut m = MergeModel::new("a\nx\nb\nc\n", "a\nx\nB1\nc\nd\n", "A\nx\nB2\nc\n");
        // Nothing is applied by itself: the result starts as the base (PyCharm).
        assert_eq!(m.result_text(true), "a\nx\nb\nc\n");
        assert_eq!((m.changes(), m.unresolved(), m.unresolved_conflicts()), (3, 3, 1));
        // A (theirs only) and d (ours only) are not conflicts.
        m.apply_non_conflicting(None);
        assert_eq!(m.result_text(true), "A\nx\nb\nc\nd\n");
        assert_eq!(m.unresolved(), 1);
        let i = (0..m.chunks.len()).find(|&i| m.is_conflict(i)).unwrap();
        m.apply(i, true);
        assert_eq!(m.result_text(true), "A\nx\nB1\nc\nd\n");
        assert_eq!(m.unresolved(), 0);
        // « after »: appends.
        m.apply(i, false);
        assert_eq!(m.result_text(true), "A\nx\nB1\nB2\nc\nd\n");
        m.ignore(i, true);
        assert_eq!(m.result_text(true), "A\nx\nB2\nc\nd\n");
        m.accept_all(true);
        assert_eq!(m.result_text(true), "a\nx\nB1\nc\nd\n");
        m.accept_all(false);
        assert_eq!(m.result_text(true), "A\nx\nB2\nc\n");
        // Only the left side's non-conflicting changes.
        let mut m = MergeModel::new("a\nx\nb\nc\n", "a\nx\nB1\nc\nd\n", "A\nx\nB2\nc\n");
        m.apply_non_conflicting(Some(true));
        assert_eq!(m.result_text(true), "a\nx\nb\nc\nd\n");
    }

    #[test]
    fn line_mapping_through_edits() {
        let ops = brisk_git::line_ops("a\nb\nc\n", "a\nNEW\nb\nc\n");
        // Line 1 (b) moved down; a line inserted at 1 goes into the change starting there.
        assert_eq!(map_line(&ops, 1, false), 1);
        assert_eq!(map_line(&ops, 1, true), 2);
        assert_eq!(map_line(&ops, 2, false), 3);
        assert_eq!(map_line(&ops, 3, false), 4);
        let ops = brisk_git::line_ops("a\nb\nc\n", "a\nB\nc\n");
        assert_eq!((map_line(&ops, 1, false), map_line(&ops, 2, true)), (1, 2));
    }

    #[test]
    fn words_split_by_line() {
        let w = words_per_line(&["x = 1".into(), "y = 2".into()], &["x = 5".into(), "y = 2".into()]);
        assert_eq!(w, vec![vec![4..5], vec![]]);
    }
}
