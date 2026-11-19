//! Diff viewer tab (PyCharm's "Show Diff", Ctrl+D): side-by-side or unified, with syntax
//! colors, changed words highlighted inside changed lines, unchanged fragments collapsed
//! ("⋯ 37 unchanged lines", click to expand) and F7 / Shift+F7 between differences.
//! Rows are virtualized; the per-row word diff is computed when a row is first shown.
//!
//! A working-copy diff is live, as in PyCharm: the right side is the file's editor (the open
//! tab's own, or one the view opened), edits re-align the diff as you type, the left side
//! follows the editor's scrolling, and a strip between them connects the changed ranges.

use crate::editor::Editor;
use crate::settings::{font_size, line_height};
use crate::theme::{mono_font, ui_font, Theme};
use brisk_git::{AlignedRow, RowKind, UnifiedRow};
use gpui::prelude::*;
use gpui::{canvas, div, point, px, uniform_list, App, Context, Entity, FocusHandle, Focusable, Hsla, SharedString, StyledText, Subscription, TextRun, UniformListScrollHandle, Window};
use std::collections::HashSet;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;

/// Unchanged lines kept around a change when the rest is collapsed (PyCharm shows 4).
const CONTEXT: usize = 4;

/// The viewer's remembered options (`<config dir>/diff.json`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DiffPrefs {
    pub unified: bool,
    pub collapse: bool,
}

impl Default for DiffPrefs {
    fn default() -> Self {
        Self { unified: false, collapse: true }
    }
}

impl DiffPrefs {
    fn path() -> PathBuf {
        brisk_db::config_dir().join("diff.json")
    }

    pub fn load() -> Self {
        let mut p = Self::default();
        if let Some(v) = std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
            if let Some(b) = v.get("unified").and_then(|b| b.as_bool()) {
                p.unified = b;
            }
            if let Some(b) = v.get("collapse").and_then(|b| b.as_bool()) {
                p.collapse = b;
            }
        }
        p
    }

    fn save(&self) {
        let v = serde_json::json!({ "unified": self.unified, "collapse": self.collapse });
        let _ = std::fs::create_dir_all(brisk_db::config_dir());
        let _ = std::fs::write(Self::path(), v.to_string());
    }
}

/// One side's text, split into lines (tabs expanded), with syntax colors per line.
pub(crate) struct Side {
    pub(crate) lines: Vec<SharedString>,
    colors: Vec<Vec<(Range<usize>, Hsla)>>,
}

/// Tabs as 4 spaces, and where each original byte lands in the expanded line.
fn expand_tabs(line: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(line.len());
    let mut map = Vec::with_capacity(line.len() + 1);
    for c in line.chars() {
        for _ in 0..c.len_utf8() {
            map.push(out.len());
        }
        if c == '\t' {
            out.push_str("    ");
        } else {
            out.push(c);
        }
    }
    map.push(out.len());
    (out, map)
}

impl Side {
    pub(crate) fn new(text: &str, lang: Option<brisk_syntax::Lang>) -> Self {
        // Line starts in `text` ("\r\n" and "\n" both end a line, as in the line diff).
        let mut starts = Vec::new();
        let mut raw: Vec<&str> = Vec::new();
        let mut at = 0;
        for piece in text.split_inclusive('\n') {
            starts.push(at);
            at += piece.len();
            raw.push(piece.trim_end_matches('\n').trim_end_matches('\r'));
        }
        let spans = match lang {
            // Big files are shown without colors: parsing them here would stall the opening.
            Some(l) if text.len() <= 2_000_000 => {
                let mut t = brisk_syntax::SyntaxTree::new(l);
                t.parse(text);
                t.highlights(text, 0..text.len())
            }
            _ => Vec::new(),
        };
        let mut lines = Vec::with_capacity(raw.len());
        let mut colors = Vec::with_capacity(raw.len());
        let mut si = 0;
        for (i, l) in raw.iter().enumerate() {
            let (s, e) = (starts[i], starts[i] + l.len());
            let (exp, map) = expand_tabs(l);
            let mut cs = Vec::new();
            while si < spans.len() && spans[si].range.end <= s {
                si += 1;
            }
            let mut k = si;
            while k < spans.len() && spans[k].range.start < e {
                let r = &spans[k].range;
                let (a, b) = (r.start.max(s) - s, r.end.min(e) - s);
                if a < b {
                    cs.push((map[a]..map[b], Theme::syntax(spans[k].kind)));
                }
                k += 1;
            }
            lines.push(SharedString::from(exp));
            colors.push(cs);
        }
        Self { lines, colors }
    }

