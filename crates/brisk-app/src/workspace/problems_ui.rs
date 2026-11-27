//! Inspections: ruff checks of open Python files as you type, the Problems tool window
//! (Alt+6, "File" and "Project" tabs), F2 / Shift+F2 and Alt+Enter quick fixes.

use super::{BottomTool, PopupItem, PopupKind, Target, Workspace};
use crate::editor::{DiagSource, Editor, EditorEvent};
use crate::theme::ui_px;
use crate::theme::{mono_font, Theme};
use brisk_lint::{Diagnostic, Severity};
use gpui::prelude::*;
use gpui::{div, px, uniform_list, ClickEvent, Context, Entity, EntityId, FocusHandle, Focusable, UniformListScrollHandle, Window};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Pause after the last keystroke before re-checking, like PyCharm's daemon delay.
const LINT_DELAY: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ProblemsTab {
    File,
    Project,
}

/// Alt+Enter choices.
#[derive(Clone)]
pub(super) enum Intent {
    Fix(Diagnostic),
    FixAllOfCode(String),
    FixAll,
    Suppress(Diagnostic),
    /// Ctrl+J: insert this live template.
    Template(&'static str),
    /// Ctrl+Alt+T: surround with `SURROUNDERS[i]`.
    Surround(usize),
    /// Generate / Override: insert this member into the class at the caret.
    Member(String),
    /// Generate: open Override Methods (`false`) or Implement Methods (`true`).
    Overrides(bool),
    /// Ctrl+Shift+Delete: unwrap or remove the enclosing statement.
    Unwrap(brisk_syntax::Unwrap),
    /// Ctrl+Alt+C: extract the selection into a module constant; `true`: every occurrence.
    ExtractConstant(bool),
    /// Extract Variable / Constant: this expression (chars), then extract (`true`: constant).
    PickExpression(std::ops::Range<usize>, bool),
    /// Byte-range edits of the active editor's text (Inline), one undo step.
    Edits(Vec<(std::ops::Range<usize>, String)>),
    /// The Rename popup's Preview with this new name.
    RenamePreview(String),
    /// The Rename popup's "Search in comments and strings" (the name typed so far).
    RenameText(String),
    /// Ctrl+Alt+V: extract the selection into a variable; `true`: every occurrence.
    ExtractVariable(bool),
    /// A language server quick fix (ty: import a name, qualify it): title and edits.
    Lsp(String, Vec<brisk_lsp::FileEdits>),
    /// Biome's safe fixes for the whole file.
    BiomeFixAll,
}

/// ty's action titles the way PyCharm words them: "import collections.defaultdict" ->
/// "Import 'collections.defaultdict'".
fn lsp_title(t: &str) -> String {
    if let Some(rest) = t.strip_prefix("import ") {
        return format!("Import '{rest}'");
    }
    if let Some(rest) = t.strip_prefix("qualify ") {
        return format!("Qualify as '{rest}'");
    }
    let mut c = t.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// The return type in ty's hover of a function: `def f(a) -> list[int]` → `list[int]`.
fn return_type(hover: &str) -> Option<String> {
    let code = hover.split("```").find(|b| b.contains("def "))?;
    let sig = &code[code.find("def ")?..];
    let t = sig[sig.rfind("->")? + 2..].trim().trim_end_matches(':').trim();
    (!t.is_empty() && !t.contains('\n')).then(|| t.to_string())
}

/// Type Info: the type in ty's hover of a name: `def f(a: int) -> str` → `(a: int) -> str`,
/// `class C` → `type[C]`, a variable → its type.
pub(super) fn type_of_hover(hover: &str) -> Option<String> {
    let code = hover.split("```").nth(1)?;
    // The fence's language (`python`, `typescript`) on its first line.
    let code = match code.split_once('\n') {
        Some((lang, rest)) if lang.chars().all(|c| c.is_ascii_alphanumeric()) => rest,
        _ => code,
    }
    .trim();
    let one = code.split_whitespace().collect::<Vec<_>>().join(" ").replace("( ", "(").replace(", )", ")").replace(" )", ")");
    // TypeScript: `const u: User`, `let n: number`, `(parameter) x: string`.
    let one =
        ["const ", "let ", "var ", "(parameter) ", "(property) ", "(local var) "].iter().find_map(|k| one.strip_prefix(k)).map_or(one.clone(), str::to_string);
    if let Some(rest) = one.strip_prefix("def ").or_else(|| one.strip_prefix("async def ")) {
        return Some(rest[rest.find('(')?..].to_string());
    }
    if let Some(rest) = one.strip_prefix("class ") {
        let name = rest.split(['(', ':']).next()?.trim();
        return Some(format!("type[{name}]"));
    }
    let t = match one.split_once(": ") {
        Some((name, t)) if name.chars().all(|c| c.is_alphanumeric() || c == '_') => t,
        _ => &one,
    };
    (!t.is_empty()).then(|| t.to_string())
}

/// The type in ty's hover of a value: a variable's type, or a called function's return type.
pub(super) fn value_type(hover: &str) -> Option<String> {
    let code = hover.split("```").nth(1)?.trim_start_matches("python").trim();
    if code.contains("def ") {
        return return_type(hover);
    }
    let line = code.lines().next()?.trim();
    let t = match line.split_once(": ") {
        Some((name, t)) if name.chars().all(|c| c.is_alphanumeric() || c == '_') => t,
        _ => line,
    };
    (!t.is_empty() && !t.contains("Unknown")).then(|| t.to_string())
}

pub(super) struct Problems {
    pub tab: ProblemsTab,
    pub focus: FocusHandle,
    pub selected: usize,
    pub scroll: UniformListScrollHandle,
    /// Last whole-project check; `None` until the Project tab is first opened. Set through
    /// `set_project`, which also groups it into `project_rows`.
    pub project: Option<Result<Vec<Diagnostic>, String>>,
    /// The Project tab's rows (file headers and problems, as indices into `project`), built
    /// once per check instead of on every frame and every key.
    project_rows: std::rc::Rc<Vec<Row>>,
    pub project_running: bool,
    /// Why ruff could not run (not installed, bad config).
    pub error: Option<String>,
    lint_gen: HashMap<EntityId, u64>,
    pub(super) intents: Vec<PopupItem>,
    /// Popup title when the list is not Alt+Enter's (Surround With, Insert Live Template).
    pub(super) intents_title: Option<&'static str>,
    /// Actions on Save found no ruff and said so (once per window).
    save_actions_warned: bool,
    /// Project tab: files whose problems are folded under their header.
    folded: HashSet<PathBuf>,
}

impl Problems {
    pub fn new(cx: &mut gpui::App) -> Self {
        Problems {
            tab: ProblemsTab::File,
            focus: cx.focus_handle(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            project: None,
            project_rows: Default::default(),
            project_running: false,
            error: None,
            lint_gen: HashMap::new(),
            intents: Vec::new(),
            intents_title: None,
            save_actions_warned: false,
            folded: HashSet::new(),
        }
    }

    pub fn set_project(&mut self, res: Option<Result<Vec<Diagnostic>, String>>) {
        self.project = res;
        self.rebuild_project_rows();
    }

    fn rebuild_project_rows(&mut self) {
        let mut rows = Vec::new();
        if let Some(Ok(all)) = &self.project {
            let mut i = 0;
            while i < all.len() {
                let n = all[i..].iter().take_while(|d| d.file == all[i].file).count();
                rows.push(Row::File(i, n));
                if !self.folded.contains(&all[i].file) {
                    rows.extend((i..i + n).map(Row::Problem));
                }
                i += n;
            }
        }
        self.project_rows = std::rc::Rc::new(rows);
    }

    /// Fold or unfold the file of the Project tab's row `i` (a header or one of its
    /// problems); the selection moves to the header.
    fn toggle_fold(&mut self, i: usize) {
        let Some(Ok(all)) = &self.project else { return };
        let first = match self.project_rows.get(i) {
            Some(Row::File(first, _)) => *first,
            Some(Row::Problem(di)) => *di,
            None => return,
        };
        let Some(file) = all.get(first).map(|d| d.file.clone()) else { return };
        if !self.folded.remove(&file) {
            self.folded.insert(file.clone());
        }
        self.rebuild_project_rows();
        let Some(Ok(all)) = &self.project else { return };
        self.selected = self.project_rows.iter().position(|r| matches!(r, Row::File(f, _) if all[*f].file == file)).unwrap_or(0);
    }

    /// Whether the Project tab's header row for `file` is folded.
    fn is_folded(&self, file: &std::path::Path) -> bool {
        self.folded.contains(file)
    }
}

/// A row of the Problems list, by index into the tab's diagnostics (the active editor's, or
/// the project check's): a file header (its first problem, problem count), or a problem.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Row {
    File(usize, usize),
    Problem(usize),
}

fn icon(s: Severity) -> (&'static str, gpui::Hsla) {
    match s {
        Severity::Error => ("✗", Theme::error()),
        Severity::Warning => ("⚠", gpui::rgb(0xc9a032).into()),
        Severity::Unused => ("⚠", Theme::text_muted()),
    }
}

impl Workspace {
    /// ruff next to the project interpreter, else next to any other Python found, else on PATH.
    /// The ruff to run. The lookup stats a few files per known interpreter and every `PATH`
    /// entry, and lint asks for it on each pause in typing, so the answer is kept for a few
    /// seconds per set of interpreters (a `pip install ruff` is still picked up soon).
    fn ruff(&self, cx: &gpui::App) -> Option<PathBuf> {
        type Key = (PathBuf, Option<PathBuf>, usize, Option<usize>);
        thread_local! {
            static CACHE: std::cell::RefCell<Option<(Key, std::time::Instant, Option<PathBuf>)>> = const { std::cell::RefCell::new(None) };
        }
        const KEEP: Duration = Duration::from_secs(10);
        let project = self.interpreter(cx).map(|i| i.path);
        let key: Key = (self.root.clone(), project.clone(), self.env.read(cx).venvs.len(), self.env.read(cx).all.as_ref().map(Vec::len));
        if let Some(hit) = CACHE.with_borrow(|c| c.as_ref().filter(|(k, at, _)| *k == key && at.elapsed() < KEEP).map(|(_, _, r)| r.clone())) {
            return hit;
        }
        let others = self.env.read(cx).venvs.iter().chain(self.env.read(cx).all.iter().flatten()).map(|i| i.path.clone());
        let found = project.into_iter().chain(others).find_map(|p| brisk_lint::find_ruff(Some(&p))).or_else(|| brisk_lint::find_ruff(None));
        CACHE.set(Some((key, std::time::Instant::now(), found.clone())));
        found
    }

    /// Interpreter discovery finished: check again files that waited for ruff.
    pub(super) fn relint_all(&mut self, cx: &mut Context<Self>) {
        let eds: Vec<_> = self.editors().cloned().collect();
        for ed in eds {
            ed.update(cx, |e, _| e.lint_sent = u64::MAX);
            self.schedule_lint(ed, Duration::ZERO, cx);
        }
    }

    /// Watch a newly opened editor: check it now and again after each pause in typing.
    pub(super) fn watch_editor(&mut self, ed: &Entity<Editor>, window: &mut Window, cx: &mut Context<Self>) {
        self.push_test_results(ed, cx);
        self.push_coverage(ed, cx);
        cx.observe(ed, |this: &mut Self, ed, cx| {
            let v = ed.read(cx).buffer.version();
            let checked = ed.read(cx).is_python() || ed.read(cx).path.as_deref().is_some_and(|p| super::ts_ui::is_ts_path(p) || this.biome_wanted(p));
            if ed.read(cx).lint_sent != v && checked {
                this.schedule_lint(ed.clone(), LINT_DELAY, cx);
            }
            this.structure_follow_caret(&ed, cx);
            this.track_place(&ed, cx);
        })
        .detach();
        cx.subscribe_in(ed, window, |this: &mut Self, ed, ev: &EditorEvent, window, cx| match ev {
            EditorEvent::ShowProblems => {
                this.problems.tab = ProblemsTab::File;
                this.bottom = Some(BottomTool::Problems);
                window.focus(&this.problems.focus.clone());
                cx.notify();
            }
            EditorEvent::ShowContextActions => this.show_intentions(window, cx),
            EditorEvent::ContextMenu(at) => this.open_editor_menu(ed.clone(), *at, cx),
            EditorEvent::BreakpointsChanged => this.breakpoints_changed(ed.clone(), cx),
            EditorEvent::EditBreakpoint(line) => this.edit_breakpoint(ed.clone(), *line, cx),
            EditorEvent::SetNextStatement(line) => this.set_next_statement(ed.clone(), *line, cx),
            EditorEvent::GutterMenu(at) => this.open_gutter_menu(ed.clone(), *at, cx),
            EditorEvent::RunMark(line, at) => this.run_mark_menu(ed.clone(), *line, *at, cx),
            EditorEvent::ShowCommit(hash) => this.show_annotation_commit(ed.clone(), hash.clone(), window, cx),
            EditorEvent::ShowVcsDiff => {
                if let Some(c) = this.editor_file_change(ed, cx) {
                    this.show_change_diff(&c, window, cx);
                }
            }
            ev => this.lsp_editor_event(ed.clone(), ev, window, cx),
        })
        .detach();
        self.schedule_lint(ed.clone(), Duration::ZERO, cx);
    }

    fn schedule_lint(&mut self, ed: Entity<Editor>, delay: Duration, cx: &mut Context<Self>) {
        // Forget closed editors so the map does not grow with every tab ever opened.
        let open: std::collections::HashSet<_> = self.editors().map(|e| e.entity_id()).collect();
        self.problems.lint_gen.retain(|id, _| open.contains(id) || *id == ed.entity_id());
        let gen = self.problems.lint_gen.entry(ed.entity_id()).or_default();
        *gen += 1;
        let my = *gen;
        let root = self.root.clone();
        let ruff = self.ruff(cx);
        let bg = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                bg.timer(delay).await;
            }
            // Superseded by a newer keystroke?
            let Ok(Some((version, text, path))) = this.update(cx, |this, cx| {
                if this.problems.lint_gen.get(&ed.entity_id()) != Some(&my) {
                    return None;
                }
                let path = ed.read(cx).path.clone()?;
                let (v, t) = ed.update(cx, |e, _| e.take_lint_request())?;
                if ed.read(cx).is_python() || super::ts_ui::is_ts_path(&path) {
                    this.ty_check(ed.clone(), path.clone(), v, &t, cx);
                }
                if this.biome_wanted(&path) {
                    this.biome_check(ed.clone(), path.clone(), v, &t, cx);
                } else {
                    // Biome turned off in Settings: its old problems go.
                    ed.update(cx, |e, _| e.set_diagnostics(DiagSource::Biome, v, Vec::new()));
                }
                // ruff is for Python only.
                if !ed.read(cx).is_python() {
                    return None;
                }
                Some((v, t, path))
            }) else {
                return;
            };
            let Some(ruff) = ruff else {
                let _ = this.update(cx, |this, cx| {
                    // Still looking for interpreters: discovery re-checks when it is done.
                    if this.env.read(cx).all.is_none() {
                        ed.update(cx, |e, _| e.lint_sent = u64::MAX);
                        return;
                    }
                    this.problems.error = Some("ruff not found: install it in the project interpreter (pip install ruff)".into());
                    cx.notify();
                });
                return;
            };
            let res = bg.spawn(async move { brisk_lint::check_text(&ruff, &root, &path, &text) }).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(diags) => {
                        this.problems.error = None;
                        ed.update(cx, |e, cx| {
                            if e.set_diagnostics(DiagSource::Ruff, version, diags) {
                                cx.notify();
                            }
                        });
                    }
                    Err(e) => this.problems.error = Some(format!("ruff: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-check the whole project in the background (Project tab).
    pub(super) fn check_project(&mut self, cx: &mut Context<Self>) {
        if self.problems.project_running {
            return;
        }
        let Some(ruff) = self.ruff(cx) else {
            if self.env.read(cx).all.is_none() {
                self.problems.set_project(None);
                return;
            }
            self.problems.set_project(Some(Err("ruff not found: install it in the project interpreter (pip install ruff)".into())));
            return;
        };
        self.problems.project_running = true;
        let root = self.root.clone();
        let task = cx.background_executor().spawn(async move { brisk_lint::check_project(&ruff, &root) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.problems.project_running = false;
                this.problems.set_project(Some(res));
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn problems_focused(&self, window: &Window) -> bool {
        self.problems.focus.is_focused(window)
    }

    /// Alt+6, PyCharm semantics: show and focus; hide when it already has focus.
    pub(super) fn toggle_problems(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.bottom == Some(BottomTool::Problems) && self.problems_focused(window) {
            self.bottom = None;
            self.focus_editor(window, cx);
            return;
        }
        self.bottom = Some(BottomTool::Problems);
        window.focus(&self.problems.focus.clone());
        if self.problems.tab == ProblemsTab::Project && self.problems.project.is_none() {
            self.check_project(cx);
        }
    }

    fn file_count(&self, cx: &gpui::App) -> usize {
        self.active_editor().map_or(0, |e| e.read(cx).diagnostics.len())
    }

    /// The current tab's rows: indices only, nothing cloned.
    fn rows(&self, cx: &gpui::App) -> std::rc::Rc<Vec<Row>> {
        match self.problems.tab {
            ProblemsTab::File => std::rc::Rc::new((0..self.file_count(cx)).map(Row::Problem).collect()),
            ProblemsTab::Project => self.problems.project_rows.clone(),
        }
    }

    /// The diagnostic a row's index points at, in the current tab.
    fn problem_at(&self, i: usize, cx: &gpui::App) -> Option<Diagnostic> {
        match self.problems.tab {
            ProblemsTab::File => self.active_editor().and_then(|e| e.read(cx).diagnostics.get(i).cloned()),
            ProblemsTab::Project => match &self.problems.project {
                Some(Ok(all)) => all.get(i).cloned(),
                _ => None,
            },
        }
    }

    /// Open a problem's file with the caret on it.
    pub(super) fn open_problem(&mut self, d: &Diagnostic, window: &mut Window, cx: &mut Context<Self>) {
        let same = self.active_editor().is_some_and(|e| e.read(cx).path.as_deref() == Some(d.file.as_path()));
        if !same {
            self.open_file(&d.file.clone(), window, cx);
        }
        if let Some(ed) = self.active_editor().cloned() {
            ed.update(cx, |e, cx| e.select_on_line(d.start.line, d.start.col..d.start.col, cx));
            let f = ed.read(cx).focus_handle(cx);
            window.focus(&f);
        }
    }

    pub(super) fn problems_key(&mut self, k: &gpui::Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let rows = self.rows(cx);
        let n = rows.len();
        match k.key.as_str() {
            "up" => self.problems.selected = self.problems.selected.saturating_sub(1),
            "down" => self.problems.selected = (self.problems.selected + 1).min(n.saturating_sub(1)),
            "home" => self.problems.selected = 0,
            "end" => self.problems.selected = n.saturating_sub(1),
            "enter" | "f4" => {
                match rows.get(self.problems.selected) {
                    Some(&Row::Problem(di)) => {
                        if let Some(d) = self.problem_at(di, cx) {
                            self.open_problem(&d, window, cx);
                        }
                    }
                    Some(Row::File(..)) => self.problems.toggle_fold(self.problems.selected),
                    None => {}
                }
                cx.notify();
                return true;
            }
            // A header folds with Left and unfolds with Right; Left on a problem folds its file.
            "left" | "right" if self.problems.tab == ProblemsTab::Project => {
                let folded = match rows.get(self.problems.selected) {
                    Some(&Row::File(first, _)) => self.problem_at(first, cx).is_some_and(|d| self.problems.is_folded(&d.file)),
                    Some(Row::Problem(_)) => false,
                    None => return true,
                };
                if folded == (k.key == "right") {
                    self.problems.toggle_fold(self.problems.selected);
                }
            }
            "escape" => {
                self.focus_editor(window, cx);
                return true;
            }
            _ => return false,
        }
        self.problems.scroll.scroll_to_item(self.problems.selected, gpui::ScrollStrategy::Center);
        cx.notify();
        true
    }

    fn set_problems_tab(&mut self, tab: ProblemsTab, window: &mut Window, cx: &mut Context<Self>) {
        self.problems.tab = tab;
        self.problems.selected = 0;
        window.focus(&self.problems.focus.clone());
        if tab == ProblemsTab::Project && self.problems.project.is_none() {
            self.check_project(cx);
        }
        cx.notify();
    }

    pub(super) fn render_problems(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let rows = self.rows(cx);
        let n = rows.len();
        let file_name = self.active_editor().and_then(|e| e.read(cx).path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()));
        let file_count = self.file_count(cx);
        let project_count = match &self.problems.project {
            Some(Ok(v)) => Some(v.len()),
            _ => None,
        };
        let tab = self.problems.tab;
        let tab_button = |id: &'static str, label: String, t: ProblemsTab, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px(px(8.))
                .py(px(3.))
                .rounded(px(5.))
                .cursor_pointer()
                .text_color(if tab == t { Theme::text() } else { Theme::text_muted() })
                .when(tab == t, |d| d.bg(Theme::selection_row()))
                .when(!(tab == t), |d| d.hover(|s| s.bg(Theme::hover_row())))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| this.set_problems_tab(t, window, cx)))
        };
        let file_label = match &file_name {
            Some(f) => format!("File: {f}  {file_count}"),
            None => "File".into(),
        };
        let project_label = match project_count {
            Some(c) => format!("Project  {c}"),
            None => "Project".into(),
        };
        let header = crate::ui_kit::tool_header()
            .gap(px(2.))
            .child(crate::ui_kit::tool_title("Problems"))
            .child(tab_button("pr-file", file_label, ProblemsTab::File, cx))
            .child(tab_button("pr-project", project_label, ProblemsTab::Project, cx))
            .when(tab == ProblemsTab::Project, |d| {
                d.child(
                    div()
                        .id("pr-refresh")
                        .px(px(8.))
                        .py(px(2.))
                        .rounded(px(5.))
                        .cursor_pointer()
                        .text_color(Theme::text_muted())
                        .hover(|s| s.bg(Theme::hover_row()))
                        .child(if self.problems.project_running {
                            div().child("Checking…").into_any_element()
                        } else {
                            crate::icons::glyph("↻", 14., Theme::text_muted())
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.check_project(cx);
                            cx.notify();
                        })),
                )
            });
        let empty = match (tab, &self.problems.error, &self.problems.project) {
            (_, Some(e), _) if tab == ProblemsTab::File => Some(e.clone()),
            (ProblemsTab::Project, _, Some(Err(e))) => Some(e.clone()),
            (ProblemsTab::Project, _, None) => Some("Checking the project…".into()),
            (ProblemsTab::File, _, _) if file_name.is_none() => Some("No file is open".into()),
            _ if n == 0 => Some("No problems found".into()),
            _ => None,
        };
        let focus = self.problems.focus.clone();
        let body: gpui::AnyElement = match empty {
            Some(msg) => {
                let icon = if n == 0 && msg == "No problems found" { "circle-check" } else { "triangle-alert" };
                crate::ui_kit::empty_state(icon, msg).into_any_element()
            }
            None => uniform_list(
                "problems",
                n,
                cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                    let rows = this.rows(cx);
                    let focused = this.problems.focus.is_focused(window);
                    let root = this.root.clone();
                    // Only the visible rows' diagnostics are cloned.
                    let visible: Vec<(usize, Row, Diagnostic)> = range
                        .filter_map(|i| {
                            let r = *rows.get(i)?;
                            let di = match r {
                                Row::File(first, _) => first,
                                Row::Problem(di) => di,
                            };
                            Some((i, r, this.problem_at(di, cx)?))
                        })
                        .collect();
                    visible
                        .into_iter()
                        .map(|(i, r, d)| {
                            let selected = this.problems.selected == i;
                            let base = div()
                                .id(("problem", i))
                                .h(ui_px(22.))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .text_size(ui_px(13.))
                                .w_full()
                                .when(selected, |d| d.bg(if focused { Theme::selection_row() } else { Theme::hover_row() }))
                                .when(!selected, |d| d.hover(|s| s.bg(Theme::hover_row())))
                                .cursor_pointer();
                            match r {
                                Row::File(_, count) => {
                                    let rel = brisk_run::relativize(&root, &d.file);
                                    let (name, dir) = match rel.rsplit_once('/') {
                                        Some((d, n)) => (n.to_string(), d.to_string()),
                                        None => (rel.clone(), String::new()),
                                    };
                                    let open = !this.problems.is_folded(&d.file);
                                    base.pl(px(10.))
                                        .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                                            this.problems.selected = i;
                                            window.focus(&this.problems.focus.clone());
                                            if e.click_count() >= 2 {
                                                this.problems.toggle_fold(i);
                                            }
                                            cx.notify();
                                        }))
                                        .child(div().id(("problem-fold", i)).child(crate::icons::tree_arrow(true, open, Theme::text_muted())).on_click(
                                            cx.listener(move |this, e: &ClickEvent, window, cx| {
                                                cx.stop_propagation();
                                                if e.click_count() == 1 {
                                                    window.focus(&this.problems.focus.clone());
                                                    this.problems.toggle_fold(i);
                                                    cx.notify();
                                                }
                                            }),
                                        ))
                                        .child(name)
                                        .child(div().text_color(Theme::text_dim()).text_size(ui_px(12.)).child(dir))
                                        .child(
                                            div()
                                                .text_color(Theme::text_muted())
                                                .text_size(ui_px(12.))
                                                .child(format!("{count} problem{}", if count == 1 { "" } else { "s" })),
                                        )
                                }
                                Row::Problem(_) => {
                                    let (g, c) = icon(d.severity);
                                    let d2 = d.clone();
                                    base.pl(px(if this.problems.tab == ProblemsTab::Project { 32. } else { 12. }))
                                        .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                                            this.problems.selected = i;
                                            window.focus(&this.problems.focus.clone());
                                            if e.click_count() >= 2 {
                                                this.open_problem(&d2, window, cx);
                                            }
                                            cx.notify();
                                        }))
                                        .child(div().w(px(14.)).flex().items_center().child(crate::icons::glyph(g, 14., c)))
                                        .child(div().text_color(Theme::text()).overflow_hidden().whitespace_nowrap().child(d.title()))
                                        .child(
                                            div()
                                                .text_color(Theme::text_dim())
                                                .font_family(mono_font())
                                                .text_size(ui_px(12.))
                                                .child(format!(":{}", d.start.line + 1)),
                                        )
                                }
                            }
                        })
                        .collect()
                }),
            )
            .track_scroll(self.problems.scroll.clone())
            .flex_1()
            .into_any_element(),
        };
        div().id("problems-root").track_focus(&focus).key_context("ProblemsView").size_full().flex().flex_col().child(header).child(body).into_any_element()
    }

    // --- Alt+Enter, F2 ------------------------------------------------------------

    pub(super) fn show_intentions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ed) = self.focused_editor(window, cx).filter(|e| e.read(cx).path.as_deref().is_some_and(|p| self.biome_wanted(p))) {
            return self.show_biome_intentions(ed, window, cx);
        }
        let Some(ed) = self.focused_editor(window, cx).filter(|e| e.read(cx).is_python()) else {
            self.status = "No context actions available here".into();
            return;
        };
        let probs = {
            let e = ed.read(cx);
            let mut p = e.problems_at_caret();
            // ruff and ty often flag the same spot (F821 and unresolved-reference): offer both.
            let same: Vec<Diagnostic> =
                e.diagnostics.iter().filter(|d| !p.contains(d) && p.iter().any(|q| d.start <= q.end && q.start <= d.end)).cloned().collect();
            p.extend(same);
            p
        };
        let (front, back, hint) = self.syntax_intentions(&ed, &probs, cx);
        let hint = hint.and_then(|h| self.ty_at_caret(&ed, cx).map(|(ty, path, _)| (h, ty, path)));
        if probs.is_empty() && front.is_empty() && back.is_empty() && hint.is_none() {
            self.status = "No context actions available here".into();
            return;
        }
        // ty's quick fixes answer ty's own diagnostics. Brisk hides ty's unresolved-reference
        // when ruff reports the same name as F821, so ask ty about that one under ty's code.
        let mut ty_diags: Vec<Diagnostic> = probs
            .iter()
            .filter_map(|d| match d.code.as_str() {
                c if brisk_lint::is_ty_code(c) => Some(d.clone()),
                "F821" => Some(Diagnostic { code: "unresolved-reference".into(), fix: None, ..d.clone() }),
                _ => None,
            })
            .collect();
        ty_diags.sort_by_key(|d| (d.start, d.end, d.code.clone()));
        ty_diags.dedup_by(|a, b| (a.start, a.end, &a.code) == (b.start, b.end, &b.code));
        let request = match ty_diags.is_empty() {
            true => None,
            false => self.ty_at_caret(&ed, cx).map(|(ty, path, _)| {
                let start = ty_diags.iter().map(|d| d.start).min().unwrap_or_default();
                let end = ty_diags.iter().map(|d| d.end).max().unwrap_or_default();
                (ty, path, start, end)
            }),
        };
        // The type a hint would write: ty's inlay hint after a variable, or the return type in
        // its hover of a function.
        let hint_fut = hint.map(|(h, ty, path)| {
            use futures::FutureExt;
            let e = ed.read(cx);
            let fut: futures::future::BoxFuture<'static, Option<String>> = match &h {
                brisk_syntax::HintTarget::Variable { insert, .. } => {
                    let p = e.byte_pos(*insert);
                    ty.inlay_hints(&path, p.line..p.line + 1)
                        .map(move |r| r.ok()?.into_iter().find(|i| !i.parameter && i.pos == p).map(|i| i.label.trim_start_matches(':').trim().to_string()))
                        .boxed()
                }
                brisk_syntax::HintTarget::Return { at, returns, .. } => {
                    use brisk_syntax::Returned;
                    // ty's inferred return type, else the union of what the `return`s give.
                    let sig = ty.hover(&path, e.byte_pos(*at));
                    let asks: Vec<_> = returns
                        .iter()
                        .map(|r| match r {
                            Returned::Ask(_, b) => Some(ty.hover(&path, e.byte_pos(*b))),
                            _ => None,
                        })
                        .collect();
                    let returns = returns.clone();
                    async move {
                        if let Some(t) = sig.await.ok().flatten().and_then(|h| return_type(&h)).filter(|t| !t.contains("Unknown")) {
                            return Some(t);
                        }
                        let mut types: Vec<String> = Vec::new();
                        for (r, ask) in returns.iter().zip(asks) {
                            let t = match r {
                                Returned::Known(t) => t.clone(),
                                Returned::Ask(..) => value_type(&ask?.await.ok()??)?,
                                Returned::Unknown => return None,
                            };
                            if !types.contains(&t) {
                                types.push(t);
                            }
                        }
                        // `X | None`, as PyCharm writes optional types.
                        types.sort_by_key(|t| t == "None");
                        Some(types.join(" | "))
                    }
                    .boxed()
                }
            };
            (h, fut)
        });
        if request.is_none() && hint_fut.is_none() {
            return self.open_intentions(&probs, Vec::new(), front, back);
        }
        let unresolved = ty_diags.iter().any(|d| d.code == "unresolved-reference");
        if unresolved {
            self.status = "Looking for imports…".into();
            cx.notify();
        }
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            use futures::FutureExt;
            let mut back = back;
            if let Some((h, fut)) = hint_fut {
                let timeout = bg.timer(Duration::from_secs(3));
                if let futures::future::Either::Left((Some(t), _)) = futures::future::select(fut, timeout.boxed()).await {
                    if !t.is_empty() && !t.contains("Unknown") && !t.contains("@Todo") {
                        let (label, edit) = match h {
                            brisk_syntax::HintTarget::Variable { name, insert, .. } => {
                                (format!("Add type hint for variable '{name}'"), (insert..insert, format!(": {t}")))
                            }
                            brisk_syntax::HintTarget::Return { insert, .. } => {
                                ("Specify return type using annotation".to_string(), (insert..insert, format!(" -> {t}")))
                            }
                        };
                        back.insert(0, PopupItem { label, detail: t, hint: String::new(), target: Target::Intention(Box::new(Intent::Edits(vec![edit]))) });
                    }
                }
            }
            let Some((ty, path, start, end)) = request else {
                let _ = this.update(cx, |this, cx| {
                    this.open_intentions(&probs, Vec::new(), front, back);
                    cx.notify();
                });
                return;
            };
            let mut actions: Vec<brisk_lsp::CodeAction> = Vec::new();
            let mut retry_until: Option<std::time::Instant> = None;
            loop {
                // Right after startup ty may still be indexing the environment and answer an
                // unknown name with nothing but "Ignore": keep asking for a few seconds, so
                // the list does not open with only Suppress while the import is on its way.
                if let Some(until) = retry_until {
                    let found = actions.iter().any(|a| !a.title.starts_with("Ignore '"));
                    if !unresolved || found || std::time::Instant::now() >= until {
                        break;
                    }
                    bg.timer(Duration::from_millis(400)).await;
                }
                retry_until.get_or_insert_with(|| std::time::Instant::now() + Duration::from_secs(3));
                // Usually milliseconds, but the first import search after startup walks the whole
                // environment (seconds with big site-packages); never hang on a stuck server.
                let fut = ty.code_actions(&path, start, end, &ty_diags);
                let timeout = bg.timer(Duration::from_secs(5));
                actions = match futures::future::select(fut.boxed(), timeout.boxed()).await {
                    futures::future::Either::Left((Ok(a), _)) => a,
                    _ => Vec::new(),
                };
            }
            let _ = this.update(cx, |this, cx| {
                this.open_intentions(&probs, actions, front, back);
                cx.notify();
            });
        })
        .detach();
    }

    /// Alt+Enter items from the syntax tree and the project index: fixes for an unknown name
    /// (import from the index, Create function) and intentions; plus where a type hint can go.
    fn syntax_intentions(
        &mut self,
        ed: &Entity<Editor>,
        probs: &[Diagnostic],
        cx: &mut Context<Self>,
    ) -> (Vec<PopupItem>, Vec<PopupItem>, Option<brisk_syntax::HintTarget>) {
        let item = |label: String, detail: String, edits: Vec<(std::ops::Range<usize>, String)>| PopupItem {
            label,
            detail,
            hint: String::new(),
            target: Target::Intention(Box::new(Intent::Edits(edits))),
        };
        let (intents, create, hint) = ed.update(cx, |e, _| e.intentions_at_caret());
        let unknown: Option<String> = probs.iter().find(|d| d.code == "F821" || d.code == "unresolved-reference").map(|d| {
            let e = ed.read(cx);
            let line = e.buffer.line(d.start.line);
            line.chars().skip(d.start.col).take(d.end.col.saturating_sub(d.start.col)).collect()
        });
        let mut front = Vec::new();
        if let Some(name) = unknown.filter(|n| !n.is_empty()) {
            // Imports of the name from the project's own modules (ty may not offer them yet).
            self.ensure_symbols(cx);
            if let (Some(list), Some(files)) = (self.symbols.list.clone(), self.symbols.files.clone()) {
                let this_file = ed.read(cx).path.clone();
                let mut mods: Vec<String> = list
                    .iter()
                    .filter(|s| *s.name == name && s.container.is_empty())
                    .filter(|s| this_file.as_deref() != Some(files.abs(s.file).as_path()))
                    .filter_map(|s| brisk_lsp::module_name(&self.root, &files.abs(s.file)))
                    .collect();
                mods.sort();
                mods.dedup();
                for m in mods.into_iter().take(5) {
                    if let Some(edit) = ed.update(cx, |e, _| e.import_edit(&format!("from {m} import {name}"))) {
                        front.push(item(format!("Import '{m}.{name}'"), "project".into(), vec![edit]));
                    }
                }
            }
            if let Some((n, edit)) = create.filter(|(n, _)| *n == name) {
                front.push(item(format!("Create function '{n}'"), String::new(), vec![edit]));
            }
        }
        let back = intents.into_iter().map(|i| item(i.label, String::new(), i.edits)).collect();
        (front, back, hint)
    }

    /// The Alt+Enter list as in PyCharm: the quick fixes for the problem (ty's, imports from
    /// the project index and Create function in `front`), ruff's, then the intentions (`back`),
    /// then suppressions.
    fn open_intentions(&mut self, probs: &[Diagnostic], mut actions: Vec<brisk_lsp::CodeAction>, front: Vec<PopupItem>, back: Vec<PopupItem>) {
        let mut items = Vec::new();
        let item =
            |label: String, detail: String, intent: Intent| PopupItem { label, detail, hint: String::new(), target: Target::Intention(Box::new(intent)) };
        // Brisk adds its own "Suppress for line"; ty's "Ignore ... for this line" is the same.
        actions.retain(|a| !a.title.starts_with("Ignore '"));
        // ty also finds the stub copies other tools vendor (`mypy.typeshed.stdlib.collections`):
        // not importable at run time.
        actions.retain(|a| !(a.title.starts_with("import ") && a.title.contains(".typeshed.")));
        actions.sort_by_key(|a| !a.preferred);
        for a in actions {
            items.push(item(lsp_title(&a.title), "ty".into(), Intent::Lsp(a.title.clone(), a.edits)));
        }
        for f in front {
            if !items.iter().any(|i| i.label == f.label) {
                items.push(f);
            }
        }
        for d in probs {
            if let Some(f) = &d.fix {
                items.push(item(
                    format!("Ruff: {}", f.message),
                    if f.safe { d.code.clone() } else { format!("{} · unsafe fix", d.code) },
                    Intent::Fix(d.clone()),
                ));
            }
        }
        let mut codes: Vec<&str> = probs.iter().filter(|d| d.fix.is_some()).map(|d| d.code.as_str()).collect();
        codes.dedup();
        for c in codes {
            items.push(item(format!("Ruff: fix all '{c}' problems in file"), String::new(), Intent::FixAllOfCode(c.to_string())));
        }
        if probs.iter().any(|d| d.fix.is_some()) {
            items.push(item("Ruff: fix all auto-fixable problems in file".into(), String::new(), Intent::FixAll));
        }
        items.extend(back);
        for d in probs.iter().filter(|d| d.noqa_line.is_some()) {
            if items.iter().any(|i| i.label == format!("Suppress '{}' for line", d.code)) {
                continue;
            }
            items.push(item(
                format!("Suppress '{}' for line", d.code),
                if brisk_lint::is_ty_code(&d.code) { "# ty: ignore".into() } else { "# noqa".into() },
                Intent::Suppress(d.clone()),
            ));
        }
        if items.is_empty() {
            self.status = "No context actions available here".into();
            return;
        }
        self.problems.intents = items;
        self.problems.intents_title = None;
        self.open_popup(PopupKind::Intentions);
    }

    pub(super) fn intention_items(&self, q: &str) -> Vec<PopupItem> {
        let ql = q.to_lowercase();
        self.problems.intents.iter().filter(|i| i.label.to_lowercase().contains(&ql)).cloned().collect()
    }

    pub(super) fn apply_intent(&mut self, intent: Intent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_editor(window, cx);
        let Some(ed) = self.active_editor().cloned() else { return };
        match intent {
            Intent::Fix(d) => {
                if let Some(f) = &d.fix {
                    ed.update(cx, |e, cx| e.apply_fix(f, cx));
                }
            }
            Intent::Suppress(d) => ed.update(cx, |e, cx| e.suppress(&d, cx)),
            Intent::Template(key) => ed.update(cx, |e, cx| e.insert_template(key, cx)),
            Intent::Surround(i) => ed.update(cx, |e, cx| e.surround_with(i, cx)),
            Intent::Unwrap(u) => ed.update(cx, |e, cx| e.apply_unwrap(&u, cx)),
            Intent::Member(code) => ed.update(cx, |e, cx| e.insert_member(&code, cx)),
            Intent::Overrides(implement) => self.override_methods(implement, window, cx),
            Intent::ExtractVariable(all) => {
                ed.update(cx, |e, cx| e.extract_variable(all, cx));
            }
            Intent::ExtractConstant(all) => {
                ed.update(cx, |e, cx| e.extract_constant(all, cx));
            }
            Intent::PickExpression(r, constant) => {
                ed.update(cx, |e, _| e.buffer.selections = vec![brisk_text::Selection { anchor: r.start, head: r.end, goal_col: None }]);
                let a = if constant { brisk_keymap::Action::ExtractConstant } else { brisk_keymap::Action::ExtractVariable };
                self.extract_action(a, window, cx);
            }
            Intent::Edits(edits) => ed.update(cx, |e, cx| e.apply_byte_edits(edits, cx)),
            Intent::RenamePreview(q) => self.rename_with(q, true, window, cx),
            Intent::RenameText(q) => self.toggle_rename_text(q),
            Intent::Lsp(title, edits) => {
                let (_, failed) = self.apply_file_edits(edits, cx);
                self.status = lsp_title(&title);
                if !failed.is_empty() {
                    self.show_toast(format!("Could not write: {}", failed.join("; ")), true, cx);
                }
            }
            Intent::BiomeFixAll => self.biome_fix_all(ed, false, cx),
            Intent::FixAll | Intent::FixAllOfCode(_) => {
                let only = match &intent {
                    Intent::FixAllOfCode(c) => Some(c.clone()),
                    _ => None,
                };
                let (Some(ruff), Some(path)) = (self.ruff(cx), ed.read(cx).path.clone()) else { return };
                let text = ed.read(cx).text_string();
                match brisk_lint::fix_all(&ruff, &self.root, &path, &text, only.as_deref()) {
                    Ok(fixed) if !fixed.is_empty() => ed.update(cx, |e, cx| e.replace_all_text(&fixed, cx)),
                    Ok(_) => {}
                    Err(e) => self.show_toast(format!("ruff: {e}"), true, cx),
                }
            }
        }
    }

    pub(super) fn goto_problem(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.focused_editor(window, cx).or_else(|| self.active_editor().cloned()) else { return };
        self.status = ed.update(cx, |e, cx| e.goto_problem(forward, cx)).unwrap_or_else(|| "No problems in this file".into());
    }

    /// `BRISK_SMOKE_LINT=<file.py>`: open, wait for ruff, print the problems, apply the first
    /// quick fix, wait for the re-check, open the Problems window on the project.
    #[cfg(feature = "smoke")]
    pub(super) fn lint_smoke(&mut self, file: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let t0 = std::time::Instant::now();
        self.open_file(&file, window, cx);
        let bg = cx.background_executor().clone();
        let wait_checked = |this: &gpui::WeakEntity<Self>, cx: &mut gpui::AsyncWindowContext| {
            this.update(cx, |this, cx| {
                this.active_editor().is_some_and(|e| e.read(cx).diag_version == e.read(cx).buffer.version()) || this.problems.error.is_some()
            })
            .unwrap_or(true)
        };
        cx.spawn_in(window, async move |this, cx| {
            for _ in 0..100 {
                bg.timer(Duration::from_millis(50)).await;
                if wait_checked(&this, cx) {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(ed) = this.active_editor().cloned() else { return };
                eprintln!("smoke-lint: checked in {:.0?}, error {:?}", t0.elapsed(), this.problems.error);
                for d in &ed.read(cx).diagnostics {
                    eprintln!("smoke-lint: {:?} {}:{} {}", d.severity, d.start.line + 1, d.start.col + 1, d.title());
                }
                let r = ed.update(cx, |e, cx| e.goto_problem(true, cx));
                eprintln!("smoke-lint: F2 -> {r:?} at {:?}", ed.read(cx).caret_line_col());
                this.show_intentions(window, cx);
                eprintln!("smoke-lint: Alt+Enter there -> {:?}", this.problems.intents.iter().map(|i| i.label.clone()).collect::<Vec<_>>());
                // Then a fixable one: caret on the first problem that has a ruff fix.
                if let Some(d) = ed.read(cx).diagnostics.iter().find(|d| d.fix.is_some()).cloned() {
                    ed.update(cx, |e, cx| e.select_on_line(d.start.line, d.start.col..d.start.col, cx));
                }
                this.show_intentions(window, cx);
                let items: Vec<String> = this.problems.intents.iter().map(|i| i.label.clone()).collect();
                eprintln!("smoke-lint: Alt+Enter -> {items:?}");
                if let Some(Target::Intention(i)) = this.problems.intents.first().map(|i| i.target.clone()) {
                    this.popup = None;
                    this.apply_intent(*i, window, cx);
                }
            });
            for _ in 0..100 {
                bg.timer(Duration::from_millis(50)).await;
                if wait_checked(&this, cx) {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(ed) = this.active_editor().cloned() {
                    eprintln!("smoke-lint: after fix {} problem(s), text starts {:?}", ed.read(cx).diagnostics.len(), ed.read(cx).text_string().lines().next());
                }
                this.problems.tab = ProblemsTab::Project;
                this.toggle_problems(window, cx);
            });
            for _ in 0..100 {
                bg.timer(Duration::from_millis(50)).await;
                if this.update(cx, |this, _| this.problems.project.is_some()).unwrap_or(true) {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                match &this.problems.project {
                    Some(Ok(v)) => {
                        let files: std::collections::BTreeSet<_> = v.iter().map(|d| d.file.clone()).collect();
                        eprintln!("smoke-lint: project {} problem(s) in {} file(s) after {:.1?}", v.len(), files.len(), t0.elapsed());
                    }
                    other => eprintln!("smoke-lint: project {:?}", other.as_ref().map(|r| r.as_ref().err())),
                }
                // The first file's header folds with Enter and unfolds with Right.
                window.focus(&this.problems.focus.clone());
                this.problems.selected = 0;
                let before = this.rows(cx).len();
                this.smoke_press("enter", window, cx);
                let folded = this.rows(cx).len();
                this.smoke_press("right", window, cx);
                eprintln!("smoke-lint: project rows {before}, folded {folded}, unfolded {}", this.rows(cx).len());
                cx.notify();
            });
        })
        .detach();
    }
}

