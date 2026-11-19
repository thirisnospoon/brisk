//! Code editor view. Renders only visible lines; highlighting is computed for the visible range.

use crate::theme::Theme;
use brisk_keymap::Action;
use brisk_lint::{Diagnostic, Fix, Severity};
use brisk_syntax::{Lang, SyntaxTree};
use brisk_text::{Buffer, Movement, Selection};
use gpui::{
    fill, font, point, px, size, App, Bounds, ClipboardItem, ContentMask, Context, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, ShapedLine,
    SharedString, Style, TextRun, UTF16Selection, Window,
};
use gpui::prelude::*;
use std::ops::Range;
use std::path::{Path, PathBuf};

pub mod annotate;
mod vcs_popup;
mod bookmarks;
pub mod breakpoints;
pub mod run_marks;
pub mod coverage;
mod select;
mod structure;
mod scope;
pub use scope::siblings as scope_siblings;
pub use select::{clipboard_history, remember_clip};
mod disk;
mod find;
mod fold;
mod marks;
mod lsp;
mod inlay;
mod column;
mod middle_click;
mod middle_scroll;
mod big;
mod extract;
mod local_sig;
mod postfix;
mod quick_def;
mod word_complete;
mod doc_view;
mod scroll;
mod motion;
mod sticky;
mod guides;
mod rounded;
mod stripe;
mod squiggle;
pub mod templates;
pub use disk::{Conflict, DiskCheck};
pub use lsp::Anchor;
pub use quick_def::definition_source;

/// PyCharm's default "Hard wrap at" for Python.
const RIGHT_MARGIN: usize = 120;

pub fn line_height() -> f32 {
    crate::settings::line_height()
}


struct LayoutCache {
    bounds: Bounds<Pixels>,
    /// Primary caret, window coordinates (for the F2 tooltip).
    caret_xy: Option<Point<Pixels>>,
    /// Left edge of the text column (unscrolled); `text_x` is where column 0 is drawn.
    text_left: Pixels,
    text_x: Pixels,
    /// Buffer lines shown, one per display row from the top (folds skip lines).
    rows: Vec<usize>,
    lines: Vec<ShapedLine>,
    char_w: Pixels,
}

impl LayoutCache {
    /// The shaped text of a buffer line, if it is on screen.
    fn shaped(&self, line: usize) -> Option<&ShapedLine> {
        self.rows.binary_search(&line).ok().map(|i| &self.lines[i])
    }
}

pub struct Editor {
    pub path: Option<PathBuf>,
    pub buffer: Buffer,
    syntax: Option<SyntaxTree>,
    parsed_version: u64,
    text_cache: (u64, String),
    focus: FocusHandle,
    scroll_y: f32,
    scroll_x: f32,
    /// Smooth scrolling and the caret's blink and slide (editor/motion.rs).
    motion: motion::Motion,
    /// Headers of the enclosing blocks pinned at the top (editor/sticky.rs).
    sticky: sticky::Sticky,
    /// The caret's block for the brighter indent guide (editor/guides.rs).
    guides: guides::Guides,
    /// Error stripe marks on the vertical scrollbar (editor/stripe.rs).
    stripe: stripe::Stripe,
    /// (buffer version, longest line in chars): the horizontal scroll range.
    max_line: (u64, usize),
    big: big::BigFile,
    occ: marks::Occurrences,
    /// A scrollbar thumb being dragged.
    drag: Option<scroll::Drag>,
    /// The scrollbar under the mouse.
    bar_hover: Option<scroll::Bar>,
    /// Middle-button scrolling in progress.
    middle: Option<middle_scroll::MiddleScroll>,
    /// Pointer position during a selection drag (drives auto-scroll past the edges).
    drag_pos: Option<Point<Pixels>>,
    autoscrolling: bool,
    /// Alt+drag column selection: the (line, column) where it started.
    column_anchor: Option<(usize, usize)>,
    /// Column selection mode (Shift+Alt+Insert) and its block: anchor and head (line, column).
    column_mode: bool,
    /// The right margin's column (PyCharm: 120, `.editorconfig` max_line_length); None: not drawn.
    pub right_margin: Option<usize>,
    column_block: Option<((usize, usize), (usize, usize))>,
    layout: Option<LayoutCache>,
    selecting: bool,
    marked: Option<Range<usize>>,
    pub read_error: Option<String>,
    /// File contents at HEAD (None: not in git / untracked). Drives the gutter change markers.
    vcs_base: Option<String>,
    vcs_changes: Vec<brisk_git::LineChange>,
    /// Set by a diff viewer before each render of this editor in it: changed lines get the
    /// diff backgrounds for that frame (the same editor in its own tab stays plain).
    pub diff_bg: bool,
    /// Set with `diff_bg`: changed words to highlight, as (line, byte range in the line).
    pub diff_words: Vec<(usize, Range<usize>)>,
    /// Set with `diff_bg`: change blocks to paint instead of the changes against HEAD (the
    /// merge window's result), as (lines, color); an empty range is a 2 px line.
    pub diff_blocks: Vec<(Range<usize>, gpui::Hsla)>,
    /// A file at a revision (Annotate Previous Revision): (path in the repository, commit).
    /// Read-only, no path (the tab shows `title_override`).
    pub revision: Option<(String, String)>,
    /// Run with Coverage marks.
    coverage: Option<coverage::CoverageMarks>,
    vcs_version: u64,
    /// False for small text fields (commit message): no line numbers.
    pub show_gutter: bool,
    /// ruff and ty problems merged, sorted by position, for buffer version `diag_version`.
    pub diagnostics: Vec<Diagnostic>,
    /// Each checker's latest results (ruff, ty).
    by_source: [Vec<Diagnostic>; 2],
    pub diag_version: u64,
    /// The buffer version last handed to ruff (the workspace schedules checks).
    pub lint_sent: u64,
    /// Problem under the mouse and where the mouse is.
    hover: Option<(usize, Point<Pixels>)>,
    /// Problem whose tooltip shows at the caret (after F2 / Shift+F2).
    caret_tip: Option<usize>,
    /// Completion, Quick Documentation, Parameter Info.
    insight: lsp::Insight,
    /// A live template / Surround With being filled in (Tab moves between its fields).
    tpl: Option<templates::TemplateRun>,
    inlays: inlay::Inlays,
    /// Ctrl+F / Ctrl+R bar.
    find: Option<find::FindBar>,
    /// The last search, for F3 after the bar is closed and for reopening it.
    last_find: find::FindOpts,
    last_replace: String,
    /// A one-line input (search fields): no find bar of its own, Enter is handled by the owner.
    pub is_field: bool,
    /// The tab title of a pathless editor ("df (value)"), instead of "untitled".
    pub title_override: Option<String>,
    /// Collapsed regions.
    folds: fold::Folds,
    /// F11 / Ctrl+F11 bookmarks.
    bookmarks: bookmarks::Bookmarks,
    breakpoints: breakpoints::Breakpoints,
    /// The debugger is paused on this line (its top frame, or the frame picked in Debug).
    pub exec_line: Option<usize>,
    /// Annotate with Git Blame: the column, and the hovered line.
    pub annotation: Option<annotate::Annotation>,
    ann_hover: Option<(usize, Point<Pixels>)>,
    /// The change marker popup: (change index, buffer version it was opened at).
    vcs_popup: Option<(usize, u64)>,
    sel_stack: select::SelStack,
    /// Ctrl+wheel distance not yet turned into a font size step.
    zoom_acc: f32,
    /// `.editorconfig` settings for this file.
    pub editorconfig: crate::editorconfig::EditorConfig,
    /// The mouse is over the gutter: show the fold markers.
    gutter_hover: bool,
    /// The run icon under the mouse (its line): drawn on a hover background.
    run_hover: Option<usize>,
    /// The file's encoding (kept on save).
    encoding: brisk_text::encoding::TextEncoding,
    /// A save had to switch the encoding to UTF-8 (shown once in the status bar).
    pub encoding_note: Option<String>,
    /// Modification time and size when last read or written.
    disk_stamp: disk::Stamp,
    /// The file changed or vanished under unsaved edits: a banner asks what to do.
    pub disk_conflict: Option<Conflict>,
    /// Run icons of the text: (buffer version, marks).
    run_cache: (u64, Vec<run_marks::RunMark>),
    /// pytest results of this file by node key, for the run icons.
    test_results: std::collections::HashMap<String, brisk_run::TestStatus>,
    /// The outline and the symbols around the caret, for the breadcrumbs.
    scope_cache: scope::ScopeCache,
}

/// Events for the workspace.
/// Who reported a problem.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiagSource {
    Ruff = 0,
    Ty = 1,
}

pub enum EditorEvent {
    /// The inspection widget (✗ 1 ⚠ 3) was clicked.
    ShowProblems,
    /// One character was typed (completion and parameter info triggers).
    Typed(char),
    /// The mouse rested on the word at this char position: show its documentation.
    HoverDoc(usize, Point<Pixels>),
    /// Ctrl+Click at this char position.
    CtrlClick(usize),
    /// Right click at this window position: show the context menu.
    ContextMenu(Point<Pixels>),
    /// A breakpoint was added, removed or changed.
    BreakpointsChanged,
    /// Right click on the breakpoint of this line: edit its condition.
    EditBreakpoint(usize),
    /// Right click on the line numbers: the gutter menu (Annotate with Git Blame).
    GutterMenu(Point<Pixels>),
    /// A click on a blame annotation: show this commit's diff of the file.
    ShowCommit(String),
    /// Show Diff in the change marker popup: the file's diff against HEAD.
    ShowVcsDiff,
    /// A click on the run icon of this line (window position for the menu).
    RunMark(usize, Point<Pixels>),
    /// The execution arrow was dragged to this line.
    SetNextStatement(usize),
}

impl gpui::EventEmitter<EditorEvent> for Editor {}

fn severity_color(s: Severity) -> gpui::Hsla {
    match s {
        Severity::Error => Theme::error(),
        Severity::Warning => Theme::warning_wave(),
        Severity::Unused => Theme::unused(),
    }
}

/// Recolor a byte range of a line's text runs (unused code is greyed out).
fn recolor(runs: Vec<TextRun>, range: Range<usize>, color: gpui::Hsla) -> Vec<TextRun> {
    let mut out = Vec::with_capacity(runs.len() + 2);
    let mut at = 0;
    for r in runs {
        let (s, e) = (at, at + r.len);
        at = e;
        let (a, b) = (range.start.clamp(s, e), range.end.clamp(s, e));
        if a >= b {
            out.push(r);
            continue;
        }
        if a > s {
            out.push(TextRun { len: a - s, ..r.clone() });
        }
        out.push(TextRun { len: b - a, color, ..r.clone() });
        if e > b {
            out.push(TextRun { len: e - b, ..r });
        }
    }
    out
}

fn char_to_byte(s: &str, col: usize) -> usize {
    s.char_indices().nth(col).map(|(b, _)| b).unwrap_or(s.len())
}

fn byte_to_char(s: &str, byte: usize) -> usize {
    s[..byte.min(s.len())].chars().count()
}