    pub(crate) fn line(&self, i: Option<usize>) -> (SharedString, &[(Range<usize>, Hsla)]) {
        match i.and_then(|i| self.lines.get(i).map(|l| (l.clone(), self.colors[i].as_slice()))) {
            Some(x) => x,
            None => (SharedString::default(), &[]),
        }
    }
}

/// Text runs for a line: syntax colors, plus a background on the changed words.
fn runs(len: usize, colors: &[(Range<usize>, Hsla)], words: &[Range<usize>], word_bg: Hsla) -> Vec<TextRun> {
    let mut cuts = vec![0, len];
    for (r, _) in colors {
        cuts.extend([r.start.min(len), r.end.min(len)]);
    }
    for r in words {
        cuts.extend([r.start.min(len), r.end.min(len)]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let font = gpui::font(mono_font());
    cuts.windows(2)
        .filter(|w| w[0] < w[1])
        .map(|w| {
            let a = w[0];
            let color = colors.iter().find(|(r, _)| r.start <= a && a < r.end).map(|(_, c)| *c).unwrap_or_else(Theme::text);
            let bg = words.iter().any(|r| r.start <= a && a < r.end).then_some(word_bg);
            TextRun { len: w[1] - a, font: font.clone(), color, background_color: bg, underline: None, strikethrough: None }
        })
        .collect()
}

pub(crate) fn line_text(text: SharedString, colors: &[(Range<usize>, Hsla)], words: &[Range<usize>], word_bg: Hsla) -> gpui::AnyElement {
    if text.is_empty() {
        return div().into_any_element();
    }
    let r = runs(text.len(), colors, words, word_bg);
    StyledText::new(text).with_runs(r).into_any_element()
}

/// A displayed row: a base row (aligned or unified, by mode) or a collapsed run of them.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Disp {
    Row(usize),
    Fold(Range<usize>),
}

/// The changed word ranges of a modified aligned row: (left, right).
type Words = Rc<(Vec<Range<usize>>, Vec<Range<usize>>)>;

/// A changed range on both sides (lines, end exclusive): the connector strip and scroll sync.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub l0: usize,
    pub l1: usize,
    pub r0: usize,
    pub r1: usize,
}

/// The change blocks of aligned rows.
pub fn blocks_of(rows: &[AlignedRow]) -> Vec<Block> {
    let mut out = Vec::new();
    let (mut lp, mut rp) = (0, 0);
    let mut i = 0;
    while i < rows.len() {
        if rows[i].kind == RowKind::Equal {
            lp = rows[i].left.map_or(lp, |l| l + 1);
            rp = rows[i].right.map_or(rp, |r| r + 1);
            i += 1;
            continue;
        }
        let mut b = Block { l0: lp, l1: lp, r0: rp, r1: rp };
        while i < rows.len() && rows[i].kind != RowKind::Equal {
            if let Some(l) = rows[i].left {
                b.l1 = l + 1;
                lp = l + 1;
            }
            if let Some(r) = rows[i].right {
                b.r1 = r + 1;
                rp = r + 1;
            }
            i += 1;
        }
        out.push(b);
    }
    out
}

/// The left line position beside right line position `r` (fractional): unchanged regions
/// line up, and a changed block is stretched over the other side's block.
pub fn right_to_left(blocks: &[Block], r: f32) -> f32 {
    let mut delta = 0.0;
    for b in blocks {
        if r < b.r0 as f32 {
            return r + delta;
        }
        if r < b.r1 as f32 {
            let f = (r - b.r0 as f32) / (b.r1 - b.r0) as f32;
            return b.l0 as f32 + f * (b.l1 - b.l0) as f32;
        }
        delta = b.l1 as f32 - b.r1 as f32;
    }
    r + delta
}

