//! Code editor view. Renders only visible lines; highlighting is computed for the visible range.

use crate::theme::{Theme, MONO_FONT};
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

mod lsp;
pub use lsp::Anchor;

pub const LINE_HEIGHT: f32 = 20.0;
pub const FONT_SIZE: f32 = 13.5;

struct LayoutCache {
    bounds: Bounds<Pixels>,
    /// Primary caret, window coordinates (for the F2 tooltip).
    caret_xy: Option<Point<Pixels>>,
    text_x: Pixels,
    first_line: usize,
    lines: Vec<ShapedLine>,
    char_w: Pixels,
}

pub struct Editor {
    pub path: Option<PathBuf>,
    pub buffer: Buffer,
    syntax: Option<SyntaxTree>,
    parsed_version: u64,
    text_cache: (u64, String),
    focus: FocusHandle,
    scroll_y: f32,
    layout: Option<LayoutCache>,
    selecting: bool,
    marked: Option<Range<usize>>,
    pub read_error: Option<String>,
    /// File contents at HEAD (None: not in git / untracked). Drives the gutter change markers.
    vcs_base: Option<String>,
    vcs_changes: Vec<brisk_git::LineChange>,
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
}

impl gpui::EventEmitter<EditorEvent> for Editor {}

fn severity_color(s: Severity) -> gpui::Hsla {
    match s {
        Severity::Error => Theme::error(),
        Severity::Warning => gpui::rgb(0xc9a032).into(),
        Severity::Unused => Theme::text_dim(),
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
        let (text, err) = match std::fs::read(path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(t) => (t, None),
                Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), Some("Not valid UTF-8; shown lossily, read-only".to_string())),
            },
            Err(e) => (String::new(), Some(e.to_string())),
        };
        let mut buffer = Buffer::new(&text);
        let lang = Lang::from_path(path);
        if lang.is_none() {
            // Line comment prefix for common non-Python files.
            buffer.line_comment = match path.extension().and_then(|e| e.to_str()) {
                Some("js" | "ts" | "tsx" | "jsx" | "rs" | "go" | "java" | "c" | "cpp" | "h" | "css" | "scss") => Some("//".into()),
                Some("toml" | "yaml" | "yml" | "sh" | "cfg" | "ini" | "txt" | "dockerfile") => Some("#".into()),
                Some("sql") => Some("--".into()),
                _ => Some("#".into()),
            };
        }
        let mut ed = Self {
            path: Some(path.to_path_buf()),
            buffer,
            syntax: lang.map(SyntaxTree::new),
            parsed_version: u64::MAX,
            text_cache: (u64::MAX, String::new()),
            focus: cx.focus_handle(),
            scroll_y: 0.0,
            layout: None,
            selecting: false,
            marked: None,
            read_error: err,
            vcs_base: None,
            vcs_changes: Vec::new(),
            vcs_version: u64::MAX,
            show_gutter: true,
            diagnostics: Vec::new(),
            by_source: [Vec::new(), Vec::new()],
            diag_version: u64::MAX,
            lint_sent: u64::MAX,
            hover: None,
            caret_tip: None,
            insight: lsp::Insight::default(),
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
            layout: None,
            selecting: false,
            marked: None,
            read_error: None,
            vcs_base: None,
            vcs_changes: Vec::new(),
            vcs_version: u64::MAX,
            show_gutter: false,
            diagnostics: Vec::new(),
            by_source: [Vec::new(), Vec::new()],
            diag_version: u64::MAX,
            lint_sent: u64::MAX,
            hover: None,
            caret_tip: None,
            insight: lsp::Insight::default(),
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

    /// Pick up changes made outside the editor (git checkout, rollback, the terminal).
    /// Unsaved edits always win. The reload is undoable, as in PyCharm.
    pub fn reload_from_disk(&mut self) -> bool {
        let Some(p) = &self.path else { return false };
        if self.buffer.is_dirty() || self.read_error.is_some() {
            return false;
        }
        let Ok(disk) = std::fs::read_to_string(p) else { return false };
        let disk = disk.replace("\r\n", "\n");
        if disk == self.buffer.text() {
            return false;
        }
        self.buffer.set_text(&disk);
        self.buffer.mark_saved();
        true
    }

    pub fn text_string(&self) -> String {
        self.buffer.text()
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

    pub fn title(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".into())
    }

    /// Write to disk if modified. PyCharm autosaves on focus loss and tab switch.
    pub fn save(&mut self) -> std::io::Result<bool> {
        if !self.buffer.is_dirty() || self.read_error.is_some() {
            return Ok(false);
        }
        if let Some(p) = &self.path {
            std::fs::write(p, self.buffer.text_for_save())?;
            self.buffer.mark_saved();
            return Ok(true);
        }
        Ok(false)
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
        let text = self.text().to_string();
        if let Some(s) = &mut self.syntax {
            s.parse(&text);
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
        let ruff = &self.by_source[0];
        let mut all = ruff.clone();
        all.extend(self.by_source[1].iter().filter(|t| !ruff.iter().any(|r| r.start == t.start && r.end == t.end)).cloned());
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
    pub fn replace_all_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = text.replace("\r\n", "\n");
        if text != self.text() {
            self.buffer.set_text(&text);
            cx.notify();
        }
    }

    pub fn caret_line_col(&self) -> (usize, usize) {
        self.buffer.pos_to_line_col(self.buffer.primary().head)
    }

    pub fn language_name(&self) -> &'static str {
        match self.syntax.as_ref().map(|s| s.lang()) {
            Some(Lang::Python) => "Python",
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
        let (line, _) = self.caret_line_col();
        let vh = self.layout.as_ref().map(|l| f32::from(l.bounds.size.height)).unwrap_or(600.0);
        let y = line as f32 * LINE_HEIGHT;
        let margin = LINE_HEIGHT * 3.0;
        if y < self.scroll_y + margin {
            self.scroll_y = (y - margin).max(0.0);
        } else if y + LINE_HEIGHT > self.scroll_y + vh - margin {
            self.scroll_y = y + LINE_HEIGHT - vh + margin;
        }
        self.clamp_scroll();
    }

    fn center_caret(&mut self) {
        let (line, _) = self.caret_line_col();
        let vh = self.layout.as_ref().map(|l| f32::from(l.bounds.size.height)).unwrap_or(600.0);
        self.scroll_y = (line as f32 * LINE_HEIGHT - vh / 3.0).max(0.0);
        self.clamp_scroll();
    }

    fn clamp_scroll(&mut self) {
        let max = (self.buffer.len_lines().saturating_sub(1)) as f32 * LINE_HEIGHT;
        self.scroll_y = self.scroll_y.clamp(0.0, max.max(0.0));
    }

    /// Execute an editor action. Returns false if this action isn't an editor action.
    pub fn perform(&mut self, action: Action, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        use Action as A;
        if self.read_error.is_some() && !matches!(action, A::Copy | A::SelectAll | A::MoveUp | A::MoveDown | A::MoveLeft | A::MoveRight | A::PageUp | A::PageDown | A::MoveDocStart | A::MoveDocEnd) {
            return true;
        }
        self.caret_tip = None;
        self.insight.doc = None;
        if action == A::EditorEscape && self.close_insight() {
            cx.notify();
            return true;
        }
        if !matches!(action, A::EditorBackspace) {
            self.insight.completion = None;
        }
        if !matches!(action, A::EditorBackspace | A::EditorDelete | A::MoveLeft | A::MoveRight | A::SelectLeft | A::SelectRight | A::MoveWordLeft | A::MoveWordRight) {
            self.insight.signature = None;
        }
        let b = &mut self.buffer;
        let page = self.layout.as_ref().map(|l| (f32::from(l.bounds.size.height) / LINE_HEIGHT) as usize).unwrap_or(30).max(1);
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
                self.scroll_y -= LINE_HEIGHT;
                self.clamp_scroll();
                keep_scroll = true;
            }
            A::ScrollDown => {
                self.scroll_y += LINE_HEIGHT;
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
            A::MoveLineUp | A::MoveStatementUp => b.move_lines(true),
            A::MoveLineDown | A::MoveStatementDown => b.move_lines(false),
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
                    let (line, _) = b.pos_to_line_col(b.primary().head);
                    format!("{}\n", b.line(line))
                } else {
                    b.selected_text()
                };
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
                    let line_paste = t.ends_with('\n') && t.matches('\n').count() == 1 && b.selections.iter().all(|s| s.is_empty());
                    if line_paste {
                        // Whole-line paste goes above the current line, like PyCharm.
                        let carets: Vec<usize> = b.selections.iter().map(|s| s.head).collect();
                        b.selections = carets.iter().map(|&p| { let (l, _) = b.pos_to_line_col(p); Selection::caret(b.line_start(l)) }).collect();
                        b.insert(&t);
                    } else {
                        b.insert(&t);
                    }
                }
            }
            A::ExtendSelection => {
                let p = b.primary();
                let r = if p.is_empty() { b.word_range_at(p.head) } else {
                    let (l1, _) = b.pos_to_line_col(p.range().start);
                    let (l2, _) = b.pos_to_line_col(p.range().end);
                    if p.range() != (b.line_start(l1)..b.line_end(l2)) { b.line_start(l1)..b.line_end(l2) } else { 0..b.len_chars() }
                };
                b.selections = vec![Selection { anchor: r.start, head: r.end, goal_col: None }];
            }
            A::ShrinkSelection => b.collapse_selections(),
            _ => return false,
        }
        if !keep_scroll {
            self.ensure_caret_visible();
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
        let line = ((y / LINE_HEIGHT).floor().max(0.0) as usize).min(self.buffer.len_lines().saturating_sub(1));
        let x = p.x - l.text_x;
        let text = self.buffer.line(line);
        let col = if line >= l.first_line && line < l.first_line + l.lines.len() {
            let shaped = &l.lines[line - l.first_line];
            byte_to_char(&text, shaped.closest_index_for_x(x.max(px(0.))))
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
        let Some(pos) = self.pos_for_point(e.position) else { return };
        if e.modifiers.control && !e.modifiers.alt && !e.modifiers.shift && e.click_count == 1 {
            cx.emit(EditorEvent::CtrlClick(pos));
            cx.notify();
            return;
        }
        if e.modifiers.alt && e.modifiers.shift || e.modifiers.alt && !e.modifiers.control {
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
        if self.selecting && e.pressed_button == Some(MouseButton::Left) {
            if let Some(pos) = self.pos_for_point(e.position) {
                self.buffer.set_caret(pos, true);
                cx.notify();
            }
            return;
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

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn on_scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let d = e.delta.pixel_delta(px(LINE_HEIGHT));
        self.scroll_y -= f32::from(d.y);
        self.clamp_scroll();
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
            .when(e > 0, |d| d.child(gpui::div().text_color(severity_color(Severity::Error)).child(format!("✗ {e}"))))
            .when(w > 0, |d| d.child(gpui::div().text_color(severity_color(Severity::Warning)).child(format!("⚠ {w}"))))
            .when(e + w == 0, |d| d.child(gpui::div().text_color(Theme::vcs_added()).child("✓")))
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
                cx.emit(EditorEvent::ShowProblems);
            }))
    }

    fn render_tooltip(&self) -> Option<gpui::AnyElement> {
        let l = self.layout.as_ref()?;
        let (i, at) = match (self.hover, self.caret_tip) {
            (Some((i, p)), _) => (i, p + point(px(0.), px(LINE_HEIGHT))),
            (None, Some(i)) => (i, l.caret_xy? + point(px(0.), px(LINE_HEIGHT + 2.))),
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
                .child(gpui::div().flex().gap(px(6.)).child(gpui::div().text_color(severity_color(d.severity)).child(match d.severity {
                    Severity::Error => "✗",
                    _ => "⚠",
                })).child(gpui::div().text_color(Theme::text()).child(d.title())))
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
        gpui::div()
            .id("editor")
            .key_context("Editor")
            .track_focus(&self.focus)
            .size_full()
            .cursor(gpui::CursorStyle::IBeam)
            .bg(Theme::editor_bg())
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            // Completion, Quick Doc and Parameter Info close on a click anywhere outside the editor.
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.close_insight() | this.caret_tip.take().is_some() {
                    cx.notify();
                }
            }))
            .relative()
            .child(EditorElement { editor: cx.entity() })
            .when(self.is_python() && self.diag_version != u64::MAX, |d| d.child(self.render_widget(cx)))
            .when_some(self.render_tooltip(), |d, t| d.child(t))
            .children(self.render_insight(cx))
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
        self.marked.as_ref().map(|m| rope.char_to_utf16_cu(m.start)..rope.char_to_utf16_cu(m.end))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(&mut self, _range: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_error.is_some() {
            return;
        }
        if let Some(m) = self.marked.take() {
            self.buffer.selections = vec![Selection { anchor: m.start, head: m.end, goal_col: None }];
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
            self.buffer.selections = vec![Selection { anchor: m.start, head: m.end, goal_col: None }];
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
        let y = l.bounds.top() + px(line as f32 * LINE_HEIGHT - self.scroll_y);
        let x = if line >= l.first_line && line < l.first_line + l.lines.len() {
            let text = self.buffer.line(line);
            l.text_x + l.lines[line - l.first_line].x_for_index(char_to_byte(&text, col))
        } else {
            l.text_x
        };
        Some(Bounds::new(point(x, y), size(px(2.), px(LINE_HEIGHT))))
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

// --- Rendering -------------------------------------------------------------------

struct EditorElement {
    editor: Entity<Editor>,
}

struct Prepaint {
    lines: Vec<ShapedLine>,
    numbers: Vec<ShapedLine>,
    first_line: usize,
    gutter_w: Pixels,
    char_w: Pixels,
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
        let font_size = px(FONT_SIZE);
        let mono = font(MONO_FONT);
        let run = |len: usize, color: gpui::Hsla| TextRun { len, font: mono.clone(), color, background_color: None, underline: None, strikethrough: None };

        self.editor.update(cx, |ed, _| {
            ed.reparse();
            ed.refresh_vcs();
        });
        let ed = self.editor.read(cx);
        let b = &ed.buffer;
        let n_lines = b.len_lines();
        let first_line = ((ed.scroll_y / LINE_HEIGHT).floor() as usize).min(n_lines.saturating_sub(1));
        let visible = (f32::from(bounds.size.height) / LINE_HEIGHT).ceil() as usize + 1;
        let last_line = (first_line + visible).min(n_lines);

        let digit = window.text_system().shape_line("0".into(), font_size, &[run(1, Theme::text())], None);
        let char_w = digit.width;
        let digits = n_lines.to_string().len().max(3);
        let gutter_w = if ed.show_gutter { char_w * digits as f32 + px(32.) } else { px(0.) };

        // Highlights for the visible byte range in one query.
        let rope = b.rope();
        let byte_lo = rope.char_to_byte(b.line_start(first_line));
        let byte_hi = if last_line < n_lines { rope.char_to_byte(b.line_start(last_line)) } else { rope.len_bytes() };
        let spans = match &ed.syntax {
            Some(s) => s.highlights(&ed.text_cache.1, byte_lo..byte_hi),
            None if ed.is_sql() => brisk_syntax::highlight_sql(&ed.text_cache.1, byte_lo..byte_hi),
            None => Vec::new(),
        };

        let (caret_line, _) = ed.caret_line_col();
        let mut lines = Vec::with_capacity(last_line - first_line);
        let mut numbers = Vec::with_capacity(last_line - first_line);
        let mut si = 0;
        for l in first_line..last_line {
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
            lines.push(window.text_system().shape_line(SharedString::from(text), font_size, &runs, None));

            if !ed.show_gutter {
                continue;
            }
            let num = (l + 1).to_string();
            let color = if l == caret_line { Theme::gutter_fg_active() } else { Theme::gutter_fg() };
            numbers.push(window.text_system().shape_line(SharedString::from(num.clone()), font_size, &[run(num.len(), color)], None));
        }
        Prepaint { lines, numbers, first_line, gutter_w, char_w }
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), pp: &mut Prepaint, window: &mut Window, cx: &mut App) {
        let focus = self.editor.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.editor.clone()), cx);
        let focused = focus.is_focused(window);

        let ed = self.editor.read(cx);
        let b = &ed.buffer;
        let lh = px(LINE_HEIGHT);
        let text_x = bounds.left() + pp.gutter_w + px(8.);
        let y_of = |l: usize| bounds.top() + px(l as f32 * LINE_HEIGHT - ed.scroll_y);
        let first = pp.first_line;
        let last = first + pp.lines.len();
        let x_of = |line: usize, col: usize| -> Pixels {
            if line >= first && line < last {
                let t = b.line(line);
                text_x + pp.lines[line - first].x_for_index(char_to_byte(&t, col))
            } else {
                text_x
            }
        };

        // Compute geometry first (borrows the editor), then paint (needs &mut App).
        let mut under: Vec<gpui::PaintQuad> = Vec::new();
        let mut carets: Vec<gpui::PaintQuad> = Vec::new();
        // Current line.
        if b.selections.iter().all(|s| s.is_empty()) {
            for s in &b.selections {
                let (l, _) = b.pos_to_line_col(s.head);
                if l >= first && l < last {
                    under.push(fill(Bounds::new(point(bounds.left(), y_of(l)), size(bounds.size.width, lh)), Theme::current_line()));
                }
            }
        }
        // Indent guides: one at each enclosing indentation level (columns 0, 4, 8, ...).
        let unit = b.indent_unit.chars().count().max(1);
        for i in 0..pp.lines.len() {
            let l = first + i;
            let t = b.line(l);
            let ws = if t.trim().is_empty() { 0 } else { t.chars().take_while(|c| *c == ' ').count() };
            for c in (0..ws).step_by(unit) {
                let x = text_x + pp.char_w * c as f32;
                under.push(fill(Bounds::new(point(x, y_of(l)), size(px(1.), lh)), Theme::indent_guide()));
            }
        }
        // Selections.
        for s in &b.selections {
            if s.is_empty() {
                continue;
            }
            let r = s.range();
            let (l1, c1) = b.pos_to_line_col(r.start);
            let (l2, c2) = b.pos_to_line_col(r.end);
            for l in l1.max(first)..=l2.min(last.saturating_sub(1)) {
                let sx = if l == l1 { x_of(l, c1) } else { text_x };
                let ex = if l == l2 { x_of(l, c2) } else { x_of(l, b.line_len(l)) + pp.char_w };
                if ex > sx {
                    under.push(fill(Bounds::from_corners(point(sx, y_of(l)), point(ex, y_of(l) + lh)), Theme::selection()));
                }
            }
        }
        if focused {
            for s in &b.selections {
                let (l, c) = b.pos_to_line_col(s.head);
                if l >= first && l < last {
                    carets.push(fill(Bounds::new(point(x_of(l, c), y_of(l)), size(px(2.), lh)), Theme::caret()));
                }
            }
        }
        // VCS change markers (PyCharm: colored bar right of the line numbers).
        let mut markers: Vec<gpui::PaintQuad> = Vec::new();
        let mx = bounds.left() + pp.gutter_w - px(9.);
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
        let line_ys: Vec<Pixels> = (0..pp.lines.len()).map(|i| y_of(first + i)).collect();
        // Problems: wavy underlines (errors and warnings) and marks on the right edge.
        let mut waves: Vec<(Point<Pixels>, Pixels, gpui::Hsla)> = Vec::new();
        let mut stripe: Vec<gpui::PaintQuad> = Vec::new();
        let total = (b.len_lines().max(1) as f32 * LINE_HEIGHT).max(f32::from(bounds.size.height));
        for d in &ed.diagnostics {
            let color = severity_color(d.severity);
            if d.severity != Severity::Unused {
                let r = ed.diag_range(d);
                let (l1, c1) = b.pos_to_line_col(r.start);
                let (l2, c2) = b.pos_to_line_col(r.end);
                for l in l1.max(first)..=l2.min(last.saturating_sub(1)) {
                    let sx = if l == l1 { x_of(l, c1) } else { text_x };
                    let mut ex = if l == l2 { x_of(l, c2) } else { x_of(l, b.line_len(l)) };
                    if ex <= sx {
                        ex = sx + pp.char_w;
                    }
                    waves.push((point(sx, y_of(l) + lh - px(3.)), ex - sx, color));
                }
            }
            let y = bounds.top() + px(d.start.line as f32 * LINE_HEIGHT / total * f32::from(bounds.size.height));
            stripe.push(fill(Bounds::new(point(bounds.right() - px(9.), y), size(px(7.), px(3.))), color));
        }
        let caret_xy = {
            let (l, c) = b.pos_to_line_col(b.primary().head);
            (l >= first && l < last).then(|| point(x_of(l, c), y_of(l)))
        };

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for q in under {
                window.paint_quad(q);
            }
            for (line, y) in pp.lines.iter().zip(&line_ys) {
                let _ = line.paint(point(text_x, *y), lh, window, cx);
            }
            for (o, w, color) in waves {
                window.paint_underline(o, w, &gpui::UnderlineStyle { color: Some(color), thickness: px(1.), wavy: true });
            }
            // Gutter (painted over text so horizontal overflow never covers line numbers).
            window.paint_quad(fill(Bounds::new(bounds.origin, size(pp.gutter_w, bounds.size.height)), Theme::editor_bg()));
            for (n, y) in pp.numbers.iter().zip(&line_ys) {
                let x = bounds.left() + pp.gutter_w - px(16.) - n.width;
                let _ = n.paint(point(x, *y), lh, window, cx);
            }
            for q in markers {
                window.paint_quad(q);
            }
            for q in carets {
                window.paint_quad(q);
            }
            for q in stripe {
                window.paint_quad(q);
            }
        });

        let lines = std::mem::take(&mut pp.lines);
        let char_w = pp.char_w;
        self.editor.update(cx, |ed, _| {
            ed.layout = Some(LayoutCache { bounds, caret_xy, text_x, first_line: first, lines, char_w });
        });
    }
}