impl Editor {
    pub fn open(path: &Path, cx: &mut Context<Self>) -> Self {
        let (text, encoding, err) = disk::load(path);
        let mut buffer = Buffer::new(&text);
        // Very large files: plain text (a full reparse per keystroke would lag).
        let lang = Lang::from_path(path).filter(|_| text.len() <= 3 * 1024 * 1024);
        if let Some(l) = lang {
            buffer.line_comment = l.line_comment().map(Into::into);
            buffer.python = l == Lang::Python;
        } else {
            // Line comment prefix for common non-Python files.
            buffer.line_comment = match path.extension().and_then(|e| e.to_str()) {
                Some("js" | "ts" | "tsx" | "jsx" | "rs" | "go" | "java" | "c" | "cpp" | "h" | "css" | "scss") => Some("//".into()),
                Some("toml" | "yaml" | "yml" | "sh" | "cfg" | "ini" | "txt" | "dockerfile") => Some("#".into()),
                Some("sql") => Some("--".into()),
                _ => Some("#".into()),
            };
        }
        if err.is_none() {
            crate::local_history::record(path, &text.replace("\r\n", "\n"), crate::local_history::Label::Opened);
        }
        // Indent and line endings: .editorconfig, else what the file already uses.
        let editorconfig = crate::editorconfig::for_file(path);
        buffer.indent_unit = editorconfig.indent_unit().or_else(|| crate::editorconfig::detect_indent(&text)).unwrap_or_else(|| "    ".into());
        if let Some(crlf) = editorconfig.crlf {
            buffer.line_ending = if crlf { brisk_text::LineEnding::CrLf } else { brisk_text::LineEnding::Lf };
        }
        let right_margin = editorconfig.max_line_length.unwrap_or(Some(RIGHT_MARGIN));
        let mut ed = Self {
            path: Some(path.to_path_buf()),
            buffer,
            syntax: lang.map(SyntaxTree::new),
            parsed_version: u64::MAX,
            text_cache: (u64::MAX, String::new()),
            focus: cx.focus_handle(),
            scroll_y: 0.0,
            scroll_x: 0.0,
            motion: Default::default(),
            sticky: Default::default(),
            guides: Default::default(),
            stripe: Default::default(),
            max_line: (u64::MAX, 0),
            big: Default::default(),
            occ: Default::default(),
            drag: None,
            bar_hover: None,
            middle: None,
            drag_pos: None,
            autoscrolling: false,
            column_anchor: None,
            column_mode: false,
            right_margin,
            column_block: None,
            layout: None,
            selecting: false,
            marked: None,
            read_error: err,
            vcs_base: None,
            vcs_changes: Vec::new(),
            diff_bg: false,
            diff_words: Vec::new(),
            diff_blocks: Vec::new(),
            revision: None,
            coverage: None,
            vcs_version: u64::MAX,
            show_gutter: true,
            diagnostics: Vec::new(),
            by_source: [Vec::new(), Vec::new()],
            diag_version: u64::MAX,
            lint_sent: u64::MAX,
            hover: None,
            caret_tip: None,
            insight: lsp::Insight::default(),
            tpl: None,
            inlays: Default::default(),
            find: None,
            last_find: find::FindOpts::default(),
            last_replace: String::new(),
            is_field: false,
            title_override: None,
            folds: Default::default(),
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            exec_line: None,
            annotation: None,
            ann_hover: None,
            vcs_popup: None,
            sel_stack: Default::default(),
            zoom_acc: 0.0,
            editorconfig,
            gutter_hover: false,
            run_hover: None,
            encoding,
            encoding_note: None,
            disk_stamp: disk::stamp(path),
            disk_conflict: None,
            run_cache: (u64::MAX, Vec::new()),
            test_results: Default::default(),
            scope_cache: Default::default(),
        };
        ed.reparse();
        ed
    }

    /// A pathless editor for text fields such as the commit message.
    pub fn scratch(text: &str, cx: &mut Context<Self>) -> Self {
        let mut buffer = Buffer::new(text);
        buffer.line_comment = None;
        Self {
            path: None,
            buffer,
            syntax: None,
            parsed_version: u64::MAX,
            text_cache: (u64::MAX, String::new()),
            focus: cx.focus_handle(),
            scroll_y: 0.0,
            scroll_x: 0.0,
            motion: Default::default(),
            sticky: Default::default(),
            guides: Default::default(),
            stripe: Default::default(),
            max_line: (u64::MAX, 0),
            big: Default::default(),
            occ: Default::default(),
            drag: None,
            bar_hover: None,
            middle: None,
            drag_pos: None,
            autoscrolling: false,
            column_anchor: None,
            column_mode: false,
            right_margin: None,
            column_block: None,
            layout: None,
            selecting: false,
            marked: None,
            read_error: None,
            vcs_base: None,
            vcs_changes: Vec::new(),
            diff_bg: false,
            diff_words: Vec::new(),
            diff_blocks: Vec::new(),
            revision: None,
            coverage: None,
            vcs_version: u64::MAX,
            show_gutter: false,
            diagnostics: Vec::new(),
            by_source: [Vec::new(), Vec::new()],
            diag_version: u64::MAX,
            lint_sent: u64::MAX,
            hover: None,
            caret_tip: None,
            insight: lsp::Insight::default(),
            tpl: None,
            inlays: Default::default(),
            find: None,
            last_find: find::FindOpts::default(),
            last_replace: String::new(),
            is_field: false,
            title_override: None,
            folds: Default::default(),
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            exec_line: None,
            annotation: None,
            ann_hover: None,
            vcs_popup: None,
            sel_stack: Default::default(),
            zoom_acc: 0.0,
            editorconfig: Default::default(),
            gutter_hover: false,
            run_hover: None,
            encoding: Default::default(),
            encoding_note: None,
            disk_stamp: None,
            disk_conflict: None,
            run_cache: (u64::MAX, Vec::new()),
            test_results: Default::default(),
            scope_cache: Default::default(),
        }
    }

    pub fn set_vcs_base(&mut self, base: Option<String>) {
        if self.vcs_base != base {
            self.vcs_base = base;
            self.vcs_version = u64::MAX;
            if self.vcs_base.is_none() {
                self.vcs_changes.clear();
            }
        }
    }

    fn refresh_vcs(&mut self) {
        let v = self.buffer.version();
        if self.vcs_version == v {
            return;
        }
        self.vcs_version = v;
        let Some(base) = self.vcs_base.clone() else { return };
        if self.buffer.len_lines() > 20_000 {
            self.vcs_changes.clear();
            return;
        }
        let text = self.text().to_string();
        self.vcs_changes = brisk_git::line_changes(&base, &text);
    }

    pub fn text_string(&self) -> String {
        self.buffer.text()
    }

    /// Vertical scroll in pixels, and the line height it is measured in (diff sync).
    pub fn scroll_top(&self) -> f32 {
        self.scroll_y
    }

    pub fn set_scroll_top(&mut self, y: f32, cx: &mut Context<Self>) {
        let y = y.clamp(0.0, self.max_scroll_y().max(0.0));
        if (y - self.scroll_y).abs() > 0.01 {
            self.scroll_y = y;
            self.motion.glide = None;
            cx.notify();
        }
    }

    pub fn is_sql(&self) -> bool {
        self.path.as_ref().and_then(|p| p.extension()).is_some_and(|e| e.eq_ignore_ascii_case("sql"))
    }

    /// What Ctrl+Enter in a SQL console runs: the selection, else the statement at the caret.
    /// Returns `(text, byte offset of the caret)`; the caller picks the statement.
    pub fn sql_to_run(&mut self) -> (Option<String>, String, usize) {
        let sel = self.buffer.selected_text();
        let text = self.text().to_string();
        let head = self.buffer.primary().head;
        let byte = self.buffer.rope().char_to_byte(head.min(self.buffer.rope().len_chars()));
        ((!sel.trim().is_empty()).then_some(sel), text, byte)
    }