/// The editable right side of a working-copy diff.
struct Live {
    editor: Entity<Editor>,
    /// The view opened this editor (the file has no tab), so the view saves it.
    owned: bool,
    base: String,
    version: u64,
    /// Per left / right line: its aligned row.
    left_row: Vec<usize>,
    right_row: Vec<usize>,
    blocks: Vec<Block>,
    left_scroll: UniformListScrollHandle,
    _sub: Subscription,
}

pub struct DiffView {
    pub title: String,
    /// The working-tree file, for F4 / "open source".
    pub path: Option<PathBuf>,
    left_label: String,
    right_label: String,
    left: Side,
    right: Side,
    rows: Vec<AlignedRow>,
    unified_rows: Vec<UnifiedRow>,
    /// Word diffs of modified rows, filled when a row is first drawn.
    words: Vec<Option<Words>>,
    prefs: DiffPrefs,
    lang: Option<brisk_syntax::Lang>,
    live: Option<Live>,
    /// Folds the user opened (their first base row).
    expanded: HashSet<usize>,
    disp: Vec<Disp>,
    /// Displayed row of each change block's first line.
    blocks: Vec<usize>,
    current: usize,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
}

impl DiffView {
    /// (left label, right label, number of changes): for smoke tests.
    pub fn summary(&self) -> (String, String, usize) {
        (self.left_label.clone(), self.right_label.clone(), self.blocks.len())
    }

    /// Smoke checks: (unified, collapse, displayed rows, folded rows).
    pub fn layout_state(&self) -> (bool, bool, usize, usize) {
        let folded = self.disp.iter().map(|d| if let Disp::Fold(r) = d { r.len() } else { 0 }).sum();
        (self.prefs.unified, self.prefs.collapse, self.disp.len(), folded)
    }

    /// Smoke checks: the changed words of the `n`-th modified row, as text (left, right).
    pub fn words_of_change(&mut self, n: usize) -> Option<(Vec<String>, Vec<String>)> {
        let i = self.rows.iter().enumerate().filter(|(_, r)| r.kind == RowKind::Modified && r.left.is_some() && r.right.is_some()).nth(n)?.0;
        let w = self.words(i)?;
        let (l, _) = self.left.line(self.rows[i].left);
        let (r, _) = self.right.line(self.rows[i].right);
        Some((w.0.iter().map(|x| l[x.clone()].to_string()).collect(), w.1.iter().map(|x| r[x.clone()].to_string()).collect()))
    }

    pub fn new(title: String, path: Option<PathBuf>, left_label: String, left: &str, right_label: String, right: &str, cx: &mut Context<Self>) -> Self {
        let rows = brisk_git::align(left, right);
        let unified_rows = brisk_git::unified(&rows);
        let lang = path.as_deref().and_then(brisk_syntax::Lang::from_path);
        let words = vec![None; rows.len()];
        let mut v = Self {
            title,
            path,
            left_label,
            right_label,
            left: Side::new(left, lang),
            right: Side::new(right, lang),
            rows,
            unified_rows,
            words,
            prefs: DiffPrefs::load(),
            lang,
            live: None,
            expanded: HashSet::new(),
            disp: Vec::new(),
            blocks: Vec::new(),
            current: 0,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
        };
        v.rebuild();
        if let Some(&first) = v.blocks.first() {
            v.scroll.scroll_to_item(first.saturating_sub(3), gpui::ScrollStrategy::Top);
        }
        v
    }

    /// A live diff of the working copy: `editor` is the right side (edits re-align the diff).
    /// `owned`: the view opened the editor itself (no tab has the file) and saves it.
    #[allow(clippy::too_many_arguments)]
    pub fn new_live(title: String, path: Option<PathBuf>, left_label: String, base: String, right_label: String, editor: Entity<Editor>, owned: bool, cx: &mut Context<Self>) -> Self {
        let right = editor.read(cx).text_string();
        let mut v = Self::new(title, path, left_label, &base, right_label, &right, cx);
        if owned {
            let b = base.clone();
            editor.update(cx, |e, _| e.set_vcs_base(Some(b)));
        }
        let version = editor.read(cx).buffer.version();
        let sub = cx.observe(&editor, |this, ed, cx| this.editor_changed(ed, cx));
        v.live = Some(Live { editor, owned, base, version, left_row: Vec::new(), right_row: Vec::new(), blocks: Vec::new(), left_scroll: UniformListScrollHandle::new(), _sub: sub });
        v.index_live();
        v
    }

