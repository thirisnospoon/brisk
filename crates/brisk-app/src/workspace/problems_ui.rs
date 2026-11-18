//! Inspections: ruff checks of open Python files as you type, the Problems tool window
//! (Alt+6, "File" and "Project" tabs), F2 / Shift+F2 and Alt+Enter quick fixes.

use super::{BottomTool, PopupItem, PopupKind, Target, Workspace};
use crate::editor::{DiagSource, Editor, EditorEvent};
use crate::theme::{Theme, mono_font};
use brisk_lint::{Diagnostic, Severity};
use gpui::prelude::*;
use gpui::{div, px, uniform_list, ClickEvent, Context, Entity, EntityId, FocusHandle, Focusable, UniformListScrollHandle, Window};
use std::collections::HashMap;
use std::path::PathBuf;
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
    /// Ctrl+Alt+V: extract the selection into a variable; `true`: every occurrence.
    ExtractVariable(bool),
    /// A language server quick fix (ty: import a name, qualify it): title and edits.
    Lsp(String, Vec<brisk_lsp::FileEdits>),
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

pub(super) struct Problems {
    pub tab: ProblemsTab,
    pub focus: FocusHandle,
    pub selected: usize,
    pub scroll: UniformListScrollHandle,
    /// Last whole-project check; `None` until the Project tab is first opened.
    pub project: Option<Result<Vec<Diagnostic>, String>>,
    pub project_running: bool,
    /// Why ruff could not run (not installed, bad config).
    pub error: Option<String>,
    lint_gen: HashMap<EntityId, u64>,
    pub(super) intents: Vec<PopupItem>,
    /// Popup title when the list is not Alt+Enter's (Surround With, Insert Live Template).
    pub(super) intents_title: Option<&'static str>,
}