    /// Append text at the end and put the caret there (used to seed a new console).
    pub fn set_initial_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.buffer.text().trim().is_empty() {
            self.buffer.set_text(text);
            let end = self.buffer.rope().len_chars();
            self.buffer.set_caret(end, false);
            cx.notify();
        }
    }

    /// Highlighting for a pathless editor (the Python Console input).
    pub fn set_language(&mut self, lang: Option<Lang>) {
        self.syntax = lang.map(SyntaxTree::new);
        self.parsed_version = u64::MAX;
    }

    pub fn title(&self) -> String {
        if let Some(t) = &self.title_override {
            return t.clone();
        }
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".into())
    }

    fn text(&mut self) -> &str {
        let v = self.buffer.version();
        if self.text_cache.0 != v {
            self.text_cache = (v, self.buffer.text());
        }
        &self.text_cache.1
    }

    fn reparse(&mut self) {
        let v = self.buffer.version();
        if self.parsed_version == v {
            return;
        }
        self.text();
        let large = self.is_big();
        if let Some(s) = &mut self.syntax {
            // A large file with a tree: only shift the tree on this thread, the parse runs in
            // the background (`background_parse`). Even an incremental parse there takes
            // tens of ms while a statement is half typed (error recovery).
            if large && s.tree().is_some() {
                s.shift(&self.text_cache.1);
            } else {
                s.parse_within(&self.text_cache.1, large.then_some(big::PARSE_BUDGET));
            }
        }
        self.parsed_version = v;
    }

    pub fn outline(&mut self) -> Vec<brisk_syntax::OutlineItem> {
        self.reparse();
        let text = self.text().to_string();
        self.syntax.as_ref().map(|s| s.outline(&text)).unwrap_or_default()
    }

    pub fn is_python(&self) -> bool {
        self.read_error.is_none() && matches!(self.syntax.as_ref().map(|s| s.lang()), Some(Lang::Python))
    }

    /// Text and version for a ruff check, when it changed since the last one.
    pub fn take_lint_request(&mut self) -> Option<(u64, String)> {
        let v = self.buffer.version();
        if !self.is_python() || self.lint_sent == v {
            return None;
        }
        self.lint_sent = v;
        Some((v, self.text().to_string()))
    }

    /// Results for `version`; ignored if the text changed meanwhile (a newer check follows).
    pub fn set_diagnostics(&mut self, source: DiagSource, version: u64, diags: Vec<Diagnostic>) -> bool {
        if version != self.buffer.version() {
            return false;
        }
        self.by_source[source as usize] = diags;
        // ty repeats some ruff findings (undefined names): keep ruff's, it has fixes and noqa.
        // A graver ty finding on the same span stays (`import-not-found` under ruff's F401).
        let ruff = &self.by_source[0];
        let mut all = ruff.clone();
        all.extend(self.by_source[1].iter().filter(|t| !ruff.iter().any(|r| r.start == t.start && r.end == t.end && r.severity <= t.severity)).cloned());
        all.sort_by_key(|d| (d.start, d.severity));
        self.diagnostics = all;
        self.diag_version = version;
        self.hover = None;
        self.caret_tip = None;
        true
    }

    /// (errors, warnings including weak ones).
    pub fn problem_counts(&self) -> (usize, usize) {
        let e = self.diagnostics.iter().filter(|d| d.severity == Severity::Error).count();
        (e, self.diagnostics.len() - e)
    }

    fn pos_of(&self, p: brisk_lint::Pos) -> usize {
        if p.line >= self.buffer.len_lines() {
            self.buffer.len_chars()
        } else {
            self.buffer.line_col_to_pos(p.line, p.col)
        }
    }

    /// Char range a problem covers; at least one character so it can be seen and hovered.
    fn diag_range(&self, d: &Diagnostic) -> Range<usize> {
        let s = self.pos_of(d.start);
        let e = self.pos_of(d.end).max(s);
        if e > s {
            s..e
        } else {
            s..(s + 1).min(self.buffer.len_chars().max(s))
        }
    }

    fn diag_at(&self, pos: usize) -> Option<usize> {
        // Errors win over warnings when they overlap.
        let mut hits: Vec<usize> = (0..self.diagnostics.len()).filter(|&i| {
            let r = self.diag_range(&self.diagnostics[i]);
            r.contains(&pos) || r.start == pos
        }).collect();
        hits.sort_by_key(|&i| self.diagnostics[i].severity);
        hits.first().copied()
    }

    /// F2 / Shift+F2: next / previous problem, errors first like PyCharm ("Go to high
    /// priority problems only" when there are any). Shows its tooltip at the caret.
    pub fn goto_problem(&mut self, forward: bool, cx: &mut Context<Self>) -> Option<String> {
        let any_err = self.diagnostics.iter().any(|d| d.severity == Severity::Error);
        let cands: Vec<(usize, usize)> = self
            .diagnostics
            .iter()
            .enumerate()
            .filter(|(_, d)| !any_err || d.severity == Severity::Error)
            .map(|(i, d)| (self.pos_of(d.start), i))
            .collect();
        if cands.is_empty() {
            return None;
        }
        let caret = self.buffer.primary().head;
        let pick = if forward {
            cands.iter().find(|(p, _)| *p > caret).or(cands.first())
        } else {
            cands.iter().rev().find(|(p, _)| *p < caret).or(cands.last())
        };
        let &(pos, i) = pick?;
        self.buffer.set_caret(pos, false);
        self.ensure_caret_visible();
        self.caret_tip = Some(i);
        cx.notify();
        Some(self.diagnostics[i].title())
    }

    /// Problems Alt+Enter offers fixes for: the one at the caret, else those on its line.
    pub fn problems_at_caret(&self) -> Vec<Diagnostic> {
        let caret = self.buffer.primary().head;
        if let Some(i) = self.diag_at(caret) {
            return vec![self.diagnostics[i].clone()];
        }
        let (line, _) = self.caret_line_col();
        self.diagnostics.iter().filter(|d| d.start.line <= line && line <= d.end.line).cloned().collect()
    }

    /// The problem at the caret, for the status bar.
    pub fn caret_problem(&self) -> Option<String> {
        self.diag_at(self.buffer.primary().head).map(|i| self.diagnostics[i].title())
    }

    /// Apply a ruff quick fix as one undoable edit.
    pub fn apply_fix(&mut self, fix: &Fix, cx: &mut Context<Self>) {
        let edits = fix.edits.iter().map(|e| (self.pos_of(e.start)..self.pos_of(e.end), e.content.clone())).collect();
        self.buffer.replace_ranges(edits);
        self.caret_tip = None;
        cx.notify();
    }

    /// "Suppress for line": add `# noqa: CODE` on the problem's line.
    pub fn suppress(&mut self, d: &Diagnostic, cx: &mut Context<Self>) {
        let Some(line) = d.noqa_line.filter(|&l| l < self.buffer.len_lines()) else { return };
        let old = self.buffer.line(line);
        let new = if brisk_lint::is_ty_code(&d.code) { brisk_lint::ty_ignore_line(&old, &d.code) } else { brisk_lint::noqa_line(&old, &d.code) };
        let start = self.buffer.line_start(line);
        self.buffer.replace_ranges(vec![(start..start + old.chars().count(), new)]);
        cx.notify();
    }

    /// Replace the whole text (ruff `--fix` output) as one undoable step.
    /// Replace the text with a new version (formatter, quick fixes) as one undoable edit of
    /// only the part that differs, so the caret, folds and scroll position stay where they were.
    pub fn replace_all_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.replace_all_quiet(text);
        cx.notify();
    }

    /// The same without a repaint: one undoable minimal edit, the caret keeps its line and column.
    pub(crate) fn replace_all_quiet(&mut self, text: &str) {
        let new = text.replace("\r\n", "\n");
        let old = self.text().to_string();
        if new == old {
            return;
        }
        let (line, col) = self.caret_line_col();
        let prefix = old.chars().zip(new.chars()).take_while(|(a, b)| a == b).count();
        let (old_n, new_n) = (old.chars().count(), new.chars().count());
        let max_suffix = old_n.min(new_n) - prefix;
        let suffix = old.chars().rev().zip(new.chars().rev()).take(max_suffix).take_while(|(a, b)| a == b).count();
        let middle: String = new.chars().skip(prefix).take(new_n - prefix - suffix).collect();
        self.buffer.replace_ranges(vec![(prefix..old_n - suffix, middle)]);
        let line = line.min(self.buffer.len_lines().saturating_sub(1));
        let col = col.min(self.buffer.line_len(line));
        let pos = self.buffer.line_col_to_pos(line, col);
        self.buffer.set_caret(pos, false);
    }

    /// "4 spaces" / "2 spaces" / "Tab" for the status bar.
    pub fn indent_label(&self) -> String {
        match self.buffer.indent_unit.as_str() {
            "\t" => "Tab".into(),
            u => format!("{} spaces", u.chars().count()),
        }
    }

    pub fn caret_line_col(&self) -> (usize, usize) {
        self.buffer.pos_to_line_col(self.buffer.primary().head)
    }

    pub fn language_name(&self) -> &'static str {
        match self.syntax.as_ref().map(|s| s.lang()) {
            Some(l) => l.name(),
            None => match self.path.as_ref().and_then(|p| p.extension()).and_then(|e| e.to_str()) {
                Some("md") => "Markdown",
                Some("toml") => "TOML",
                Some("json") => "JSON",
                Some("js" | "jsx") => "JavaScript",
                Some("ts" | "tsx") => "TypeScript",
                Some("css") => "CSS",
                Some("html") => "HTML",
                Some("yaml" | "yml") => "YAML",
                Some("sql") => "SQL",
                _ => "Plain text",
            },
        }
    }

    pub fn go_to_line(&mut self, line: usize, cx: &mut Context<Self>) {
        let pos = self.buffer.line_col_to_pos(line, 0);
        let ws = self.buffer.line(line.min(self.buffer.len_lines().saturating_sub(1))).chars().take_while(|c| c.is_whitespace()).count();
        self.buffer.set_caret(pos + ws, false);
        self.center_caret();
        cx.notify();
    }

    /// Select `cols` (chars) on `line` and scroll it into view — navigating to a search result.
    pub fn select_on_line(&mut self, line: usize, cols: Range<usize>, cx: &mut Context<Self>) {
        let anchor = self.buffer.line_col_to_pos(line, cols.start);
        let head = self.buffer.line_col_to_pos(line, cols.end);
        self.buffer.selections = vec![Selection { anchor, head, goal_col: None }];
        self.center_caret();
        cx.notify();
    }

    /// Text to prefill search fields with: a single-line selection.
    pub fn search_seed(&self) -> Option<String> {
        let t = self.buffer.selected_text();
        (!t.is_empty() && !t.contains('\n')).then_some(t)
    }

    fn ensure_caret_visible(&mut self) {
        self.sync_folds();
        let shown = self.begin_scroll();
        let (line, _) = self.caret_line_col();
        let vh = self.layout.as_ref().map(|l| f32::from(l.bounds.size.height)).unwrap_or(600.0);
        let y = self.row_of(line) as f32 * line_height();
        let margin = line_height() * 3.0;
        if y < self.scroll_y + margin {
            self.scroll_y = (y - margin).max(0.0);
        } else if y + line_height() > self.scroll_y + vh - margin {
            self.scroll_y = y + line_height() - vh + margin;
        }
        self.clamp_scroll();
        self.end_scroll(shown, Self::jump_duration());
        self.ensure_caret_visible_x();
    }

    fn center_caret(&mut self) {
        self.sync_folds();
        let (line, _) = self.caret_line_col();
        let vh = self.layout.as_ref().map(|l| f32::from(l.bounds.size.height)).unwrap_or(600.0);
        let shown = self.begin_scroll();
        self.scroll_y = (self.row_of(line) as f32 * line_height() - vh / 3.0).max(0.0);
        self.clamp_scroll();
        self.end_scroll(shown, Self::jump_duration());
    }

    fn clamp_scroll(&mut self) {
        let max = (self.display_rows().saturating_sub(1)) as f32 * line_height();
        self.scroll_y = self.scroll_y.clamp(0.0, max.max(0.0));
        self.clamp_scroll_x();
    }

    /// Smoke checks: (scroll x, scroll y, max x, max y) and the text area's bounds.
    pub fn scroll_state(&self) -> (f32, f32, f32, f32, Option<Bounds<Pixels>>) {
        (self.scroll_x, self.scroll_y, self.max_scroll_x(), self.max_scroll_y(), self.layout.as_ref().map(|l| l.bounds))
    }

    /// Execute an editor action. Returns false if this action isn't an editor action.
    pub fn perform(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) -> bool {
        use Action as A;
        if !self.is_field {
            match action {
                A::Find => {
                    self.open_find(false, window, cx);
                    return true;
                }
                A::Replace => {
                    self.open_find(true, window, cx);
                    return true;
                }
                A::FindNext | A::FindPrevious => {
                    self.find_next(action == A::FindNext, window, cx);
                    return true;
                }
                _ => {}
            }
        }
        if self.read_error.is_some() && !matches!(action, A::Copy | A::SelectAll | A::MoveUp | A::MoveDown | A::MoveLeft | A::MoveRight | A::PageUp | A::PageDown | A::MoveDocStart | A::MoveDocEnd) {
            return true;
        }
        self.caret_tip = None;
        self.insight.doc = None;
        if action == A::EditorEscape && self.vcs_popup.take().is_some() {
            cx.notify();
            return true;
        }
        if action == A::EditorEscape && self.close_insight() {
            cx.notify();
            return true;
        }
        // Esc in the editor closes the search bar, after popups and before multi-carets.
        if action == A::EditorEscape && self.find.is_some() && self.buffer.selections.len() == 1 {
            self.close_find(window, cx);
            return true;
        }
        if !matches!(action, A::EditorBackspace) {
            self.insight.completion = None;
        }
        if !matches!(action, A::EditorBackspace | A::EditorDelete | A::MoveLeft | A::MoveRight | A::SelectLeft | A::SelectRight | A::MoveWordLeft | A::MoveWordRight) {
            self.insight.signature = None;
        }
        if action == A::ToggleColumnSelectionMode && !self.is_field {
            self.toggle_column_mode();
            cx.notify();
            return true;
        }
        if self.column_key(action) {
            self.ensure_caret_visible();
            cx.notify();
            return true;
        }
        self.column_block = None;
        match action {
            // Live templates, block comments (editor/templates.rs).
            A::EditorTab if !self.is_field && self.template_next(true, cx) => return true,
            A::EditorUnindent if !self.is_field && self.template_next(false, cx) => return true,
            A::EditorEscape if self.end_template() => return true,
            A::EditorNewline | A::StartNewLine | A::StartNewLineBefore if !self.is_field => {
                // Enter finishes a template being filled in; the key still does its job.
                self.end_template();
            }
            A::EditorTab if !self.is_field && !self.completion_open() && self.expand_live_template(cx) => return true,
            A::CommentBlock if !self.is_field => {
                self.toggle_block_comment(cx);
                return true;
            }
            A::CommentLine if !self.is_field && self.toggle_line_block_comment(cx) => return true,
            A::NextMethod | A::PreviousMethod if !self.is_field => {
                self.goto_method(action == A::NextMethod, cx);
                return true;
            }
            A::ExpandRegion | A::CollapseRegion | A::ExpandAll | A::CollapseAll if !self.is_field => {
                match action {
                    A::ExpandRegion => self.expand_at_caret(),
                    A::CollapseRegion => self.collapse_at_caret(),
                    A::ExpandAll => self.expand_all(),
                    _ => self.collapse_all(),
                }
                self.clamp_scroll();
                cx.notify();
                return true;
            }
            _ => {}
        }
        let b = &mut self.buffer;
        let page = self.layout.as_ref().map(|l| (f32::from(l.bounds.size.height) / line_height()) as usize).unwrap_or(30).max(1);
        let mut keep_scroll = false;
        match action {
            A::MoveLeft => b.move_carets(Movement::Left, false),
            A::MoveRight => b.move_carets(Movement::Right, false),
            A::MoveUp => b.move_carets(Movement::Up, false),
            A::MoveDown => b.move_carets(Movement::Down, false),
            A::MoveWordLeft => b.move_carets(Movement::WordLeft, false),
            A::MoveWordRight => b.move_carets(Movement::WordRight, false),
            A::MoveLineStart => b.move_carets(Movement::LineStart, false),
            A::MoveLineEnd => b.move_carets(Movement::LineEnd, false),
            A::MoveDocStart => b.move_carets(Movement::DocStart, false),
            A::MoveDocEnd => b.move_carets(Movement::DocEnd, false),
            A::PageUp => b.move_carets(Movement::PageUp(page), false),
            A::PageDown => b.move_carets(Movement::PageDown(page), false),
            A::SelectLeft => b.move_carets(Movement::Left, true),
            A::SelectRight => b.move_carets(Movement::Right, true),
            A::SelectUp => b.move_carets(Movement::Up, true),
            A::SelectDown => b.move_carets(Movement::Down, true),
            A::SelectWordLeft => b.move_carets(Movement::WordLeft, true),
            A::SelectWordRight => b.move_carets(Movement::WordRight, true),
            A::SelectLineStart => b.move_carets(Movement::LineStart, true),
            A::SelectLineEnd => b.move_carets(Movement::LineEnd, true),
            A::SelectDocStart => b.move_carets(Movement::DocStart, true),
            A::SelectDocEnd => b.move_carets(Movement::DocEnd, true),
            A::ScrollUp => {
                self.scroll_y -= line_height();
                self.clamp_scroll();
                keep_scroll = true;
            }
            A::ScrollDown => {
                self.scroll_y += line_height();
                self.clamp_scroll();
                keep_scroll = true;
            }
            A::EditorNewline => b.newline(),
            A::StartNewLine => b.start_new_line(false),
            A::StartNewLineBefore => b.start_new_line(true),
            A::EditorBackspace => b.backspace(),
            A::EditorDelete => b.delete_forward(),
            A::EditorDeleteWordBack => b.delete_word(false),
            A::EditorDeleteWordForward => b.delete_word(true),
            A::EditorTab => b.indent(),
            A::EditorUnindent => b.unindent(),
            A::EditorEscape => b.collapse_selections(),
            A::Undo => {
                b.undo();
            }
            A::Redo => {
                b.redo();
            }
            A::SelectAll => {
                b.select_all();
                keep_scroll = true;
            }
            A::DuplicateLine => b.duplicate(),
            A::DeleteLine => b.delete_lines(),
            A::JoinLines => b.join_lines(),
            A::ToggleCase => b.toggle_case(),
            A::MoveLineUp => b.move_lines(true),
            A::MoveLineDown => b.move_lines(false),
            A::MoveStatementUp | A::MoveStatementDown => self.move_statement(action == A::MoveStatementUp),
            A::CommentLine => b.toggle_line_comment(),
            A::AddCaretNextOccurrence => b.add_next_occurrence(),
            A::SelectAllOccurrences => b.select_all_occurrences(),
            A::UnselectOccurrence => {
                if b.selections.len() > 1 {
                    b.selections.pop();
                }
            }
            A::Copy | A::Cut => {
                // PyCharm: with no selection, copy/cut the whole line.
                let whole_line = b.selections.iter().all(|s| s.is_empty());
                let text = if whole_line {
                    // Every caret's line (each once), so pasting into as many carets spreads them.
                    b.caret_lines().iter().map(|&l| format!("{}\n", b.line(l))).collect()
                } else {
                    b.selected_text()
                };
                remember_clip(&text);
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                if action == A::Cut {
                    if whole_line {
                        b.delete_lines();
                    } else {
                        b.insert("");
                    }
                }
            }
            A::Paste => {
                if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    let t = t.replace("\r\n", "\n");
                    remember_clip(&t);
                    let line_paste = t.ends_with('\n') && t.matches('\n').count() == 1 && b.selections.iter().all(|s| s.is_empty());
                    // One line per caret (a multi-caret copy): each caret gets its own line.
                    let pieces: Vec<String> = t.strip_suffix('\n').unwrap_or(&t).split('\n').map(str::to_string).collect();
                    if b.selections.len() > 1 && pieces.len() == b.selections.len() {
                        if t.ends_with('\n') && b.selections.iter().all(|s| s.is_empty()) {
                            // Whole lines from a multi-caret line copy: each above its caret's line.
                            let starts: Vec<usize> = b.selections.iter().map(|s| b.line_start(b.pos_to_line_col(s.head).0)).collect();
                            b.selections = starts.into_iter().map(Selection::caret).collect();
                            let lines: Vec<String> = pieces.iter().map(|p| format!("{p}\n")).collect();
                            b.insert_each(&lines);
                        } else {
                            b.insert_each(&pieces);
                        }
                    } else if line_paste {
                        // Whole-line paste goes above the current line, like PyCharm.
                        let carets: Vec<usize> = b.selections.iter().map(|s| s.head).collect();
                        b.selections = carets.iter().map(|&p| { let (l, _) = b.pos_to_line_col(p); Selection::caret(b.line_start(l)) }).collect();
                        b.insert(&t);
                    } else {
                        // Python: a pasted block takes the indent of where it goes.
                        b.paste_python(&t);
                    }
                }
            }
            A::ExtendSelection => {
                self.extend_selection();
                keep_scroll = true;
            }
            A::ShrinkSelection => self.shrink_selection(),
            A::CloneCaretAbove | A::CloneCaretBelow => self.clone_caret(action == A::CloneCaretAbove),
            _ => return false,
        }
        match action {
            A::MoveUp | A::SelectUp | A::PageUp => self.skip_folded_lines(false),
            A::MoveDown | A::SelectDown | A::PageDown => self.skip_folded_lines(true),
            _ => {}
        }
        if matches!(action, A::PageUp | A::PageDown) && !self.is_field {
            self.page_scroll(action == A::PageDown, page);
        }
        if !keep_scroll {
            self.ensure_caret_visible();
        } else {
            self.sync_folds();
        }
        if action == A::EditorBackspace {
            self.refilter_completion();
        }
        cx.notify();
        true
    }

    fn pos_for_point(&self, p: Point<Pixels>) -> Option<usize> {
        let l = self.layout.as_ref()?;
        let y = f32::from(p.y - l.bounds.top()) + self.scroll_y;
        let line = self.line_of_row((y / line_height()).floor().max(0.0) as usize);
        let x = p.x - l.text_x;
        let text = self.buffer.line(line);
        let col = if let Some(shaped) = l.shaped(line) {
            byte_to_char(&text, self.text_byte(line, &text, shaped.closest_index_for_x(x.max(px(0.)))))
        } else {
            ((f32::from(x) / f32::from(l.char_w)).round().max(0.0) as usize).min(text.chars().count())
        };
        Some(self.buffer.line_col_to_pos(line, col))
    }

    fn on_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.caret_tip = None;
        self.hover = None;
        self.insight.completion = None;
        self.insight.doc = None;
        self.insight.signature = None;
        if e.button == MouseButton::Left && self.stripe_click(e.position, cx) {
            return;
        }
        if self.scrollbar_down(e.position) {
            cx.notify();
            return;
        }
        if e.button == MouseButton::Left && self.sticky_click(e.position, cx) {
            return;
        }
        if self.fold_click(e.position) {
            cx.notify();
            return;
        }
        if e.button == MouseButton::Left && self.annotation_click(e.position, cx) {
            return;
        }
        if e.button == MouseButton::Left && self.vcs_marker_click(e.position, cx) {
            return;
        }
        if e.button == MouseButton::Left && self.exec_arrow_down(e.position) {
            cx.notify();
            return;
        }
        if e.button == MouseButton::Left {
            if let Some(line) = self.run_mark_at(e.position) {
                cx.emit(EditorEvent::RunMark(line, e.position));
                return;
            }
        }
        if e.button == MouseButton::Left && self.breakpoint_click(e.position, cx) {
            return;
        }
        let Some(pos) = self.pos_for_point(e.position) else { return };
        if e.modifiers.control && !e.modifiers.alt && !e.modifiers.shift && e.click_count == 1 {
            cx.emit(EditorEvent::CtrlClick(pos));
            cx.notify();
            return;
        }
        let plain = !e.modifiers.alt && !e.modifiers.shift && !e.modifiers.control;
        if (e.modifiers.alt && !e.modifiers.shift && !e.modifiers.control || self.column_mode && plain) && e.click_count == 1 {
            // Alt+drag (or any drag in column selection mode): column (block) selection.
            let (line, col) = self.line_col_for_point(e.position);
            self.column_anchor = Some((line, col));
            self.buffer.set_caret(pos, false);
            self.selecting = true;
        } else if e.modifiers.alt && e.modifiers.shift {
            self.buffer.add_caret(pos);
        } else if e.click_count == 2 {
            let r = self.buffer.word_range_at(pos);
            self.buffer.selections = vec![Selection { anchor: r.start, head: r.end, goal_col: None }];
        } else if e.click_count >= 3 {
            let (line, _) = self.buffer.pos_to_line_col(pos);
            let end = if line + 1 < self.buffer.len_lines() { self.buffer.line_start(line + 1) } else { self.buffer.len_chars() };
            self.buffer.selections = vec![Selection { anchor: self.buffer.line_start(line), head: end, goal_col: None }];
        } else {
            self.buffer.set_caret(pos, e.modifiers.shift);
            self.selecting = true;
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let in_gutter = self.show_gutter && self.layout.as_ref().is_some_and(|l| e.position.x < l.text_left);
        if e.pressed_button == Some(MouseButton::Middle) {
            self.middle_move(e.position);
        }
        self.hover_scrollbar(Some(e.position), cx);
        self.sticky_hover(Some(e.position), cx);
        self.stripe_hover(Some(e.position), cx);
        if in_gutter != self.gutter_hover {
            self.gutter_hover = in_gutter;
            cx.notify();
        }
        let run_hover = if in_gutter { self.run_mark_at(e.position) } else { None };
        if run_hover != self.run_hover {
            self.run_hover = run_hover;
            cx.notify();
        }
        if self.gutter_pointer_move(e.position, in_gutter) {
            cx.notify();
        }
        if self.annotation.is_some() {
            self.annotation_hover(e.position, cx);
        }
        // Selection and scrollbar drags are handled window-wide (`global_drag`).
        if self.selecting || self.drag.is_some() {
            return;
        }
        if self.hover_inlay(e.position) {
            cx.notify();
        }
        // Problem tooltip on hover, only over the text itself.
        let hit = self.pos_for_point(e.position).filter(|_| !self.diagnostics.is_empty()).and_then(|pos| {
            let l = self.layout.as_ref()?;
            let (line, col) = self.buffer.pos_to_line_col(pos);
            let i = self.diag_at(pos)?;
            let r = self.diag_range(&self.diagnostics[i]);
            let end_col = self.buffer.pos_to_line_col(r.end).1.max(col + 1);
            let x = f32::from(e.position.x - l.text_x) / f32::from(l.char_w);
            let in_line = self.buffer.pos_to_line_col(r.start).0 == line || self.buffer.pos_to_line_col(r.end).0 == line;
            (in_line && x >= 0.0 && x <= end_col as f32 + 0.5).then_some(i)
        });
        let changed = match (hit, self.hover) {
            (Some(i), Some((j, _))) => i != j,
            (None, None) => false,
            _ => true,
        };
        if changed {
            self.hover = hit.map(|i| (i, e.position));
            cx.notify();
        }
        if hit.is_some() {
            self.mouse_left_text(cx);
        } else {
            // Only over the text itself, not the empty space after a line.
            let over_text = self.pos_for_point(e.position).filter(|&p| {
                let Some(l) = self.layout.as_ref() else { return false };
                let (line, _) = self.buffer.pos_to_line_col(p);
                let x = f32::from(e.position.x - l.text_x) / f32::from(l.char_w);
                x >= 0.0 && x < self.buffer.line(line).chars().count() as f32
            });
            match over_text {
                Some(p) => self.hover_for_doc(Some(p), e.position, cx),
                None => self.mouse_left_text(cx),
            }
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.exec_drag_end(cx);
        self.end_drag();
    }

    /// Right click: move the caret there unless it hits the selection, then ask for the menu.
    fn on_right_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.close_insight();
        if let Some(line) = self.breakpoint_at_point(e.position) {
            cx.emit(EditorEvent::EditBreakpoint(line));
            cx.notify();
            return;
        }
        if self.show_gutter && !self.is_field && self.layout.as_ref().is_some_and(|l| e.position.x < l.text_left) {
            cx.emit(EditorEvent::GutterMenu(e.position));
            cx.notify();
            return;
        }
        if let Some(pos) = self.pos_for_point(e.position) {
            let inside = self.buffer.selections.iter().any(|s| !s.is_empty() && s.range().contains(&pos));
            if !inside {
                self.buffer.set_caret(pos, false);
            }
        }
        cx.emit(EditorEvent::ContextMenu(e.position));
        cx.notify();
    }

    /// Line and (unclamped) column under a point: column selection may reach past line ends.
    fn line_col_for_point(&self, p: Point<Pixels>) -> (usize, usize) {
        let Some(l) = self.layout.as_ref() else { return (0, 0) };
        let y = f32::from(p.y - l.bounds.top()) + self.scroll_y;
        let line = self.line_of_row((y / line_height()).floor().max(0.0) as usize);
        let col = (f32::from(p.x - l.text_x) / f32::from(l.char_w)).round().max(0.0) as usize;
        (line, col)
    }

    /// Alt+drag: one selection per line between the anchor and the pointer.
    pub(super) fn drag_column(&mut self, p: Point<Pixels>) {
        let Some(anchor) = self.column_anchor else { return };
        let head = self.line_col_for_point(p);
        self.set_column_block(anchor, head);
    }

    fn on_scroll(&mut self, e: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let d = e.delta.pixel_delta(px(line_height()));
        if e.modifiers.control && !self.is_field {
            // Ctrl+wheel: font size in all editors (wheel up = bigger), one step per notch.
            self.zoom_acc += f32::from(d.y);
            let step = line_height();
            while self.zoom_acc.abs() >= step {
                let up = self.zoom_acc > 0.0;
                self.zoom_acc -= if up { step } else { -step };
                crate::settings::zoom(Some(if up { 1.0 } else { -1.0 }));
            }
            self.clamp_scroll();
            window.refresh();
            return;
        }
        // A mouse wheel moves in steps of lines: glide there instead of jumping. Touchpads send
        // pixels and are already smooth.
        if matches!(e.delta, gpui::ScrollDelta::Lines(_)) && !e.modifiers.shift && d.x == px(0.) {
            self.smooth_scroll(f32::from(d.y), cx);
            return;
        }
        self.motion.glide = None;
        self.scroll_by(f32::from(d.x), f32::from(d.y), e.modifiers.shift);
        cx.notify();
    }
}