    /// The right side's editor in a live diff (keyboard actions go to it).
    pub fn live_editor(&self) -> Option<Entity<Editor>> {
        self.live.as_ref().map(|l| l.editor.clone())
    }

    /// The editor this view opened itself (saved with Save All and on close).
    pub fn owned_editor(&self) -> Option<Entity<Editor>> {
        self.live.as_ref().filter(|l| l.owned).map(|l| l.editor.clone())
    }

    fn editor_changed(&mut self, ed: Entity<Editor>, cx: &mut Context<Self>) {
        let v = ed.read(cx).buffer.version();
        let Some(live) = self.live.as_mut() else { return };
        if live.version != v {
            live.version = v;
            let base = live.base.clone();
            let text = ed.read(cx).text_string();
            self.rows = brisk_git::align(&base, &text);
            self.unified_rows = brisk_git::unified(&self.rows);
            self.words = vec![None; self.rows.len()];
            // The editor colors the right side itself; only the unified view needs colors.
            self.right = Side::new(&text, if self.prefs.unified { self.lang } else { None });
            self.index_live();
            self.rebuild();
        }
        cx.notify();
    }

    fn index_live(&mut self) {
        let Some(live) = self.live.as_mut() else { return };
        live.left_row = vec![0; self.left.lines.len()];
        live.right_row = vec![0; self.right.lines.len()];
        for (i, r) in self.rows.iter().enumerate() {
            if let Some(l) = r.left.filter(|l| *l < live.left_row.len()) {
                live.left_row[l] = i;
            }
            if let Some(x) = r.right.filter(|x| *x < live.right_row.len()) {
                live.right_row[x] = i;
            }
        }
        live.blocks = blocks_of(&self.rows);
    }

    fn base_len(&self) -> usize {
        if self.prefs.unified { self.unified_rows.len() } else { self.rows.len() }
    }

    fn base_equal(&self, i: usize) -> bool {
        if self.prefs.unified { self.unified_rows[i].kind == RowKind::Equal } else { self.rows[i].kind == RowKind::Equal }
    }

    /// Recompute the displayed rows (after a mode change or an expanded fold).
    fn rebuild(&mut self) {
        let n = self.base_len();
        let folds = if self.prefs.collapse { brisk_git::foldable(n, CONTEXT, |i| self.base_equal(i)) } else { Vec::new() };
        let mut disp = Vec::with_capacity(n);
        let mut i = 0;
        let mut fi = 0;
        while i < n {
            while fi < folds.len() && folds[fi].end <= i {
                fi += 1;
            }
            if fi < folds.len() && folds[fi].start == i && !self.expanded.contains(&i) {
                disp.push(Disp::Fold(folds[fi].clone()));
                i = folds[fi].end;
                continue;
            }
            disp.push(Disp::Row(i));
            i += 1;
        }
        let mut blocks = Vec::new();
        let mut prev_equal = true;
        for (d, x) in disp.iter().enumerate() {
            let eq = match x {
                Disp::Row(i) => self.base_equal(*i),
                Disp::Fold(_) => true,
            };
            if !eq && prev_equal {
                blocks.push(d);
            }
            prev_equal = eq;
        }
        self.disp = disp;
        self.blocks = blocks;
        self.current = self.current.min(self.blocks.len().saturating_sub(1));
    }

    pub fn set_unified(&mut self, unified: bool, cx: &mut Context<Self>) {
        if self.prefs.unified == unified {
            return;
        }
        self.prefs.unified = unified;
        self.prefs.save();
        if let Some(live) = self.live.as_ref() {
            let text = live.editor.read(cx).text_string();
            self.right = Side::new(&text, if unified { self.lang } else { None });
        }
        self.expanded.clear();
        self.rebuild();
        if let Some(&b) = self.blocks.get(self.current) {
            self.scroll.scroll_to_item(b.saturating_sub(3), gpui::ScrollStrategy::Top);
        }
        cx.notify();
    }