impl Workspace {
    /// Settings › Tools › Actions on Save, as in PyCharm: Optimize imports, then Reformat code,
    /// on the modified Python files being saved (Ctrl+S and the autosave on leaving the
    /// window). ruff runs in the background: the file is saved as typed now, and the cleaned
    /// text is applied and saved when ruff answers, unless the file was edited meanwhile. A
    /// file ruff cannot parse stays as it is.
    pub(super) fn actions_on_save(&mut self, cx: &mut Context<Self>) {
        let s = brisk_settings::current();
        if !s.reformat_on_save && !s.optimize_imports_on_save {
            return;
        }
        let root = self.root.clone();
        let eds: Vec<_> = self
            .editors()
            .filter(|e| {
                let e = e.read(cx);
                let other =
                    || s.reformat_on_save && e.path.as_deref().and_then(|p| reformatter(p, &root)).is_some_and(|r| !matches!(r, Reformatter::TypeScript));
                e.buffer.is_dirty() && (e.is_python() || other()) && e.path.is_some() && e.read_error.is_none() && e.disk_conflict.is_none()
            })
            .cloned()
            .collect();
        if eds.is_empty() {
            return;
        }
        let ruff = self.ruff(cx);
        if ruff.is_none() && eds.iter().any(|e| e.read(cx).is_python()) {
            // Before interpreter discovery ends, ruff may still turn up next to a Python.
            if self.env.read(cx).all.is_some() && !self.problems.save_actions_warned {
                self.problems.save_actions_warned = true;
                self.show_toast("Actions on Save (Reformat code, Optimize imports) need ruff, which was not found. Install it into the project interpreter: pip install ruff".into(), true, cx);
            }
        }
        for ed in eds {
            let (Some(path), text, version, python, unit) = ({
                let e = ed.read(cx);
                (e.path.clone(), e.text_string(), e.buffer.version(), e.is_python(), e.buffer.indent_unit.clone())
            }) else {
                continue;
            };
            let root = self.root.clone();
            let (imports, reformat) = (s.optimize_imports_on_save, s.reformat_on_save);
            // Python goes to ruff; other files to Prettier or Brisk's formatter.
            let how = if python { None } else { reformatter(&path, &root) };
            let ruff = match (python, &ruff) {
                (true, Some(r)) => Some(r.clone()),
                (true, None) => continue,
                _ => None,
            };
            let task = cx.background_executor().spawn(async move {
                let keep = |r: Result<String, String>, cur: &str| r.ok().filter(|t| !t.is_empty() || cur.is_empty());
                let mut out = text.clone();
                if let Some(ruff) = &ruff {
                    if imports {
                        out = keep(brisk_lint::optimize_imports(ruff, &root, &path, &out), &out).unwrap_or(out);
                    }
                    if reformat {
                        out = keep(brisk_lint::format_text(ruff, &root, &path, &out, None), &out).unwrap_or(out);
                    }
                } else if let Some(r) = how.as_ref().and_then(|h| reformat_offline(h, &root, &path, &out, &unit)) {
                    out = keep(r, &out).unwrap_or(out);
                }
                (out.replace("\r\n", "\n") != text.replace("\r\n", "\n")).then_some(out)
            });
            cx.spawn(async move |_, cx| {
                let Some(out) = task.await else { return };
                let _ = ed.update(cx, |e, cx| {
                    // Typed into since: ruff's text is outdated; the next save cleans it up.
                    if e.buffer.version() != version {
                        return;
                    }
                    e.replace_all_text(&out, cx);
                    let _ = e.save();
                });
            })
            .detach();
        }
    }

