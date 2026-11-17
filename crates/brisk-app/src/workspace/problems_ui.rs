//! Inspections: ruff checks of open Python files as you type, the Problems tool window
//! (Alt+6, "File" and "Project" tabs), F2 / Shift+F2 and Alt+Enter quick fixes.

use super::{BottomTool, PopupItem, PopupKind, Target, Workspace};
use crate::editor::{DiagSource, Editor, EditorEvent};
use crate::theme::{Theme, MONO_FONT};
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
    intents: Vec<PopupItem>,
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
            ev => this.lsp_editor_event(ed.clone(), ev, window, cx),
        })
        .detach();
        self.schedule_lint(ed.clone(), Duration::ZERO, cx);
    }

    fn schedule_lint(&mut self, ed: Entity<Editor>, delay: Duration, cx: &mut Context<Self>) {
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
                .hover(|s| s.bg(Theme::hover_row()))
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
                                .when(selected, |d| d.bg(if focused { Theme::selection_row() } else { Theme::hover_row() }))
                                .hover(|s| s.bg(Theme::hover_row()))
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
                                        .child(div().text_color(Theme::text_dim()).font_family(MONO_FONT).text_size(px(12.)).child(format!(":{}", d.start.line + 1)))
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
        let probs = ed.read(cx).problems_at_caret();
        if probs.is_empty() {
            self.status = "No context actions available here".into();
            return;
        }
        let mut items = Vec::new();
        let item = |label: String, detail: String, intent: Intent| PopupItem { label, detail, hint: String::new(), target: Target::Intention(Box::new(intent)) };
        for d in &probs {
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
            items.push(item(format!("Suppress '{}' for line", d.code), if brisk_lint::is_ty_code(&d.code) { "# ty: ignore".into() } else { "# noqa".into() }, Intent::Suppress(d.clone())));
        }
        self.problems.intents = items;
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