    pub fn set_collapse(&mut self, collapse: bool, cx: &mut Context<Self>) {
        self.prefs.collapse = collapse;
        self.prefs.save();
        self.expanded.clear();
        self.rebuild();
        if let Some(&b) = self.blocks.get(self.current) {
            self.scroll.scroll_to_item(b.saturating_sub(3), gpui::ScrollStrategy::Top);
        }
        cx.notify();
    }

    fn expand(&mut self, start: usize, cx: &mut Context<Self>) {
        self.expanded.insert(start);
        self.rebuild();
        cx.notify();
    }

    /// F7 / Shift+F7 in PyCharm's diff: jump between differences.
    pub fn jump(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.blocks.is_empty() {
            return;
        }
        let n = self.blocks.len();
        self.current = if forward { (self.current + 1) % n } else { (self.current + n - 1) % n };
        self.scroll.scroll_to_item(self.blocks[self.current].saturating_sub(3), gpui::ScrollStrategy::Top);
        if let Some(live) = self.live.as_ref().filter(|_| !self.prefs.unified) {
            if let Some(b) = live.blocks.get(self.current) {
                let line = b.r0;
                live.editor.update(cx, |e, cx| e.go_to_line(line, cx));
            }
        }
        cx.notify();
    }

    fn words(&mut self, i: usize) -> Option<Words> {
        let r = &self.rows[i];
        if r.kind != RowKind::Modified {
            return None;
        }
        let (Some(l), Some(rr)) = (r.left, r.right) else { return None };
        if self.words[i].is_none() {
            let w = brisk_git::inline_diff(&self.left.lines[l], &self.right.lines[rr]);
            self.words[i] = Some(Rc::new(w));
        }
        self.words[i].clone()
    }

    fn render_side_by_side(&mut self, i: usize) -> gpui::AnyElement {
        let r = self.rows[i].clone();
        let words = self.words(i);
        let (lbg, rbg) = match r.kind {
            RowKind::Equal => (Theme::editor_bg(), Theme::editor_bg()),
            RowKind::Added => (Theme::diff_filler_bg(), Theme::diff_added_bg()),
            RowKind::Deleted => (Theme::diff_deleted_bg(), Theme::diff_filler_bg()),
            RowKind::Modified => (
                if r.left.is_some() { Theme::diff_modified_bg() } else { Theme::diff_filler_bg() },
                if r.right.is_some() { Theme::diff_modified_bg() } else { Theme::diff_filler_bg() },
            ),
        };
        let (lt, lc) = self.left.line(r.left);
        let (rt, rc) = self.right.line(r.right);
        let none: &[Range<usize>] = &[];
        let lw = words.as_ref().map(|w| w.0.as_slice()).unwrap_or(none);
        let rw = words.as_ref().map(|w| w.1.as_slice()).unwrap_or(none);
        let changed = |j: usize| self.rows.get(j).is_some_and(|x| x.kind != RowKind::Equal);
        let edges = if r.kind == RowKind::Equal { (false, false) } else { (i == 0 || !changed(i - 1), !changed(i + 1)) };
        div()
            .flex()
            .w_full()
            .child(side(r.left, line_text(lt, lc, lw, Theme::diff_modified_word_bg()), lbg, edges))
            .child(div().w(px(1.)).h(px(line_height())).bg(Theme::divider()))
            .child(side(r.right, line_text(rt, rc, rw, Theme::diff_modified_word_bg()), rbg, edges))
            .into_any_element()
    }