impl Problems {
    pub fn new(cx: &mut gpui::App) -> Self {
        Problems {
            tab: ProblemsTab::File,
            focus: cx.focus_handle(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            project: None,
            project_running: false,
            error: None,
            lint_gen: HashMap::new(),
            intents: Vec::new(),
            intents_title: None,
        }
    }
}

/// A row of the Problems list.
enum Row {
    File(PathBuf, usize),
    Problem(Diagnostic),
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
    fn ruff(&self) -> Option<PathBuf> {
        let project = self.runs.interpreter(&self.root).map(|i| i.path);
        let others = self.runs.venvs.iter().chain(self.runs.all.iter().flatten()).map(|i| i.path.clone());
        project.into_iter().chain(others).find_map(|p| brisk_lint::find_ruff(Some(&p))).or_else(|| brisk_lint::find_ruff(None))
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
            if ed.read(cx).lint_sent != v && ed.read(cx).is_python() {
                this.schedule_lint(ed, LINT_DELAY, cx);
            }
        })
        .detach();
        cx.subscribe_in(ed, window, |this: &mut Self, ed, ev: &EditorEvent, window, cx| match ev {
            EditorEvent::ShowProblems => {
                this.problems.tab = ProblemsTab::File;
                this.bottom = Some(BottomTool::Problems);
                window.focus(&this.problems.focus.clone());
                cx.notify();
            }
            EditorEvent::ContextMenu(at) => this.open_editor_menu(ed.clone(), *at, cx),
            EditorEvent::BreakpointsChanged => this.breakpoints_changed(ed.clone(), cx),
            EditorEvent::EditBreakpoint(line) => this.edit_breakpoint(ed.clone(), *line, cx),
            EditorEvent::SetNextStatement(line) => this.set_next_statement(ed.clone(), *line, cx),
            EditorEvent::GutterMenu(at) => this.open_gutter_menu(ed.clone(), *at, cx),
            EditorEvent::RunMark(line, at) => this.run_mark_menu(ed.clone(), *line, *at, cx),
            EditorEvent::ShowCommit(hash) => this.show_annotation_commit(ed.clone(), hash.clone(), window, cx),
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
        let ruff = self.ruff();
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
                this.ty_check(ed.clone(), path.clone(), v, &t, cx);
                Some((v, t, path))
            }) else {
                return;
            };
            let Some(ruff) = ruff else {
                let _ = this.update(cx, |this, cx| {
                    // Still looking for interpreters: discovery re-checks when it is done.
                    if this.runs.all.is_none() {
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
        let Some(ruff) = self.ruff() else {
            if self.runs.all.is_none() {
                self.problems.project = None;
                return;
            }
            self.problems.project = Some(Err("ruff not found: install it in the project interpreter (pip install ruff)".into()));
            return;
        };
        self.problems.project_running = true;
        let root = self.root.clone();
        let task = cx.background_executor().spawn(async move { brisk_lint::check_project(&ruff, &root) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.problems.project_running = false;
                this.problems.project = Some(res);
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

    fn file_rows(&self, cx: &gpui::App) -> Vec<Row> {
        self.active_editor().map(|e| e.read(cx).diagnostics.iter().cloned().map(Row::Problem).collect()).unwrap_or_default()
    }

    fn rows(&self, cx: &gpui::App) -> Vec<Row> {
        match self.problems.tab {
            ProblemsTab::File => self.file_rows(cx),
            ProblemsTab::Project => {
                let Some(Ok(all)) = &self.problems.project else { return Vec::new() };
                let mut rows = Vec::new();
                let mut i = 0;
                while i < all.len() {
                    let f = all[i].file.clone();
                    let n = all[i..].iter().take_while(|d| d.file == f).count();
                    rows.push(Row::File(f, n));
                    rows.extend(all[i..i + n].iter().cloned().map(Row::Problem));
                    i += n;
                }
                rows
            }
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
                if let Some(Row::Problem(d)) = rows.get(self.problems.selected) {
                    let d = d.clone();
                    self.open_problem(&d, window, cx);
                }
                cx.notify();
                return true;
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
        let file_count = self.file_rows(cx).len();
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
        let header = div()
            .h(px(34.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(8.))
            .text_size(px(13.))
            .child(div().pr(px(8.)).font_weight(gpui::FontWeight::SEMIBOLD).child("Problems"))
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
                        .child(if self.problems.project_running { "Checking…" } else { "↻" })
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
            Some(msg) => div().flex_1().flex().items_center().justify_center().text_color(Theme::text_muted()).text_size(px(13.)).child(msg).into_any_element(),
            None => uniform_list(
                "problems",
                n,
                cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                    let rows = this.rows(cx);
                    let focused = this.problems.focus.is_focused(window);
                    let root = this.root.clone();
                    range
                        .filter_map(|i| rows.get(i).map(|r| (i, r)))
                        .map(|(i, r)| {
                            let selected = this.problems.selected == i;
                            let base = div()
                                .id(("problem", i))
                                .h(px(22.))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .text_size(px(13.))
                                .w_full()
                                .when(selected, |d| d.bg(if focused { Theme::selection_row() } else { Theme::hover_row() }))
                                .when(!selected, |d| d.hover(|s| s.bg(Theme::hover_row())))
                                .cursor_pointer();
                            match r {
                                Row::File(f, count) => {
                                    let rel = brisk_run::relativize(&root, f);
                                    let (name, dir) = match rel.rsplit_once('/') {
                                        Some((d, n)) => (n.to_string(), d.to_string()),
                                        None => (rel.clone(), String::new()),
                                    };
                                    base.pl(px(10.))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.problems.selected = i;
                                            window.focus(&this.problems.focus.clone());
                                            cx.notify();
                                        }))
                                        .child(div().text_color(Theme::text_muted()).child("▾"))
                                        .child(name)
                                        .child(div().text_color(Theme::text_dim()).text_size(px(12.)).child(dir))
                                        .child(div().text_color(Theme::text_muted()).text_size(px(12.)).child(format!("{count} problem{}", if *count == 1 { "" } else { "s" })))
                                }
                                Row::Problem(d) => {
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
                                        .child(div().w(px(14.)).text_color(c).child(g))
                                        .child(div().text_color(Theme::text()).overflow_hidden().whitespace_nowrap().child(d.title()))
                                        .child(div().text_color(Theme::text_dim()).font_family(mono_font()).text_size(px(12.)).child(format!(":{}", d.start.line + 1)))
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
        let Some(ed) = self.focused_editor(window, cx).filter(|e| e.read(cx).is_python()) else {
            self.status = "No context actions available here".into();
            return;
        };
        let probs = {
            let e = ed.read(cx);
            let mut p = e.problems_at_caret();
            // ruff and ty often flag the same spot (F821 and unresolved-reference): offer both.
            let same: Vec<Diagnostic> = e.diagnostics.iter().filter(|d| !p.contains(d) && p.iter().any(|q| d.start <= q.end && q.start <= d.end)).cloned().collect();
            p.extend(same);
            p
        };
        if probs.is_empty() {
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
        let Some((ty, path, start, end)) = request else { return self.open_intentions(&probs, Vec::new()) };
        let unresolved = ty_diags.iter().any(|d| d.code == "unresolved-reference");
        if unresolved {
            self.status = "Looking for imports…".into();
            cx.notify();
        }
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            use futures::FutureExt;
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
                this.open_intentions(&probs, actions);
                cx.notify();
            });
        })
        .detach();
    }

    /// The Alt+Enter list: ty's fixes (auto-import first), then ruff's, then suppressions.
    fn open_intentions(&mut self, probs: &[Diagnostic], mut actions: Vec<brisk_lsp::CodeAction>) {
        let mut items = Vec::new();
        let item = |label: String, detail: String, intent: Intent| PopupItem { label, detail, hint: String::new(), target: Target::Intention(Box::new(intent)) };
        // Brisk adds its own "Suppress for line"; ty's "Ignore ... for this line" is the same.
        actions.retain(|a| !a.title.starts_with("Ignore '"));
        actions.sort_by_key(|a| !a.preferred);
        for a in actions {
            items.push(item(lsp_title(&a.title), "ty".into(), Intent::Lsp(a.title.clone(), a.edits)));
        }
        for d in probs {
            if let Some(f) = &d.fix {
                items.push(item(format!("Ruff: {}", f.message), if f.safe { d.code.clone() } else { format!("{} · unsafe fix", d.code) }, Intent::Fix(d.clone())));
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
        for d in probs.iter().filter(|d| d.noqa_line.is_some()) {
            if items.iter().any(|i| i.label == format!("Suppress '{}' for line", d.code)) {
                continue;
            }
            items.push(item(format!("Suppress '{}' for line", d.code), if brisk_lint::is_ty_code(&d.code) { "# ty: ignore".into() } else { "# noqa".into() }, Intent::Suppress(d.clone())));
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
            Intent::ExtractVariable(all) => {
                ed.update(cx, |e, cx| e.extract_variable(all, cx));
            }
            Intent::Lsp(title, edits) => {
                let (_, failed) = self.apply_file_edits(edits, cx);
                self.status = lsp_title(&title);
                if !failed.is_empty() {
                    self.show_toast(format!("Could not write: {}", failed.join("; ")), true, cx);
                }
            }
            Intent::FixAll | Intent::FixAllOfCode(_) => {
                let only = match &intent {
                    Intent::FixAllOfCode(c) => Some(c.clone()),
                    _ => None,
                };
                let (Some(ruff), Some(path)) = (self.ruff(), ed.read(cx).path.clone()) else { return };
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
    pub(super) fn lint_smoke(&mut self, file: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let t0 = std::time::Instant::now();
        self.open_file(&file, window, cx);
        let bg = cx.background_executor().clone();
        let wait_checked = |this: &gpui::WeakEntity<Self>, cx: &mut gpui::AsyncWindowContext| {
            this.update(cx, |this, cx| this.active_editor().is_some_and(|e| e.read(cx).diag_version == e.read(cx).buffer.version()) || this.problems.error.is_some()).unwrap_or(true)
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
            let _ = this.update(cx, |this, cx| {
                match &this.problems.project {
                    Some(Ok(v)) => {
                        let files: std::collections::BTreeSet<_> = v.iter().map(|d| d.file.clone()).collect();
                        eprintln!("smoke-lint: project {} problem(s) in {} file(s) after {:.1?}", v.len(), files.len(), t0.elapsed());
                    }
                    other => eprintln!("smoke-lint: project {:?}", other.as_ref().map(|r| r.as_ref().err())),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Workspace {
    /// Ctrl+Alt+L (the selected lines, else the file) and Ctrl+Alt+O, with ruff.
    pub(super) fn format_action(&mut self, action: brisk_keymap::Action, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.focused_editor(window, cx).or_else(|| self.active_editor().cloned()) else { return };
        let (path, python) = {
            let e = ed.read(cx);
            (e.path.clone(), e.is_python())
        };
        let Some(path) = path.filter(|_| python) else {
            self.status = "Reformat Code and Optimize Imports work on Python files (ruff)".into();
            cx.notify();
            return;
        };
        let Some(ruff) = self.ruff() else {
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
        let result = if reformat {
            brisk_lint::format_text(&ruff, &self.root, &path, &text, lines)
        } else {
            brisk_lint::optimize_imports(&ruff, &self.root, &path, &text)
        };
        let what = if reformat { if lines.is_some() { "Reformatted the selected lines" } else { "Reformatted the file" } } else { "Optimized imports" };
        match result {
            Ok(out) if out.is_empty() && !text.is_empty() => self.status = "ruff returned nothing; the file was not changed".into(),
            Ok(out) => {
                let changed = out.replace("\r\n", "\n") != text.replace("\r\n", "\n");
                if changed {
                    ed.update(cx, |e, cx| e.replace_all_text(&out, cx));
                }
                if reformat {
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