impl Editor {
    /// PyCharm's inspection widget in the editor's top-right corner.
    fn render_widget(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (e, w) = self.problem_counts();
        let stale = self.diag_version != self.buffer.version();
        gpui::div()
            .id("inspection-widget")
            .absolute()
            .top(px(4.))
            .right(px(18.))
            .flex()
            .gap(px(8.))
            .px(px(6.))
            .py(px(1.))
            .rounded(px(4.))
            .text_size(px(12.))
            .bg(Theme::editor_bg())
            .hover(|s| s.bg(Theme::hover_row()))
            .cursor_pointer()
            .when(stale, |d| d.opacity(0.6))
            .when(e > 0, |d| d.child(gpui::div().flex().items_center().gap(px(3.)).text_color(severity_color(Severity::Error)).child(crate::icons::icon("circle-x", 13., severity_color(Severity::Error))).child(e.to_string())))
            .when(w > 0, |d| d.child(gpui::div().flex().items_center().gap(px(3.)).text_color(severity_color(Severity::Warning)).child(crate::icons::icon("triangle-alert", 13., severity_color(Severity::Warning))).child(w.to_string())))
            .when(e + w == 0, |d| d.child(crate::icons::icon("check", 13., Theme::vcs_added())))
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
                cx.emit(EditorEvent::ShowProblems);
            }))
    }

    fn render_tooltip(&self) -> Option<gpui::AnyElement> {
        let l = self.layout.as_ref()?;
        let (i, at) = match (self.hover, self.caret_tip) {
            (Some((i, p)), _) => (i, p + point(px(0.), px(line_height()))),
            (None, Some(i)) => (i, l.caret_xy? + point(px(0.), px(line_height() + 2.))),
            _ => return None,
        };
        let d = self.diagnostics.get(i)?;
        let rel = at - l.bounds.origin;
        let fix = d.fix.as_ref().map(|f| f.message.clone());
        Some(
            gpui::div()
                .absolute()
                .left(rel.x.max(px(4.)))
                .top(rel.y)
                .max_w(px(560.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(6.))
                .border_1()
                .border_color(Theme::divider())
                .bg(Theme::panel())
                .shadow_md()
                .text_size(px(12.5))
                .cursor(gpui::CursorStyle::Arrow)
                .child(gpui::div().flex().items_center().gap(px(6.)).child(crate::icons::icon(match d.severity {
                    Severity::Error => "circle-x",
                    _ => "triangle-alert",
                }, 14., severity_color(d.severity))).child(gpui::div().text_color(Theme::text()).child(d.title())))
                .child(
                    gpui::div()
                        .pt(px(4.))
                        .text_color(Theme::text_muted())
                        .child(match fix {
                            Some(f) => format!("{f}   Alt+Enter"),
                            None => "More actions…   Alt+Enter".into(),
                        }),
                )
                .into_any_element(),
        )
    }
}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Editor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bar = self.render_find_bar(cx);
        let banner = self.render_disk_banner(cx);
        let text_area = gpui::div()
            .id("editor-text")
            .flex_1()
            .min_h(px(0.))
            .relative()
            .cursor(self.pointer_style())
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_middle_down))
            .on_mouse_up(MouseButton::Middle, cx.listener(|this, _, _, cx| this.middle_up(true, cx)))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered && (this.gutter_hover || this.run_hover.is_some()) {
                    this.gutter_hover = false;
                    this.run_hover = None;
                    cx.notify();
                }
                if !*hovered && this.gutter_pointer_leave() {
                    cx.notify();
                }
                if !*hovered {
                    this.hover_scrollbar(None, cx);
                    this.sticky_hover(None, cx);
                    this.stripe_hover(None, cx);
                    if this.inlays.tip.take().is_some() {
                        cx.notify();
                    }
                }
            }))
            .child(EditorElement { editor: cx.entity(), diff_bg: std::mem::take(&mut self.diff_bg), diff_words: std::mem::take(&mut self.diff_words), diff_blocks: std::mem::take(&mut self.diff_blocks) })
            .when(self.is_python() && self.diag_version != u64::MAX, |d| d.child(self.render_widget(cx)))
            .when_some(self.render_tooltip(), |d, t| d.child(t))
            .when_some(self.render_stripe_tip(), |d, t| d.child(t))
            .when_some(self.render_breakpoint_tip(), |d, t| d.child(t))
            .when_some(self.render_inlay_tip(), |d, t| d.child(t))
            .when_some(self.render_middle_anchor(), |d, a| d.child(a))
            .when_some(self.render_annotation_tip(), |d, t| d.child(t))
            .when_some(self.render_vcs_popup(cx), |d, t| d.child(t))
            .children(self.render_insight(cx));
        gpui::div()
            .id("editor")
            .key_context("Editor")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            // A one-line field takes its look (background, border) from the input around it.
            .when(!self.is_field, |d| d.bg(Theme::editor_bg()))
            // Completion, Quick Doc and Parameter Info close on a click anywhere outside the editor.
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.close_insight() | this.caret_tip.take().is_some() {
                    cx.notify();
                }
            }))
            .children(banner)
            .children(bar)
            .child(text_area)
    }
}