    /// Ctrl+Alt+L (the selected lines, else the file), Ctrl+Alt+O and Code Cleanup (every safe
    /// ruff fix in the file, PyCharm's "fix all problems"), with ruff.
    /// Reformat Code / Optimize Imports with a folder or file selected in the Project tree, as
    /// in PyCharm: every Python file under it, open ones in their editors (one undo step each),
    /// the rest on disk; ruff runs off the UI thread. False when the tree is not focused.
    fn format_tree_action(&mut self, action: brisk_keymap::Action, window: &mut Window, cx: &mut Context<Self>) -> bool {
        use brisk_keymap::Action as A;
        if !matches!(action, A::ReformatCode | A::OptimizeImports) || !self.tree_focus.is_focused(window) {
            return false;
        }
        let Some((_, path, _)) = self.selected_path() else { return false };
        let reformat = action == A::ReformatCode;
        let title = if reformat { "Reformat Code" } else { "Optimize Imports" };
        let files: Vec<PathBuf> = if path.is_dir() {
            self.project_python_files(&path)
        } else if matches!(path.extension().and_then(|e| e.to_str()), Some("py" | "pyi")) {
            vec![path.clone()]
        } else {
            Vec::new()
        };
        if files.is_empty() {
            self.status = format!("{title}: no Python files in {}", path.display());
            cx.notify();
            return true;
        }
        let Some(ruff) = self.ruff(cx) else {
            self.show_toast("ruff not found: install it in the project interpreter (pip install ruff)".into(), true, cx);
            return true;
        };
        let open: HashMap<PathBuf, String> = self.editors().filter_map(|e| Some((e.read(cx).path.clone()?, e.read(cx).text_string()))).collect();
        let inputs: Vec<(PathBuf, String, bool)> = files
            .into_iter()
            .filter_map(|f| match open.get(&f) {
                Some(t) => Some((f, t.clone(), true)),
                None => std::fs::read_to_string(&f).ok().map(|t| (f, t, false)),
            })
            .collect();
        let total = inputs.len();
        let root = self.root.clone();
        let task = cx.background_executor().spawn(async move {
            inputs
                .into_iter()
                .filter_map(|(f, text, open)| {
                    let out =
                        if reformat { brisk_lint::format_text(&ruff, &root, &f, &text, None) } else { brisk_lint::optimize_imports(&ruff, &root, &f, &text) }
                            .ok()?;
                    (!out.is_empty() && out != text).then_some((f, text, out, open))
                })
                .collect::<Vec<_>>()
        });
        self.status = format!("{title}: {total} file(s)…");
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let changed = task.await;
            let _ = this.update(cx, |this, cx| {
                let mut on_disk = Vec::new();
                let mut n = 0;
                for (f, before, out, open) in changed {
                    let ed = this.editors().find(|e| e.read(cx).path.as_deref() == Some(f.as_path())).cloned();
                    // Skip a file that changed meanwhile.
                    let now = match &ed {
                        Some(e) if open => e.read(cx).text_string(),
                        None if !open => std::fs::read_to_string(&f).unwrap_or_default(),
                        _ => continue,
                    };
                    if now != before {
                        continue;
                    }
                    brisk_history::record(&f, &before.replace("\r\n", "\n"), brisk_history::Label::BeforeReformat);
                    match ed {
                        Some(e) => e.update(cx, |e, cx| e.replace_all_text(&out, cx)),
                        None if std::fs::write(&f, &out).is_ok() => on_disk.push(f),
                        None => continue,
                    }
                    n += 1;
                }
                if let (Some(ty), false) = (&this.lsp.ty, on_disk.is_empty()) {
                    ty.files_changed(&on_disk);
                }
                let msg = match (reformat, n) {
                    (_, 0) if reformat => format!("No changes: {total} file(s) already formatted"),
                    (_, 0) => format!("Imports are already optimized in {total} file(s)"),
                    (true, _) => format!("Reformatted {n} of {total} file(s) (ruff)"),
                    (false, _) => format!("Optimized imports in {n} of {total} file(s)"),
                };
                this.status = msg.clone();
                this.show_toast(msg, false, cx);
                this.refresh_git(cx);
                cx.notify();
            });
        })
        .detach();
        true
    }

    pub(super) fn format_action(&mut self, action: brisk_keymap::Action, window: &mut Window, cx: &mut Context<Self>) {
        if self.format_tree_action(action, window, cx) {
            return;
        }
        let Some(ed) = self.focused_editor(window, cx).or_else(|| self.active_editor().cloned()) else { return };
        let (path, python) = {
            let e = ed.read(cx);
            (e.path.clone(), e.is_python())
        };
        if let (Some(path), false, brisk_keymap::Action::ReformatCode) = (&path, python, action) {
            self.reformat_other(ed, path.clone(), cx);
            return;
        }
        // Optimize Imports and Code Cleanup in a Biome file: Biome's import sorting, its safe fixes.
        if path.as_deref().is_some_and(|p| !python && self.biome_wanted(p)) {
            self.biome_fix_all(ed, action == brisk_keymap::Action::OptimizeImports, cx);
            return;
        }
        let Some(path) = path.filter(|_| python) else {
            self.status = "Optimize Imports and Code Cleanup work on Python files (ruff)".into();
            cx.notify();
            return;
        };
        let Some(ruff) = self.ruff(cx) else {
            self.show_toast("ruff not found: install it in the project interpreter (pip install ruff)".into(), true, cx);
            return;
        };
        let (text, lines) = {
            let e = ed.read(cx);
            let sel = e.buffer.primary();
            let lines = (!sel.is_empty() && e.buffer.selections.len() == 1).then(|| {
                let r = sel.range();
                let (a, _) = e.buffer.pos_to_line_col(r.start);
                let (z, zc) = e.buffer.pos_to_line_col(r.end);
                (a, if zc == 0 && z > a { z - 1 } else { z })
            });
            (e.text_string(), lines)
        };
        let reformat = action == brisk_keymap::Action::ReformatCode;
        let cleanup = action == brisk_keymap::Action::CodeCleanup;
        let result = if reformat {
            brisk_lint::format_text(&ruff, &self.root, &path, &text, lines)
        } else if cleanup {
            brisk_lint::fix_all(&ruff, &self.root, &path, &text, None)
        } else {
            brisk_lint::optimize_imports(&ruff, &self.root, &path, &text)
        };
        let what = if reformat {
            if lines.is_some() {
                "Reformatted the selected lines"
            } else {
                "Reformatted the file"
            }
        } else {
            "Optimized imports"
        };
        match result {
            Ok(out) if out.is_empty() && !text.is_empty() => self.status = "ruff returned nothing; the file was not changed".into(),
            Ok(out) => {
                let changed = out.replace("\r\n", "\n") != text.replace("\r\n", "\n");
                if changed {
                    brisk_history::record(&path, &text.replace("\r\n", "\n"), brisk_history::Label::BeforeReformat);
                    ed.update(cx, |e, cx| e.replace_all_text(&out, cx));
                }
                if cleanup {
                    self.status = if changed { "Code Cleanup: applied ruff's fixes".into() } else { "Code Cleanup: nothing to fix".into() };
                } else if reformat {
                    self.status = if changed { format!("{what} (ruff)") } else { "No changes: the code is already formatted".into() };
                } else {
                    // PyCharm reports what Optimize Imports did in a balloon.
                    let msg = brisk_lint::imports_summary(&text, &out);
                    self.status = msg.clone();
                    self.show_toast(msg, false, cx);
                }
            }
            Err(e) => self.show_toast(format!("ruff: {e}"), true, cx),
        }
        cx.notify();
    }
}