    fn render_unified(&mut self, i: usize) -> gpui::AnyElement {
        let u = self.unified_rows[i].clone();
        let words = self.words(u.aligned);
        let none: &[Range<usize>] = &[];
        let (text, colors, w, bg, word_bg, sign) = match u.kind {
            RowKind::Deleted => {
                let (t, c) = self.left.line(u.old);
                (t, c, words.as_ref().map(|w| w.0.as_slice()).unwrap_or(none), Theme::diff_deleted_bg(), Theme::diff_deleted_word_bg(), "-")
            }
            RowKind::Added => {
                let (t, c) = self.right.line(u.new);
                (t, c, words.as_ref().map(|w| w.1.as_slice()).unwrap_or(none), Theme::diff_added_bg(), Theme::diff_added_word_bg(), "+")
            }
            _ => {
                let (t, c) = self.right.line(u.new);
                (t, c, none, Theme::editor_bg(), Theme::editor_bg(), "")
            }
        };
        let num = |n: Option<usize>| {
            div().w(px(44.)).flex_shrink_0().pr(px(8.)).flex().justify_end().text_color(Theme::gutter_fg()).child(n.map(|n| (n + 1).to_string()).unwrap_or_default())
        };
        let kind_at = |j: usize| self.unified_rows.get(j).map(|x| x.kind);
        let edges = if u.kind == RowKind::Equal { (false, false) } else { (i == 0 || kind_at(i - 1) != Some(u.kind), kind_at(i + 1) != Some(u.kind)) };
        div()
            .flex()
            .w_full()
            .h(px(line_height()))
            .items_center()
            .bg(bg)
            .when(edges.0, |d| d.rounded_t(px(4.)))
            .when(edges.1, |d| d.rounded_b(px(4.)))
            .child(num(u.old))
            .child(num(u.new))
            .child(div().w(px(16.)).flex_shrink_0().text_color(Theme::text_dim()).child(sign))
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(line_text(text, colors, w, word_bg)))
            .into_any_element()
    }

    fn render_fold(&self, d: usize, r: &Range<usize>, cx: &mut Context<Self>) -> gpui::AnyElement {
        let start = r.start;
        let n = r.len();
        div()
            .id(("diff-fold", d))
            .w_full()
            .h(px(line_height()))
            .flex()
            .items_center()
            .gap(px(6.))
            .pl(px(14.))
            .bg(Theme::diff_fold_bg())
            .text_color(Theme::text_muted())
            .font_family(crate::theme::ui_font())
            .text_size(px(12.))
            .cursor_pointer()
            .hover(|s| s.bg(Theme::hover_row()))
            .child(crate::icons::icon("unfold-vertical", 14., Theme::text_muted()))
            .child(format!("{n} unchanged line{}", if n == 1 { "" } else { "s" }))
            .on_click(cx.listener(move |this, _, _, cx| this.expand(start, cx)))
            .into_any_element()
    }
}