// --- IME / text input ------------------------------------------------------------

impl EntityInputHandler for Editor {
    fn text_for_range(&mut self, range: Range<usize>, actual: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let rope = self.buffer.rope();
        let total = rope.len_utf16_cu();
        let r = range.start.min(total)..range.end.min(total);
        let a = rope.utf16_cu_to_char(r.start);
        let b = rope.utf16_cu_to_char(r.end);
        actual.replace(r);
        Some(rope.slice(a..b).to_string())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let s = self.buffer.primary();
        let rope = self.buffer.rope();
        let r = s.range();
        Some(UTF16Selection { range: rope.char_to_utf16_cu(r.start)..rope.char_to_utf16_cu(r.end), reversed: s.head < s.anchor })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let rope = self.buffer.rope();
        let len = rope.len_chars();
        self.marked.as_ref().map(|m| rope.char_to_utf16_cu(m.start.min(len))..rope.char_to_utf16_cu(m.end.min(len)))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(&mut self, _range: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_error.is_some() {
            return;
        }
        if let Some(m) = self.marked.take() {
            // The buffer may have shrunk under an open composition (reload, undo): clamp.
            let len = self.buffer.len_chars();
            self.buffer.selections = vec![Selection { anchor: m.start.min(len), head: m.end.min(len), goal_col: None }];
        }
        // Ctrl+<letter> combos without a binding produce control chars on Windows — ignore them.
        if text.is_empty() || text.chars().any(|c| c.is_control() && c != '\t') {
            return;
        }
        let mut chars = text.chars();
        self.insight.doc = None;
        match (chars.next(), chars.next()) {
            (Some(c), None) => {
                self.buffer.type_char(c);
                self.refilter_completion();
                if c == ')' {
                    self.insight.signature = None;
                }
                cx.emit(EditorEvent::Typed(c));
            }
            _ => {
                self.buffer.insert(text);
                self.insight.completion = None;
            }
        }
        self.ensure_caret_visible();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(&mut self, _range: Option<Range<usize>>, text: &str, _sel: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(m) = self.marked.take() {
            // The buffer may have shrunk under an open composition (reload, undo): clamp.
            let len = self.buffer.len_chars();
            self.buffer.selections = vec![Selection { anchor: m.start.min(len), head: m.end.min(len), goal_col: None }];
        }
        let start = self.buffer.primary().range().start;
        self.buffer.insert(text);
        let n = text.chars().count();
        self.marked = (n > 0).then(|| start..start + n);
        cx.notify();
    }

    fn bounds_for_range(&mut self, _range: Range<usize>, _bounds: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let l = self.layout.as_ref()?;
        let (line, col) = self.caret_line_col();
        let y = l.bounds.top() + px(self.row_of(line) as f32 * line_height() - self.scroll_y);
        let x = if let Some(shaped) = l.shaped(line) {
            let text = self.buffer.line(line);
            l.text_x + shaped.x_for_index(self.display_byte(line, &text, char_to_byte(&text, col)))
        } else {
            l.text_x
        };
        Some(Bounds::new(point(x, y), size(px(2.), px(line_height()))))
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

// --- Rendering -------------------------------------------------------------------

struct EditorElement {
    editor: Entity<Editor>,
    diff_bg: bool,
    diff_words: Vec<(usize, Range<usize>)>,
    diff_blocks: Vec<(Range<usize>, gpui::Hsla)>,
}

struct Prepaint {
    lines: Vec<ShapedLine>,
    numbers: Vec<ShapedLine>,
    /// Buffer line of each display row on screen.
    rows: Vec<usize>,
    /// Gutter fold markers: (row index, collapsed or expanded glyph).
    fold_marks: Vec<(usize, bool)>,
    /// "..." drawn after a collapsed header.
    placeholder: ShapedLine,
    /// Bookmarks on screen: (row index, mnemonic shaped).
    bookmark_marks: Vec<(usize, Option<ShapedLine>)>,
    /// Breakpoints on screen: (row index, has a condition, verified by the debugger, enabled).
    breakpoint_marks: Vec<(usize, bool, Option<bool>, bool)>,
    /// A faint breakpoint under the pointer, on a line without one: its row index.
    breakpoint_preview: Option<usize>,
    /// Run icons on screen: (row index, last test status: passed?, under the mouse), and
    /// the width of their column.
    run_icons: Vec<(usize, Option<bool>, bool)>,
    run_slot: Pixels,
    /// Blame annotations on screen: (row index, label, age).
    annotations: Vec<(usize, ShapedLine, f32)>,
    /// Debugger values after line ends: (row index, "name: value, …").
    inline_vals: Vec<(usize, ShapedLine)>,
    /// Inlay hint chips: (row index, byte range in the drawn line).
    chips: Vec<(usize, std::ops::Range<usize>)>,
    gutter_w: Pixels,
    char_w: Pixels,
    marks: marks::CaretMarks,
    /// Sticky lines: (text, line number, top offset, under the mouse).
    sticky: Vec<(ShapedLine, ShapedLine, f32, bool)>,
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = gpui::relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> Prepaint {
        // One-line fields (filters, names, find) use the UI font like the rest of the chrome.
        let is_field = self.editor.read(cx).is_field;
        let font_size = px(if is_field { crate::ui_kit::FIELD_TEXT } else { crate::settings::font_size() });
        let mono = font(if is_field { crate::theme::ui_font().to_string() } else { crate::settings::editor_font() });
        let run = |len: usize, color: gpui::Hsla| TextRun { len, font: mono.clone(), color, background_color: None, underline: None, strikethrough: None };

        let headers = self.editor.update(cx, |ed, cx| {
            if ed.step_glide() {
                window.request_animation_frame();
            }
            ed.reparse();
            ed.background_parse(cx);
            ed.schedule_folds(cx);
            ed.update_occurrences(cx);
            ed.refresh_vcs();
            ed.refresh_line_extent();
            ed.sync_folds();
            ed.sync_bookmarks();
            ed.refresh_run_marks(cx);
            ed.update_sticky();
            ed.update_guides();
            ed.update_stripe();
            let h = if ed.show_gutter {
                ed.fold_headers()
            } else {
                Vec::new()
            };
            h
        });
        let ed = self.editor.read(cx);
        let b = &ed.buffer;
        let n_lines = b.len_lines();
        let total_rows = ed.display_rows();
        let first_row = ((ed.scroll_y / line_height()).floor() as usize).min(total_rows.saturating_sub(1));
        let visible = (f32::from(bounds.size.height) / line_height()).ceil() as usize + 1;
        let mut rows = Vec::with_capacity(visible);
        let mut next = ed.line_of_row(first_row);
        while rows.len() < visible && next < n_lines {
            rows.push(next);
            next = ed.next_shown_line(next);
        }

        let digit = window.text_system().shape_line("0".into(), font_size, &[run(1, Theme::text())], None);
        let char_w = digit.width;
        let digits = n_lines.to_string().len().max(3);
        let run_slot = ed.run_slot();
        let gutter_w = if ed.show_gutter { char_w * digits as f32 + px(46.) + ed.annotation_width(char_w) + run_slot } else { px(0.) };

        // Highlights: one query per run of consecutive shown lines (folds split them).
        let rope = b.rope();
        let line_bytes = |l: usize| rope.char_to_byte(b.line_start(l));
        let end_bytes = |l: usize| if l + 1 < n_lines { line_bytes(l + 1) } else { rope.len_bytes() };
        let mut spans = Vec::new();
        let mut i = 0;
        while i < rows.len() {
            let mut j = i;
            while j + 1 < rows.len() && rows[j + 1] == rows[j] + 1 {
                j += 1;
            }
            let range = line_bytes(rows[i])..end_bytes(rows[j]);
            spans.extend(match &ed.syntax {
                Some(s) => s.highlights(&ed.text_cache.1, range),
                None if ed.is_sql() => brisk_syntax::highlight_sql(&ed.text_cache.1, range),
                None => Vec::new(),
            });
            i = j + 1;
        }
        let (byte_lo, byte_hi) = match (rows.first(), rows.last()) {
            (Some(&a), Some(&z)) => (line_bytes(a), end_bytes(z)),
            _ => (0, 0),
        };
        let marks = ed.caret_marks(byte_lo..byte_hi);

        let (caret_line, _) = ed.caret_line_col();
        let mut lines = Vec::with_capacity(rows.len());
        let mut numbers = Vec::with_capacity(rows.len());
        let mut fold_marks = Vec::new();
        let mut inline_vals = Vec::new();
        let mut chips = Vec::new();
        let mut si = 0;
        for (ri, &l) in rows.iter().enumerate() {
            let folded = ed.is_folded_header(l);
            if folded || (ed.gutter_hover && headers.binary_search(&l).is_ok()) {
                fold_marks.push((ri, folded));
            }
            let text = b.line(l);
            let lb = rope.char_to_byte(b.line_start(l));
            let le = lb + text.len();
            let mut runs = Vec::new();
            let mut cur = lb;
            while si < spans.len() && spans[si].range.end <= lb {
                si += 1;
            }
            let mut j = si;
            while j < spans.len() && spans[j].range.start < le {
                let sp = &spans[j];
                let s = sp.range.start.max(lb);
                let e = sp.range.end.min(le);
                if s > cur {
                    runs.push(run(s - cur, Theme::text()));
                }
                if e > s {
                    runs.push(run(e - s, Theme::syntax(sp.kind)));
                }
                cur = e.max(cur);
                j += 1;
            }
            if le > cur {
                runs.push(run(le - cur, Theme::text()));
            }
            for d in ed.diagnostics.iter().filter(|d| d.severity == Severity::Unused && d.start.line <= l && l <= d.end.line) {
                let c0 = if d.start.line == l { d.start.col } else { 0 };
                let c1 = if d.end.line == l { d.end.col } else { usize::MAX };
                let (b0, b1) = (char_to_byte(&text, c0), char_to_byte(&text, c1));
                if b1 > b0 {
                    runs = recolor(runs, b0..b1, severity_color(Severity::Unused));
                }
            }
            for d in ed.diagnostics.iter().filter(|d| squiggle::is_deprecated(d) && d.start.line <= l && l <= d.end.line) {
                let c0 = if d.start.line == l { d.start.col } else { 0 };
                let c1 = if d.end.line == l { d.end.col } else { usize::MAX };
                let (b0, b1) = (char_to_byte(&text, c0), char_to_byte(&text, c1));
                if b1 > b0 {
                    runs = squiggle::strike(runs, b0..b1);
                }
            }
            // Hints in the UI font, so they do not read as code; their chips are painted under the text.
            let (text, runs, hint_chips) = ed.line_with_inlays(l, text, runs, |len| TextRun { font: font(crate::theme::ui_font()), ..run(len, Theme::inlay_fg()) });
            chips.extend(hint_chips.into_iter().map(|r| (ri, r)));
            lines.push(window.text_system().shape_line(SharedString::from(text), font_size, &runs, None));
            if let Some(v) = ed.inline_value(l) {
                let r = TextRun { font: gpui::Font { style: gpui::FontStyle::Italic, ..run(0, Theme::text_dim()).font }, ..run(v.len(), Theme::text_dim()) };
                inline_vals.push((ri, window.text_system().shape_line(SharedString::from(v.to_string()), font_size, &[r], None)));
            }

            if !ed.show_gutter {
                continue;
            }
            let num = (l + 1).to_string();
            let color = if l == caret_line { Theme::gutter_fg_active() } else { Theme::gutter_fg() };
            numbers.push(window.text_system().shape_line(SharedString::from(num.clone()), font_size, &[run(num.len(), color)], None));
        }
        let placeholder = window.text_system().shape_line("...".into(), font_size, &[run(3, Theme::text_muted())], None);
        let mut bookmark_marks = Vec::new();
        if ed.show_gutter {
            for (line, m) in ed.bookmark_lines_now() {
                if let Ok(i) = rows.binary_search(&line) {
                    let shaped = m.map(|c| {
                        let s = c.to_string();
                        window.text_system().shape_line(s.clone().into(), px(10.5), &[run(s.len(), Theme::bg())], None)
                    });
                    bookmark_marks.push((i, shaped));
                }
            }
        }
        let mut breakpoint_marks = Vec::new();
        if ed.show_gutter {
            for (line, bp) in ed.breakpoint_list() {
                if let Ok(i) = rows.binary_search(&line) {
                    // Muted: grey like a disabled one, but filled.
                    let verified = if ed.breakpoints_muted() { Some(true) } else { bp.verified };
                    breakpoint_marks.push((i, bp.condition.is_some(), verified, bp.enabled && !ed.breakpoints_muted()));
                }
            }
        }
        let breakpoint_preview = ed.breakpoint_preview_line().and_then(|l| rows.binary_search(&l).ok());
        let mut run_icons = Vec::new();
        for m in ed.run_marks_now() {
            if let Ok(i) = rows.binary_search(&m.line) {
                let status = match ed.test_status(&m.target) {
                    Some(brisk_run::TestStatus::Passed) => Some(true),
                    Some(brisk_run::TestStatus::Failed | brisk_run::TestStatus::Error) => Some(false),
                    _ => None,
                };
                run_icons.push((i, status, ed.run_hover == Some(m.line)));
            }
        }
        let mut annotations = Vec::new();
        if let Some(a) = ed.annotation.as_ref().filter(|_| ed.show_gutter) {
            for (i, &l) in rows.iter().enumerate() {
                if let Some(Some(x)) = a.lines.get(l) {
                    let s = x.label();
                    let color = if ed.ann_hover.is_some_and(|h| h.0 == l) { Theme::accent() } else { Theme::text_muted() };
                    annotations.push((i, window.text_system().shape_line(SharedString::from(s.clone()), font_size * 0.9, &[run(s.len(), color)], None), x.age));
                }
            }
        }
        let sticky = ed
            .sticky
            .shown
            .iter()
            .enumerate()
            .map(|(k, &(l, y))| {
                let (text, runs) = ed.sticky_runs(l, &run);
                let num = (l + 1).to_string();
                let n = window.text_system().shape_line(SharedString::from(num.clone()), font_size, &[run(num.len(), Theme::gutter_fg())], None);
                (window.text_system().shape_line(SharedString::from(text), font_size, &runs, None), n, y, ed.sticky.hover == Some(k))
            })
            .collect();
        Prepaint { lines, numbers, rows, fold_marks, placeholder, bookmark_marks, breakpoint_marks, breakpoint_preview, run_icons, run_slot, annotations, inline_vals, chips, gutter_w, char_w, marks, sticky }
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), pp: &mut Prepaint, window: &mut Window, cx: &mut App) {
        let focus = self.editor.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.editor.clone()), cx);
        let focused = focus.is_focused(window);

        let ed = self.editor.read(cx);
        let b = &ed.buffer;
        let lh = px(line_height());
        let text_left = bounds.left() + pp.gutter_w + px(8.);
        let text_x = text_left - px(ed.scroll_x);
        let exec_line = ed.exec_line;
        let is_field = ed.is_field;
        let exec_ghost = ed.exec_drag_target();
        let ann_x0 = bounds.left() + ed.annotation_left();
        let ann_w = ed.annotation_width(pp.char_w) - px(6.);
        let y_of = |l: usize| bounds.top() + px(ed.row_of(l) as f32 * line_height() - ed.scroll_y);
        let rows = &pp.rows;
        let first = rows.first().copied().unwrap_or(0);
        let last = rows.last().map_or(0, |l| l + 1);
        let idx_of = |line: usize| rows.binary_search(&line).ok();
        let x_of = |line: usize, col: usize| -> Pixels {
            match idx_of(line) {
                Some(i) => {
                    let t = b.line(line);
                    text_x + pp.lines[i].x_for_index(ed.display_byte(line, &t, char_to_byte(&t, col)))
                }
                None => text_x,
            }
        };

        // Compute geometry first (borrows the editor), then paint (needs &mut App).
        let mut under: Vec<gpui::PaintQuad> = Vec::new();
        let mut carets: Vec<gpui::PaintQuad> = Vec::new();
        // Current line (not in a one-line field).
        if !ed.is_field && b.selections.iter().all(|s| s.is_empty()) {
            for s in &b.selections {
                let (l, _) = b.pos_to_line_col(s.head);
                if idx_of(l).is_some() {
                    under.push(fill(Bounds::new(point(bounds.left(), y_of(l)), size(bounds.size.width, lh)), Theme::current_line()));
                }
            }
        }
        // The debugger's execution point.
        if let Some(l) = ed.exec_line.filter(|l| idx_of(*l).is_some()) {
            under.push(fill(Bounds::new(point(bounds.left(), y_of(l)), size(bounds.size.width, lh)), Theme::exec_line()));
        }
        // In a diff viewer: the changed lines' backgrounds, as in PyCharm's diff.
        if self.diff_bg && !self.diff_blocks.is_empty() {
            let x0 = bounds.left() + pp.gutter_w - px(4.);
            let x1 = bounds.right() - px(14.);
            for (r, color) in &self.diff_blocks {
                if r.start > last || r.end < first {
                    continue;
                }
                if r.is_empty() {
                    under.push(fill(Bounds::from_corners(point(x0, y_of(r.start) - px(1.)), point(x1, y_of(r.start) + px(1.))), color.opacity(1.)));
                } else {
                    under.push(fill(Bounds::from_corners(point(x0, y_of(r.start)), point(x1, y_of(r.end))), *color).corner_radii(px(4.)));
                }
            }
        } else if self.diff_bg {
            for ch in &ed.vcs_changes {
                use brisk_git::LineChangeKind as K;
                match ch.kind {
                    K::Added | K::Modified => {
                        // One rounded block per change, as in PyCharm's diff.
                        let color = if ch.kind == K::Added { Theme::diff_added_bg() } else { Theme::diff_modified_bg() };
                        if ch.lines.start < last && ch.lines.end > first {
                            let (y0, y1) = (y_of(ch.lines.start), y_of(ch.lines.end));
                            let x0 = bounds.left() + pp.gutter_w - px(4.);
                            under.push(fill(Bounds::from_corners(point(x0, y0), point(bounds.right() - px(14.), y1)), color).corner_radii(px(4.)));
                        }
                    }
                    K::Deleted => {
                        let l = ch.lines.start;
                        if l >= first && l <= last {
                            under.push(fill(Bounds::new(point(bounds.left(), y_of(l) - px(1.)), size(bounds.size.width, px(2.))), Theme::vcs_deleted()));
                        }
                    }
                }
            }
        }
        // Changed words inside changed lines (live diff), over the line backgrounds.
        if self.diff_bg {
            for (l, r) in &self.diff_words {
                if idx_of(*l).is_none() {
                    continue;
                }
                let t = b.line(*l);
                if t.contains('\t') || r.end > t.len() || !t.is_char_boundary(r.start) || !t.is_char_boundary(r.end) {
                    continue;
                }
                let (a, z) = (x_of(*l, byte_to_char(&t, r.start)), x_of(*l, byte_to_char(&t, r.end)));
                under.push(fill(Bounds::from_corners(point(a, y_of(*l) + px(1.)), point(z, y_of(*l) + lh - px(1.))), Theme::diff_modified_word_bg()).corner_radii(px(3.)));
            }
        }
        // Indent guides (editor/guides.rs): one at each enclosing block's level, none of their
        // own on continuation lines, kept through blank lines; the caret's block brighter.
        let unit = b.indent_unit.chars().count().max(1);
        let scale = window.scale_factor();
        for &l in rows.iter() {
            let ws = ed.guide_indent(l);
            for c in (0..ws).step_by(unit) {
                // Snapped to a device pixel so the 1-px line stays crisp.
                let x = px((f32::from(text_x + pp.char_w * c as f32) * scale).round() / scale);
                let active = ed.guides.active.is_some_and(|(col, a, z)| col == c && (a..=z).contains(&l));
                let color = if active { Theme::indent_guide_active() } else { Theme::indent_guide() };
                under.push(fill(Bounds::new(point(x, y_of(l)), size(px(1. / scale), lh)), color));
            }
        }
        // The right margin (PyCharm's "Hard wrap at" column), a thin line across the text area.
        if let Some(m) = ed.right_margin.filter(|_| !ed.is_field) {
            let x = text_x + pp.char_w * m as f32;
            if x > text_left && x < bounds.right() {
                under.push(fill(Bounds::new(point(x, bounds.top()), size(px(1.), bounds.size.height)), Theme::right_margin()));
            }
        }
        // Search matches: all of them, the current one brighter.
        if ed.find.is_some() {
            let lo = b.line_start(first);
            let hi = if last < b.len_lines() { b.line_start(last) } else { b.len_chars() };
            for (r, cur) in ed.visible_matches(lo, hi) {
                let (l1, c1) = b.pos_to_line_col(r.start);
                let (l2, c2) = b.pos_to_line_col(r.end);
                let mut shape = Vec::new();
                for l in (l1.max(first)..=l2.min(last.saturating_sub(1))).filter(|&l| idx_of(l).is_some()) {
                    let sx = if l == l1 { x_of(l, c1) } else { text_x };
                    let ex = if l == l2 { x_of(l, c2) } else { x_of(l, b.line_len(l)) + pp.char_w };
                    if ex > sx {
                        // 1 px in at the hit's top and bottom, so hits on adjacent lines stay apart.
                        let (t, z) = (if l == l1 { px(1.) } else { px(0.) }, if l == l2 { px(1.) } else { px(0.) });
                        shape.push(rounded::row(y_of(l) + t, y_of(l) + lh - z, sx, ex));
                    }
                }
                rounded::push(&shape, rounded::MARK, scale, if cur { Theme::search_current() } else { Theme::search_hit() }, &mut under);
            }
        }
        // Identifier occurrences and the matching brackets under the caret.
        let mark_fill = |r: &Range<usize>, color: gpui::Hsla, under: &mut Vec<gpui::PaintQuad>| {
            let (l1, c1) = b.pos_to_line_col(r.start);
            let (l2, c2) = b.pos_to_line_col(r.end);
            if l1 == l2 && idx_of(l1).is_some() {
                rounded::push(&[rounded::row(y_of(l1) + px(1.), y_of(l1) + lh - px(1.), x_of(l1, c1), x_of(l1, c2))], rounded::MARK, scale, color, under);
            }
        };
        for r in &pp.marks.occurrences {
            mark_fill(r, Theme::occurrence(), &mut under);
        }
        for r in &pp.marks.brackets {
            mark_fill(r, Theme::bracket_match(), &mut under);
        }
        // Selections.
        for s in &b.selections {
            if s.is_empty() {
                continue;
            }
            let r = s.range();
            let (l1, c1) = b.pos_to_line_col(r.start);
            let (l2, c2) = b.pos_to_line_col(r.end);
            // One shape per selection: rounded outer corners, the rows joined.
            let mut shape = Vec::new();
            for l in (l1.max(first)..=l2.min(last.saturating_sub(1))).filter(|&l| idx_of(l).is_some()) {
                let sx = if l == l1 { x_of(l, c1) } else { text_x };
                let ex = if l == l2 { x_of(l, c2) } else { x_of(l, b.line_len(l)) + pp.char_w };
                if ex > sx {
                    shape.push(rounded::row(y_of(l), y_of(l) + lh, sx, ex));
                }
            }
            rounded::push(&shape, rounded::SELECTION, scale, Theme::selection(), &mut under);
        }
        // Carets: fading blink, optional slide of the primary one, 2 px snapped to device pixels.
        let caret_look = focused.then(|| {
            let (l, c) = b.pos_to_line_col(b.primary().head);
            let target = idx_of(l).map(|_| point(x_of(l, c), y_of(l)));
            let key = (b.version(), b.primary().head, b.selections.len());
            ed.motion.caret_look(key, target, point(px(ed.scroll_x), px(ed.scroll_y)))
        });
        if let Some(look) = caret_look.as_ref().filter(|k| k.alpha > 0.0) {
            let scale = window.scale_factor();
            let snap = |v: Pixels| px((f32::from(v) * scale).round() / scale);
            let w = px((2.0 * scale).round().max(1.0) / scale);
            let primary = b.primary().head;
            for s in &b.selections {
                let (l, c) = b.pos_to_line_col(s.head);
                if idx_of(l).is_some() {
                    let at = look.primary.filter(|_| s.head == primary).unwrap_or_else(|| point(x_of(l, c), y_of(l)));
                    carets.push(fill(Bounds::new(point(snap(at.x), snap(at.y)), size(w, lh)), Theme::caret().opacity(look.alpha)));
                }
            }
        }
        // VCS change markers (PyCharm: colored bar right of the line numbers).
        let mut markers: Vec<gpui::PaintQuad> = Vec::new();
        let mx = bounds.left() + pp.gutter_w - px(23.);
        for ch in &ed.vcs_changes {
            use brisk_git::LineChangeKind as K;
            match ch.kind {
                K::Added | K::Modified => {
                    let s = ch.lines.start.max(first);
                    let e = ch.lines.end.min(last);
                    if s < e {
                        let color = if ch.kind == K::Added { Theme::vcs_added() } else { Theme::vcs_modified() };
                        markers.push(fill(Bounds::from_corners(point(mx, y_of(s)), point(mx + px(3.), y_of(e))), color));
                    }
                }
                K::Deleted => {
                    let l = ch.lines.start;
                    if l >= first && l <= last {
                        markers.push(fill(Bounds::new(point(mx - px(2.), y_of(l) - px(1.5)), size(px(7.), px(3.))), Theme::vcs_deleted()));
                    }
                }
            }
        }
        // Coverage: a bar between the change markers and the fold markers.
        if ed.coverage.is_some() {
            for &l in rows.iter() {
                if let Some(hit) = ed.coverage_at(l) {
                    let color = if hit { Theme::coverage_hit() } else { Theme::coverage_miss() };
                    markers.push(fill(Bounds::new(point(mx + px(4.), y_of(l)), size(px(3.), lh)), color));
                }
            }
        }
        let line_ys: Vec<Pixels> = rows.iter().map(|&l| y_of(l)).collect();
        // Collapsed headers: a "..." box after the line.
        let mut folded_boxes: Vec<(gpui::PaintQuad, Point<Pixels>)> = Vec::new();
        for (i, &l) in rows.iter().enumerate() {
            if ed.is_folded_header(l) {
                let x = x_of(l, b.line_len(l)) + pp.char_w;
                let w = pp.placeholder.width + px(8.);
                let q = fill(Bounds::new(point(x, line_ys[i] + px(2.)), size(w, lh - px(4.))), Theme::hover_row()).corner_radii(px(3.));
                folded_boxes.push((q, point(x + px(4.), line_ys[i])));
            }
        }
        // Problems: wavy underlines (errors and warnings) and marks on the right edge.
        let mut waves: Vec<(Pixels, Pixels, Pixels, gpui::Hsla)> = Vec::new();
        let mut stripe: Vec<gpui::PaintQuad> = Vec::new();
        let total = (ed.display_rows() as f32 * line_height()).max(f32::from(bounds.size.height));
        for d in &ed.diagnostics {
            let color = severity_color(d.severity);
            if d.severity != Severity::Unused && !squiggle::is_deprecated(d) {
                let r = ed.diag_range(d);
                let (l1, c1) = b.pos_to_line_col(r.start);
                let (l2, c2) = b.pos_to_line_col(r.end);
                for l in (l1.max(first)..=l2.min(last.saturating_sub(1))).filter(|&l| idx_of(l).is_some()) {
                    let Some((a, z)) = squiggle::wave_cols(&b.line(l), (l == l1).then_some(c1), (l == l2).then_some(c2)) else { continue };
                    let len = b.line_len(l);
                    let sx = x_of(l, a.min(len)) + pp.char_w * a.saturating_sub(len) as f32;
                    let ex = x_of(l, z.min(len)) + pp.char_w * z.saturating_sub(len) as f32;
                    waves.push((sx, ex, squiggle::wave_top(y_of(l), lh), color));
                }
            }
        }
        let mut thumbs: Vec<gpui::PaintQuad> = Vec::new();
        if ed.show_gutter {
            let vh = f32::from(bounds.size.height);
            let line_y = |l: usize| bounds.top() + px(ed.row_of(l) as f32 * line_height() / total * vh);
            // The error stripe (editor/stripe.rs): changes as a thin strip on the left of the track,
            // the rest as small marks, errors on top; marks closer than 2 px to the previous one of
            // the same kind are skipped. Then the caret line's tick.
            let mut last: Option<(stripe::Kind, f32)> = None;
            for m in &ed.stripe.marks {
                let (t, h) = ed.stripe_y(m, vh, total);
                let vcs = matches!(m.kind, stripe::Kind::Added | stripe::Kind::Modified | stripe::Kind::Deleted);
                if !vcs && last.is_some_and(|(k, y)| k == m.kind && t - y < 2.0) {
                    continue;
                }
                last = Some((m.kind, t));
                let (x, w) = if vcs { (11., 2.) } else { (8., 6.) };
                stripe.push(fill(Bounds::new(point(bounds.right() - px(x), bounds.top() + px(t)), size(px(w), px(h))), m.kind.color()));
            }
            if !ed.stripe.marks.is_empty() || !ed.diagnostics.is_empty() {
                let t = ed.stripe_caret_y(vh, total);
                stripe.push(fill(Bounds::new(point(bounds.right() - px(9.), bounds.top() + px(t)), size(px(8.), px(1.))), Theme::caret().opacity(0.7)));
            }
            if pp.marks.occurrence_lines.len() > 1 {
                let mut last_y = -10.0f32;
                for &l in &pp.marks.occurrence_lines {
                    let y = line_y(l);
                    if f32::from(y) - last_y >= 2.0 {
                        stripe.push(fill(Bounds::new(point(bounds.right() - px(8.), y), size(px(6.), px(2.))), Theme::occurrence_mark()));
                        last_y = f32::from(y);
                    }
                }
            }
            // Hovered or dragged: a faint track, and a lighter thumb 2 px thicker.
            let look = |bar| if ed.bar_hot(bar) { (Theme::scroll_thumb_hot(), scroll::BAR - 2.) } else { (Theme::scroll_thumb(), scroll::BAR - 4.) };
            if let Some((top, h)) = ed.v_thumb(vh) {
                let (color, t) = look(scroll::Bar::Vertical);
                if ed.bar_hot(scroll::Bar::Vertical) {
                    thumbs.push(fill(Bounds::new(point(bounds.right() - px(scroll::BAR), bounds.top()), size(px(scroll::BAR), px(vh))), Theme::scroll_track_hot()));
                }
                thumbs.push(fill(Bounds::new(point(bounds.right() - px(t + 1.), bounds.top() + px(top)), size(px(t), px(h))), color).corner_radii(px(t / 2.)));
            }
            let vw = f32::from(bounds.right() - text_left) - scroll::BAR;
            if let Some((left, w)) = ed.h_thumb(vw) {
                let (color, t) = look(scroll::Bar::Horizontal);
                if ed.bar_hot(scroll::Bar::Horizontal) {
                    thumbs.push(fill(Bounds::new(point(text_left, bounds.bottom() - px(scroll::BAR)), size(px(vw), px(scroll::BAR))), Theme::scroll_track_hot()));
                }
                thumbs.push(fill(Bounds::new(point(text_left + px(left), bounds.bottom() - px(t + 1.)), size(px(w), px(t))), color).corner_radii(px(t / 2.)));
            }
        }
        let dragging = ed.selecting || ed.drag.is_some() || ed.exec_dragging();
        let ed_sticky_h = ed.sticky_height();
        let caret_xy = {
            let (l, c) = b.pos_to_line_col(b.primary().head);
            idx_of(l).is_some().then(|| point(x_of(l, c), y_of(l)))
        };

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for q in under {
                window.paint_quad(q);
            }
            // Inlay hint chips: rounded, snapped to device pixels, a little inside the hint's
            // padding so they keep a gap from the code.
            let scale = window.scale_factor();
            let snap = |v: Pixels| px((f32::from(v) * scale).round() / scale);
            for (i, r) in &pp.chips {
                if let (Some(line), Some(y)) = (pp.lines.get(*i), line_ys.get(*i)) {
                    let (x0, x1) = (snap(text_x + line.x_for_index(r.start) + px(1.5)), snap(text_x + line.x_for_index(r.end) - px(1.5)));
                    let (top, h) = (snap(*y + px(2.)), snap(lh - px(4.)));
                    window.paint_quad(fill(Bounds::new(point(x0, top), size(x1 - x0, h)), Theme::inlay_bg()).corner_radii(px(4.)));
                }
            }
            for (line, y) in pp.lines.iter().zip(&line_ys) {
                let _ = line.paint(point(text_x, *y), lh, window, cx);
            }
            for (i, v) in &pp.inline_vals {
                if let (Some(line), Some(y)) = (pp.lines.get(*i), line_ys.get(*i)) {
                    let _ = v.paint(point(text_x + line.width + pp.char_w * 3., *y), lh, window, cx);
                }
            }
            for (q, at) in folded_boxes {
                window.paint_quad(q);
                let _ = pp.placeholder.paint(at, lh, window, cx);
            }
            for (x0, x1, y, color) in waves {
                squiggle::paint_wave(window, x0, x1, y, color);
            }
            // Gutter (painted over text so horizontal overflow never covers line numbers).
            if !is_field {
                window.paint_quad(fill(Bounds::new(bounds.origin, size(text_left - bounds.left() - px(2.), bounds.size.height)), Theme::editor_bg()));
            }
            if !pp.annotations.is_empty() {
                // Newer lines get a stronger tint, like PyCharm's annotation colors.
                for (i, label, age) in &pp.annotations {
                    let y = line_ys[*i];
                    window.paint_quad(fill(Bounds::new(point(ann_x0, y), size(ann_w, lh)), Theme::accent().opacity(0.06 + 0.22 * (1.0 - age))));
                    let _ = label.paint(point(ann_x0 + px(4.), y), lh, window, cx);
                }
            }
            for (row, (n, y)) in pp.numbers.iter().zip(&line_ys).enumerate() {
                // A breakpoint takes the number's place, as in PyCharm.
                if pp.breakpoint_marks.iter().any(|b| b.0 == row) || pp.breakpoint_preview == Some(row) {
                    continue;
                }
                let x = bounds.left() + pp.gutter_w - px(30.) - pp.run_slot - n.width;
                let _ = n.paint(point(x, *y), lh, window, cx);
            }
            for &(i, conditional, verified, enabled) in &pp.breakpoint_marks {
                // A circle over the line number: an analytic quad on whole device pixels, even
                // at 125% / 150% (see `icons::paint_dot`): a ring when disabled or the debugger
                // cannot stop there, a hole in it when it has a condition.
                let d = 11.;
                let c = point(bounds.left() + pp.gutter_w - px(30.) - pp.run_slot - px(1. + d / 2.), line_ys[i] + lh / 2.);
                let color = if enabled { Theme::breakpoint() } else { Theme::text_dim() };
                let solid = (enabled && verified != Some(false)) || (!enabled && verified == Some(true));
                let ring = match (solid, conditional) {
                    (true, true) => Some(d / 3.),
                    (true, false) => None,
                    (false, _) => Some(d * 0.15),
                };
                crate::icons::paint_dot(window, c, d, color, ring);
            }
            if let Some(i) = pp.breakpoint_preview {
                // Where a click would set a breakpoint, as in PyCharm: a faint one.
                let d = 11.;
                let c = point(bounds.left() + pp.gutter_w - px(30.) - pp.run_slot - px(1. + d / 2.), line_ys[i] + lh / 2.);
                crate::icons::paint_dot(window, c, d, Theme::breakpoint().opacity(0.4), None);
            }
            if let Some(i) = exec_line.and_then(|l| idx_of(l)) {
                // ▶ arrow at the bookmark slot.
                let y = line_ys[i];
                let x = bounds.left() + px(5.);
                let h = lh - px(8.);
                window.paint_quad(fill(Bounds::new(point(x, y + px(4.)), size(px(9.), h)), Theme::accent()).corner_radii(px(2.)));
            }
            if let Some(i) = exec_ghost.and_then(|l| idx_of(l)) {
                // Where a dragged arrow would go.
                let y = line_ys[i];
                window.paint_quad(fill(Bounds::new(point(bounds.left() + px(5.), y + px(4.)), size(px(9.), lh - px(8.))), Theme::accent().opacity(0.45)).corner_radii(px(2.)));
                window.paint_quad(fill(Bounds::new(point(text_x, y), size(bounds.right() - text_x, lh)), Theme::accent().opacity(0.08)));
            }
            for (i, m) in &pp.bookmark_marks {
                let y = line_ys[*i];
                let x = bounds.left() + px(4.);
                match m {
                    Some(text) => {
                        window.paint_quad(fill(Bounds::new(point(x, y + px(3.)), size(px(13.), lh - px(6.))), Theme::bookmark()).corner_radii(px(2.)));
                        let _ = text.paint(point(x + (px(13.) - text.width) / 2., y), lh, window, cx);
                    }
                    None => {
                        window.paint_quad(fill(Bounds::new(point(x + px(3.), y + px(4.)), size(px(7.), lh - px(8.))), Theme::bookmark()).corner_radii(px(1.)));
                    }
                }
            }
            for &(i, status, hover) in &pp.run_icons {
                // Centered in the column between the line numbers and the fold markers: a play
                // mark, with a green or red dot once its tests ran.
                let c = point(text_left - px(31.) - pp.run_slot / 2., line_ys[i] + lh / 2.);
                let scale = window.scale_factor();
                if hover {
                    let b = crate::icons::snapped_square(c, 18., scale);
                    window.paint_quad(fill(b, Theme::hover_row()).corner_radii(px(4.)));
                }
                crate::icons::paint_icon(window, cx, "play-solid", c, 12., Theme::run_icon());
                if let Some(ok) = status {
                    let b = point(c.x + px(4.5), c.y + px(4.5));
                    crate::icons::paint_dot(window, b, 8., if hover { Theme::hover_row() } else { Theme::editor_bg() }, None);
                    crate::icons::paint_dot(window, b, 6., if ok { Theme::run_passed() } else { Theme::error() }, None);
                }
            }
            for &(i, folded) in &pp.fold_marks {
                let c = point(bounds.left() + pp.gutter_w - px(12.), line_ys[i] + lh / 2.);
                let (name, color) = if folded { ("chevron-right", Theme::text_muted()) } else { ("chevron-down", Theme::gutter_fg()) };
                crate::icons::paint_icon(window, cx, name, c, 12., color);
            }
            for mut q in markers {
                q.bounds = crate::icons::snap_bounds(q.bounds, window.scale_factor());
                window.paint_quad(q);
            }
            for q in carets {
                window.paint_quad(q);
            }
            if !pp.sticky.is_empty() {
                let right = bounds.right() - px(scroll::BAR);
                for (text, num, y, hover) in &pp.sticky {
                    let top = bounds.top() + px(*y);
                    let row = Bounds::new(point(bounds.left(), top), size(right - bounds.left(), lh));
                    window.paint_quad(fill(row, Theme::editor_bg()));
                    if *hover {
                        window.paint_quad(fill(row, Theme::hover_row()));
                    }
                    let _ = num.paint(point(bounds.left() + pp.gutter_w - px(30.) - pp.run_slot - num.width, top), lh, window, cx);
                    window.with_content_mask(Some(ContentMask { bounds: Bounds::from_corners(point(text_left, top), point(right, top + lh)) }), |window| {
                        let _ = text.paint(point(text_x, top), lh, window, cx);
                    });
                }
                // A soft shadow under the panel.
                let bottom = bounds.top() + px(ed_sticky_h);
                for (i, a) in [0.22f32, 0.12, 0.06, 0.02].iter().enumerate() {
                    window.paint_quad(fill(Bounds::new(point(bounds.left(), bottom + px(i as f32)), size(right - bounds.left(), px(1.))), gpui::black().opacity(*a)));
                }
            }
            for mut q in thumbs {
                q.bounds = crate::icons::snap_bounds(q.bounds, window.scale_factor());
                window.paint_quad(q);
            }
            for mut q in stripe {
                q.bounds = crate::icons::snap_bounds(q.bounds, window.scale_factor());
                window.paint_quad(q);
            }
        });
        // Middle-button scrolling: follow the mouse anywhere in the window until the release.
        if self.editor.read(cx).middle_active() {
            let ed = self.editor.clone();
            window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                if phase == gpui::DispatchPhase::Bubble {
                    ed.update(cx, |ed, cx| {
                        if e.pressed_button == Some(MouseButton::Middle) {
                            ed.middle_move(e.position);
                        } else {
                            // Released where we did not see it (outside the window).
                            ed.middle_up(false, cx);
                        }
                    });
                }
            });
            let ed = self.editor.clone();
            window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                if phase == gpui::DispatchPhase::Bubble && e.button == MouseButton::Middle {
                    ed.update(cx, |ed, cx| ed.middle_up(true, cx));
                }
            });
        }
        // While dragging (selection or a scrollbar), follow the mouse anywhere in the window.
        if dragging {
            let ed = self.editor.clone();
            window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                if phase == gpui::DispatchPhase::Bubble && e.pressed_button == Some(MouseButton::Left) {
                    ed.update(cx, |ed, cx| ed.global_drag(e, window, cx));
                }
            });
            let ed = self.editor.clone();
            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                if phase == gpui::DispatchPhase::Bubble {
                    ed.update(cx, |ed, cx| {
                        ed.exec_drag_end(cx);
                        ed.end_drag();
                        cx.notify();
                    });
                }
            });
        }

        let lines = std::mem::take(&mut pp.lines);
        let rows = std::mem::take(&mut pp.rows);
        let char_w = pp.char_w;
        let me = self.editor.clone();
        self.editor.update(cx, |ed, cx| {
            ed.layout = Some(LayoutCache { bounds, caret_xy, text_left, text_x, rows, lines, char_w });
            match caret_look {
                Some(look) => ed.keep_caret_look(look, &me, window, cx),
                None => ed.stop_caret_look(),
            }
        });
    }
}