/// How a file other than Python is reformatted, in PyCharm's order: Biome when the project
/// uses it, Prettier when the project has it, Biome when Settings turns it on everywhere,
/// then TypeScript's server for JS / TS, then Brisk's own formatter for the type.
enum Reformatter {
    Biome(PathBuf),
    Prettier(PathBuf, PathBuf),
    TypeScript,
    Builtin(brisk_format::Kind),
}

fn reformatter(path: &Path, root: &Path) -> Option<Reformatter> {
    use brisk_lsp::biome;
    let with_biome = || biome::supports(path).then(|| biome::find(root)).flatten().map(Reformatter::Biome);
    if biome::in_use(root) {
        if let Some(b) = with_biome() {
            return Some(b);
        }
    }
    if brisk_lint::prettier::supports(path) {
        if let (Some(script), Some(node)) = (brisk_lint::prettier::find(path, root), brisk_run::node::on_path("node")) {
            return Some(Reformatter::Prettier(node, script));
        }
    }
    if brisk_settings::biome_everywhere() {
        if let Some(b) = with_biome() {
            return Some(b);
        }
    }
    if super::ts_ui::is_ts_path(path) {
        return Some(Reformatter::TypeScript);
    }
    brisk_format::Kind::of(path).map(Reformatter::Builtin)
}