impl DiffView {
    /// Live side-by-side: the base text (no fillers) | connectors | the editor.
    fn render_live(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(live) = self.live.as_ref() else { return div().into_any_element() };
        let editor = live.editor.clone();
        let lh = line_height();
        let right_y = editor.read(cx).scroll_top();
        let left_y = right_to_left(&live.blocks, right_y / lh) * lh;
        live.left_scroll.0.borrow().base_handle.set_offset(point(px(0.), px(-left_y)));
        let blocks = live.blocks.clone();
        let left_scroll = live.left_scroll.clone();
        // The changed words of the right lines on screen (and a screen's worth around them).
        let top = (right_y / lh) as usize;
        let rows: Vec<usize> = live.right_row.iter().copied().skip(top.saturating_sub(50)).take(250).collect();
        let mut words = Vec::new();
        for i in rows {
            if let (Some(w), Some(r)) = (self.words(i), self.rows[i].right) {
                words.extend(w.1.iter().map(|x| (r, x.clone())));
            }
        }
        editor.update(cx, |e, _| {
            e.diff_bg = true;
            e.diff_words = words;
        });
        // Blank rows past the end: the editor scrolls its last line to the top, so must the left.
        let n = self.left.lines.len() + 150;
        let ed2 = editor.clone();
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id("diff-left-pane")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .on_scroll_wheel(move |ev, _, cx| {
                        let d = ev.delta.pixel_delta(px(lh));
                        ed2.update(cx, |e, cx| {
                            let y = e.scroll_top() - f32::from(d.y);
                            e.set_scroll_top(y, cx);
                        });
                        cx.stop_propagation();
                    })
                    .child(
                        uniform_list("diff-left", n, cx.processor(|this, range: Range<usize>, _, _| range.map(|l| this.render_live_left(l)).collect()))
                            .track_scroll(left_scroll)
                            .size_full()
                            .font_family(mono_font())
                            .text_size(px(font_size()))
                            .text_color(Theme::text()),
                    ),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let top = f32::from(bounds.top());
                        let (x0, x1) = (bounds.left(), bounds.right());
                        let h = f32::from(bounds.size.height);
                        let mid = (x0 + x1) / 2.;
                        for b in &blocks {
                            let ly0 = top + b.l0 as f32 * lh - left_y;
                            let ly1 = top + b.l1 as f32 * lh - left_y;
                            let ry0 = top + b.r0 as f32 * lh - right_y;
                            let ry1 = top + b.r1 as f32 * lh - right_y;
                            if ly1.max(ry1) < top || ly0.min(ry0) > top + h {
                                continue;
                            }
                            let color = if b.l0 == b.l1 {
                                Theme::diff_added_bg()
                            } else if b.r0 == b.r1 {
                                Theme::diff_deleted_bg()
                            } else {
                                Theme::diff_modified_bg()
                            };
                            // An empty side gets a 2 px band, so a pure insertion still shows.
                            let (ly1, ry1) = (ly1.max(ly0 + 2.), ry1.max(ry0 + 2.));
                            let mut pb = gpui::PathBuilder::fill();
                            pb.move_to(point(x0, px(ly0)));
                            pb.cubic_bezier_to(point(x1, px(ry0)), point(mid, px(ly0)), point(mid, px(ry0)));
                            pb.line_to(point(x1, px(ry1)));
                            pb.cubic_bezier_to(point(x0, px(ly1)), point(mid, px(ry1)), point(mid, px(ly1)));
                            pb.close();
                            if let Ok(path) = pb.build() {
                                window.paint_path(path, color);
                            }
                        }
                    },
                )
                .w(px(28.))
                .h_full()
                .flex_shrink_0(),
            )
            .child(div().flex_1().min_w_0().h_full().child(editor))
            .into_any_element()
    }

    fn render_live_left(&mut self, l: usize) -> gpui::AnyElement {
        if l >= self.left.lines.len() {
            return div().h(px(line_height())).into_any_element();
        }
        let row = self.live.as_ref().and_then(|lv| lv.left_row.get(l).copied()).unwrap_or(0);
        let kind = self.rows.get(row).map_or(RowKind::Equal, |r| r.kind);
        let words = self.words(row);
        let bg = match kind {
            RowKind::Deleted => Theme::diff_deleted_bg(),
            RowKind::Modified => Theme::diff_modified_bg(),
            _ => Theme::editor_bg(),
        };
        let (t, c) = self.left.line(Some(l));
        let none: &[Range<usize>] = &[];
        let w = words.as_ref().map(|w| w.0.as_slice()).unwrap_or(none);
        let changed = |j: usize| {
            let row = self.live.as_ref().and_then(|lv| lv.left_row.get(j).copied());
            row.and_then(|r| self.rows.get(r)).is_some_and(|x| x.kind != RowKind::Equal)
        };
        let edges = if kind == RowKind::Equal { (false, false) } else { (l == 0 || !changed(l - 1), !changed(l + 1)) };
        // As wide as the pane, with the editor's right margin.
        div().w_full().flex().pr(px(6.)).child(side(Some(l), line_text(t, c, w, Theme::diff_modified_word_bg()), bg, edges)).into_any_element()
    }
}

impl Focusable for DiffView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// One side of a row. `edges`: the row starts / ends its change block (rounded corners there).
pub(crate) fn side(num: Option<usize>, text: gpui::AnyElement, bg: Hsla, edges: (bool, bool)) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .h(px(line_height()))
        .flex()
        .items_center()
        .bg(bg)
        .when(edges.0, |d| d.rounded_t(px(4.)))
        .when(edges.1, |d| d.rounded_b(px(4.)))
        .child(
            div()
                .w(px(48.))
                .flex_shrink_0()
                .pr(px(10.))
                .flex()
                .justify_end()
                .text_color(Theme::gutter_fg())
                .child(num.map(|n| (n + 1).to_string()).unwrap_or_default()),
        )
        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(text))
}

/// A toolbar toggle: icon, tooltip, pressed state.
pub(crate) fn tool(id: &'static str, icon: &'static str, tip: &'static str, on: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .cursor_pointer()
        .when(on, |d| d.bg(Theme::selection_row()))
        .when(!on, |d| d.hover(|s| s.bg(Theme::hover_row())))
        .tooltip(move |_, cx| cx.new(|_| crate::workspace::Tip(tip.into())).into())
        .child(crate::icons::icon(icon, 15., if on { Theme::text() } else { Theme::text_muted() }))
}

impl Render for DiffView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let n = self.disp.len();
        let live = self.live.is_some() && !self.prefs.unified;
        let live_el = live.then(|| self.render_live(cx));
        let diffs = self.blocks.len();
        let unified = self.prefs.unified;
        let collapse = self.prefs.collapse;
        let labels = if unified {
            div().flex_1().px(px(12.)).child(format!("{}  →  {}", self.left_label, self.right_label)).into_any_element()
        } else {
            div()
                .flex_1()
                .flex()
                .child(div().flex_1().px(px(12.)).child(self.left_label.clone()))
                .child(div().flex_1().px(px(12.)).child(self.right_label.clone()))
                .into_any_element()
        };
        div()
            .track_focus(&self.focus)
            .key_context("DiffView")
            .size_full()
            .flex()
            .flex_col()
            .bg(Theme::editor_bg())
            .child(
                div()
                    .h(px(32.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(8.))
                    .bg(Theme::panel())
                    .border_b_1()
                    .border_color(Theme::divider())
                    .font_family(ui_font())
                    .text_size(px(12.))
                    .text_color(Theme::text_muted())
                    .child(tool("diff-prev", "arrow-up", "Previous Difference (Shift+F7)", false).on_click(cx.listener(|this, _, _, cx| this.jump(false, cx))))
                    .child(tool("diff-next", "arrow-down", "Next Difference (F7)", false).on_click(cx.listener(|this, _, _, cx| this.jump(true, cx))))
                    .child(div().w(px(1.)).h(px(16.)).mx(px(4.)).bg(Theme::divider()))
                    .child(tool("diff-sbs", "columns-2", "Side-by-side viewer", !unified).on_click(cx.listener(|this, _, _, cx| this.set_unified(false, cx))))
                    .child(tool("diff-uni", "align-justify", "Unified viewer", unified).on_click(cx.listener(|this, _, _, cx| this.set_unified(true, cx))))
                    .child(
                        tool("diff-collapse", "fold-vertical", "Collapse Unchanged Fragments", collapse)
                            .on_click(cx.listener(move |this, _, _, cx| this.set_collapse(!collapse, cx))),
                    )
                    .child(div().w(px(1.)).h(px(16.)).mx(px(4.)).bg(Theme::divider()))
                    .child(if diffs == 0 { "Contents are identical".to_string() } else { format!("{diffs} difference{}", if diffs == 1 { "" } else { "s" }) }),
            )
            .child(
                div()
                    .flex()
                    .h(px(24.))
                    .items_center()
                    .font_family(ui_font())
                    .text_size(px(12.))
                    .text_color(Theme::text_muted())
                    .border_b_1()
                    .border_color(Theme::divider())
                    .child(labels),
            )
            .children(live_el)
            .when(!live, |d| d.child(
                uniform_list(
                    "diff-rows",
                    n,
                    cx.processor(|this, range: Range<usize>, _, cx| {
                        range
                            .map(|d| match this.disp[d].clone() {
                                Disp::Fold(r) => this.render_fold(d, &r, cx),
                                Disp::Row(i) if this.prefs.unified => this.render_unified(i),
                                Disp::Row(i) => this.render_side_by_side(i),
                            })
                            .collect()
                    }),
                )
                .track_scroll(self.scroll.clone())
                .flex_1()
                .font_family(mono_font())
                .text_size(px(font_size()))
                .text_color(Theme::text()),
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_blocks_and_scroll_sync() {
        // base: a b c d e f; right: a X Y c d f (b -> X Y, e deleted)
        let rows = brisk_git::align("a\nb\nc\nd\ne\nf\n", "a\nX\nY\nc\nd\nf\n");
        let b = blocks_of(&rows);
        assert_eq!(b, vec![Block { l0: 1, l1: 2, r0: 1, r1: 3 }, Block { l0: 4, l1: 5, r0: 5, r1: 5 }]);
        assert_eq!(right_to_left(&b, 0.5), 0.5);
        assert_eq!(right_to_left(&b, 2.0), 1.5);
        assert_eq!(right_to_left(&b, 3.0), 2.0);
        assert_eq!(right_to_left(&b, 5.0), 5.0);
    }
}