/// The text reformatted off the UI thread, without the TypeScript server (Actions on Save).
fn reformat_offline(how: &Reformatter, root: &Path, path: &Path, text: &str, unit: &str) -> Option<Result<String, String>> {
    Some(match how {
        Reformatter::Biome(exe) => brisk_lsp::biome::format(exe, root, path, text),
        Reformatter::Prettier(node, script) => brisk_lint::prettier::format(node, script, root, path, text),
        Reformatter::Builtin(kind) => brisk_format::format(
            *kind,
            &text.replace(
                "

", "\r\n",
            ),
            unit,
        ),
        Reformatter::TypeScript => return None,
    })
}

impl Workspace {
    /// Ctrl+Alt+L on a file that is not Python: the whole file, by `reformatter`. A text the
    /// formatter cannot parse stays as it is, with the reason in the status bar.
    fn reformat_other(&mut self, ed: Entity<Editor>, path: PathBuf, cx: &mut Context<Self>) {
        let Some(how) = reformatter(&path, &self.root) else {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("these");
            self.status = format!("Reformat Code: no formatter for .{ext} files");
            cx.notify();
            return;
        };
        let (text, version, unit) = {
            let e = ed.read(cx);
            (e.text_string(), e.buffer.version(), e.buffer.indent_unit.clone())
        };
        let by = match &how {
            Reformatter::Biome(_) => "Biome",
            Reformatter::Prettier(..) => "Prettier",
            Reformatter::TypeScript => "TypeScript",
            Reformatter::Builtin(_) => "",
        };
        let task: gpui::Task<Result<String, String>> = match how {
            Reformatter::TypeScript => {
                self.ensure_ts(cx);
                let ready = self.lsp.ts.state == super::lsp_ui::TyState::Ready;
                let Some(server) = ready.then(|| self.ty_sync(&path, version, &text)).flatten() else {
                    self.status =
                        "Reformat Code for JavaScript and TypeScript needs Prettier in the project or TypeScript's language server, which is not running yet"
                            .into();
                    cx.notify();
                    return;
                };
                let tab = unit.chars().count().max(1);
                let request = server.formatting(&path, if unit == "\t" { 4 } else { tab }, unit != "\t");
                let before = text.replace(
                    "

", "\r\n",
                );
                cx.background_executor().spawn(async move { request.await.map(|edits| brisk_lint::apply_edits(&before, &edits)) })
            }
            how => {
                let (root, p, t) = (self.root.clone(), path.clone(), text.clone());
                cx.background_executor().spawn(async move { reformat_offline(&how, &root, &p, &t, &unit).unwrap_or(Ok(t)) })
            }
        };
        self.status = "Reformatting…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let by = if by.is_empty() { String::new() } else { format!(" ({by})") };
                this.status = match result {
                    // Typed into meanwhile: the result is for an older text.
                    _ if ed.read(cx).buffer.version() != version => "Reformat Code: the file changed meanwhile; run it again".into(),
                    Ok(out) if out.is_empty() && !text.trim().is_empty() => format!("Reformat Code{by}: nothing came back; the file was not changed"),
                    Ok(out) => {
                        let before = text.replace(
                            "

", "\r\n",
                        );
                        if out.replace(
                            "

", "\r\n",
                        ) == before
                        {
                            "No changes: the code is already formatted".into()
                        } else {
                            brisk_history::record(&path, &before, brisk_history::Label::BeforeReformat);
                            ed.update(cx, |e, cx| e.replace_all_text(&out, cx));
                            format!("Reformatted the file{by}")
                        }
                    }
                    Err(e) => format!("Reformat Code{by}: {e}; the file was not changed"),
                };
                cx.notify();
            });
        })
        .detach();
    }
}
