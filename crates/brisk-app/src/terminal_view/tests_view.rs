//! The test runner part of a Run tab, like PyCharm's: a live tree of tests on the left,
//! the console (or the selected test's output and failure) on the right.

use super::{RunEvent, TermEvent, TerminalPanel, LINK};
use crate::theme::{mono_font, Theme};
use brisk_run::pytest::history;
use brisk_run::{TestFeed, TestStatus, TestTree, TreeView};
use futures::channel::mpsc;
use gpui::prelude::*;
use gpui::{div, font, px, rgb, uniform_list, Context, Hsla, ScrollHandle, ScrollStrategy, UniformListScrollHandle};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const ROW_H: f32 = 22.0;

pub(super) struct TestsView {
    pub feed: Arc<Mutex<TestFeed>>,
    stop: Arc<AtomicBool>,
    pub selected: usize,
    pub collapsed: HashSet<usize>,
    pub scroll: UniformListScrollHandle,
    pub bar: crate::scrollbar::BarState,
    pub details_scroll: ScrollHandle,
    pub finished: bool,
    /// Show Passed / Show Ignored / Sort by Duration.
    pub view: TreeView,
    /// The visible rows for (tree revision, view, collapsed set): rebuilt when one of them
    /// changes, not on every frame.
    rows_cache: std::cell::RefCell<Option<(RowsKey, std::rc::Rc<Vec<usize>>)>>,
}

/// (tree revision, view options, collapsed count, collapsed fingerprint).
type RowsKey = (u64, TreeView, usize, u64);

impl Drop for TestsView {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// What a finished test run reports to the workspace.
pub struct TestsOutcome {
    pub summary: String,
    pub failed: bool,
    /// Every test that ran: (file, node id, status), for the editor's run icons.
    pub results: Vec<(Option<PathBuf>, String, TestStatus)>,
}

/// The status as an icon of the test tree.
pub(super) fn status_icon(s: TestStatus) -> gpui::Svg {
    let (name, color): (&str, Hsla) = match s {
        TestStatus::Passed => ("circle-check", crate::theme::Theme::run_passed()),
        TestStatus::Failed => ("circle-x", Theme::error()),
        TestStatus::Error => ("circle-alert", Theme::error()),
        TestStatus::Skipped => ("circle-slash", Theme::text_muted()),
        TestStatus::Running => ("loader-circle", rgb(0x6c9be8).into()),
        TestStatus::Pending => ("circle", Theme::text_dim()),
    };
    crate::icons::icon(name, 14., color)
}

pub(super) fn status_glyph(s: TestStatus) -> (&'static str, Hsla) {
    match s {
        TestStatus::Passed => ("✓", rgb(0x5fb865).into()),
        TestStatus::Failed => ("✗", rgb(0xf05d5d).into()),
        TestStatus::Error => ("!", rgb(0xf05d5d).into()),
        TestStatus::Skipped => ("⊘", Theme::text_muted()),
        TestStatus::Running => ("◌", rgb(0x6c9be8).into()),
        TestStatus::Pending => ("○", Theme::text_dim()),
    }
}

impl TestsView {
    /// Tail the plugin's events file in the background; every change wakes the panel.
    pub fn start(events: PathBuf, tab: u64, tx: mpsc::UnboundedSender<(u64, TermEvent)>) -> Self {
        let feed = Arc::new(Mutex::new(TestFeed::new(events)));
        let stop = Arc::new(AtomicBool::new(false));
        {
            let (feed, stop) = (feed.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(100));
                    let changed = feed.lock().unwrap_or_else(|e| e.into_inner()).poll();
                    if changed && tx.unbounded_send((tab, TermEvent::Wakeup)).is_err() {
                        break;
                    }
                }
            });
        }
        TestsView {
            feed,
            stop,
            selected: 0,
            collapsed: HashSet::new(),
            scroll: UniformListScrollHandle::new(),
            bar: Default::default(),
            details_scroll: ScrollHandle::new(),
            finished: false,
            view: TreeView::default(),
            rows_cache: Default::default(),
        }
    }

    /// A finished run from the Test History.
    pub fn done(tree: TestTree) -> Self {
        let mut t = TestsView {
            feed: Arc::new(Mutex::new(TestFeed::done(tree))),
            stop: Arc::new(AtomicBool::new(true)),
            selected: 0,
            collapsed: HashSet::new(),
            scroll: UniformListScrollHandle::new(),
            bar: Default::default(),
            details_scroll: ScrollHandle::new(),
            finished: true,
            view: TreeView::default(),
            rows_cache: Default::default(),
        };
        let first = t.tree().tree.first_failed();
        if let Some(i) = first {
            t.select(i);
        }
        t
    }

    /// Visible rows with the view options.
    pub fn rows(&self) -> std::rc::Rc<Vec<usize>> {
        let f = self.tree();
        self.rows_of(&f)
    }

    /// `rows` with the feed already locked; cached.
    pub fn rows_of(&self, f: &TestFeed) -> std::rc::Rc<Vec<usize>> {
        // The collapsed set is small (folded nodes): an order-independent fingerprint of it.
        let fp = self.collapsed.iter().fold(0u64, |a, &x| a.wrapping_add((x as u64 ^ 0x9e37_79b9_7f4a_7c15).wrapping_mul(0xbf58_476d_1ce4_e5b9)));
        let key = (f.tree.revision, self.view, self.collapsed.len(), fp);
        if let Some((k, rows)) = &*self.rows_cache.borrow() {
            if *k == key {
                return rows.clone();
            }
        }
        let rows = std::rc::Rc::new(f.tree.rows_view(&self.collapsed, self.view));
        *self.rows_cache.borrow_mut() = Some((key, rows.clone()));
        rows
    }

    pub fn tree(&self) -> std::sync::MutexGuard<'_, TestFeed> {
        self.feed.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The process exited: read the rest and select the first failed test, as PyCharm does.
    pub fn finish(&mut self) -> TestsOutcome {
        self.stop.store(true, Ordering::SeqCst);
        self.finished = true;
        let (summary, failed, first, results) = {
            let mut f = self.tree();
            f.poll();
            let c = f.tree.counts();
            let root = f.tree.rootdir.clone();
            let results = f
                .tree
                .nodes
                .iter()
                .filter(|n| n.is_test)
                .map(|n| {
                    (
                        n.file.as_ref().map(|p| if p.is_absolute() { p.clone() } else { root.as_ref().map(|r| r.join(p)).unwrap_or_else(|| p.clone()) }),
                        n.id.clone(),
                        n.status,
                    )
                })
                .collect();
            (f.tree.summary(true), c.failed > 0, f.tree.first_failed(), results)
        };
        if let Some(i) = first {
            self.select(i);
        }
        TestsOutcome { summary, failed, results }
    }

    /// Select a node, expanding its parents and scrolling it into view.
    pub fn select(&mut self, i: usize) {
        let feed = self.feed.clone();
        let rows = {
            let f = feed.lock().unwrap_or_else(|e| e.into_inner());
            let mut p = f.tree.nodes.get(i).and_then(|n| n.parent);
            while let Some(x) = p {
                self.collapsed.remove(&x);
                p = f.tree.nodes[x].parent;
            }
            f.tree.rows_view(&self.collapsed, self.view)
        };
        self.selected = i;
        if let Some(ix) = rows.iter().position(|&r| r == i) {
            self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
        }
    }

    /// Whether the right side shows the selected test instead of the whole console.
    pub fn shows_details(&self) -> bool {
        self.tree().tree.nodes.get(self.selected).is_some_and(|n| n.is_test)
    }
}

/// Where Enter / F4 / double click on a node goes: the test's line, the class definition,
/// the top of the module.
pub(super) fn node_location(tree: &TestTree, i: usize) -> Option<(PathBuf, usize)> {
    let n = tree.nodes.get(i)?;
    let file = n.file.clone().filter(|f| f.is_file())?;
    if let Some(l) = n.line {
        return Some((file, l));
    }
    if !n.is_test && n.depth > 1 {
        // A class: find its definition.
        if let Ok(text) = std::fs::read_to_string(&file) {
            let needle = format!("class {}", n.name);
            if let Some(l) = text.lines().position(|l| {
                let t = l.trim_start();
                t.starts_with(&needle) && t[needle.len()..].starts_with(['(', ':'])
            }) {
                return Some((file, l + 1));
            }
        }
    }
    Some((file, 1))
}

impl TerminalPanel {
    pub(super) fn tests(&self) -> Option<&TestsView> {
        self.tabs.get(self.active)?.run.as_ref()?.tests.as_ref()
    }

    fn tests_mut(&mut self) -> Option<&mut TestsView> {
        self.tabs.get_mut(self.active)?.run.as_mut()?.tests.as_mut()
    }

    /// The active tab's test summary (smoke tests, status bar).
    pub fn tests_summary(&self) -> Option<String> {
        let t = self.tests()?;
        let s = t.tree().tree.summary(t.finished);
        Some(s)
    }

    /// The tree as indented text with status glyphs (smoke tests).
    pub fn tests_tree_text(&self) -> Option<String> {
        let t = self.tests()?;
        let f = t.tree();
        let mut s = String::new();
        for i in f.tree.rows_view(&t.collapsed, t.view) {
            let n = &f.tree.nodes[i];
            s.push_str(&format!("{}{} {}\n", "  ".repeat(n.depth), status_glyph(n.status).0, n.name));
        }
        Some(s)
    }

    /// Absolute targets of the failed tests in the active tab.
    pub fn failed_tests(&self) -> Vec<String> {
        self.tests().map(|t| t.tree().tree.failed_targets()).unwrap_or_default()
    }

    /// The active tab's selected test and where it is defined (smoke tests).
    pub fn selected_test(&self) -> Option<(String, Option<(PathBuf, usize)>)> {
        let t = self.tests()?;
        let f = t.tree();
        let n = f.tree.nodes.get(t.selected)?;
        Some((n.name.clone(), node_location(&f.tree, t.selected)))
    }

    /// Show Passed / Show Ignored / Sort by Duration on the active tab's tree.
    pub fn set_tests_view(&mut self, f: impl FnOnce(&mut TreeView)) {
        if let Some(t) = self.tests_mut() {
            f(&mut t.view);
            let sel = t.selected;
            t.select(sel);
        }
    }

    /// "Export Test Results": the active tab's tree as an HTML page in
    /// `.brisk/test-results`; returns its path.
    pub fn export_tests(&self) -> Result<PathBuf, String> {
        let t = self.tests().ok_or("No test results in this tab")?;
        let name = self.tabs.get(self.active).and_then(|t| t.run.as_ref()).map(|r| r.name.clone()).unwrap_or_default();
        let html = t.tree().tree.to_html(&format!("Test Results: {name}"));
        let dir = self.root.join(".brisk").join("test-results");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let safe: String = name.chars().map(|c| if c.is_alphanumeric() || " _-.".contains(c) { c } else { '_' }).collect();
        let p = dir.join(format!("{}.html", if safe.is_empty() { "tests".into() } else { safe }));
        std::fs::write(&p, html).map_err(|e| format!("Cannot write {}: {e}", p.display()))?;
        Ok(p)
    }

    /// Keep a finished test run in the Test History.
    pub(super) fn save_test_history(root: &std::path::Path, name: &str, t: &TestsView) {
        let events = t.tree().events_text();
        if !events.trim().is_empty() {
            let _ = history::save(root, name, &events);
        }
    }

    /// Open a run from the Test History in a tab of its own.
    pub fn open_history(&mut self, e: &history::Entry) -> Result<(), String> {
        let tree = history::load(e).ok_or_else(|| format!("Cannot read {}", e.path.display()))?;
        let summary = tree.summary(true);
        let (cols, rows) = self.tabs.get(self.active).map(|t| t.session.size()).unwrap_or((120, 12));
        let when = crate::workspace::fmt_time(e.time);
        let session = brisk_terminal::Session::offline(&format!("Test History: '{}', {when}\n{summary}\n", e.name), cols, rows);
        let id = self.next_id;
        self.next_id += 1;
        let name = format!("{} ({when})", e.name);
        let cfg = brisk_run::RunConfig { name: name.clone(), kind: brisk_run::RunKind::Pytest, ..Default::default() };
        let cmd = brisk_run::Command { program: PathBuf::new(), args: Vec::new(), cwd: self.root.clone(), env: Default::default() };
        let tests = Some(TestsView::done(tree));
        let now = std::time::Instant::now();
        let run = super::RunTab { name, cfg, cmd, running: false, code: None, tests, started: now, ended: Some(now), pinned: true, stopped: false };
        self.tabs.push(super::Tab { id, session, title: String::new(), number: 0, shell: String::new(), run: Some(run) });
        self.active = self.tabs.len() - 1;
        self.trim_run_tabs();
        self.history_open = false;
        Ok(())
    }

    /// The temporary configuration that runs node `i` of the active tab's tree on its own
    /// (a test, one parameter set, a class, a module), like PyCharm's Run on a tree node.
    pub fn node_config(&self, i: usize) -> Option<brisk_run::RunConfig> {
        let r = self.tabs.get(self.active)?.run.as_ref()?;
        let t = r.tests.as_ref()?;
        let f = t.tree();
        let n = f.tree.nodes.get(i)?;
        if i == 0 {
            return Some(r.cfg.clone());
        }
        let parts = brisk_run::split_nodeid(&n.id);
        let mut file = n.file.clone().or_else(|| f.tree.rootdir.as_ref().map(|d| d.join(parts[0])))?;
        if !file.is_absolute() {
            file = r.cmd.cwd.join(file);
        }
        // A parameter group's id is `file::test_x`; a parameter set's `file::test_x[2]`.
        let scope: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
        let mut cfg = brisk_run::RunConfig::for_tests_with(&self.root, &file, &scope, r.cfg.kind);
        cfg.working_directory = brisk_run::relativize(&self.root, &r.cmd.cwd);
        cfg.interpreter = r.cfg.interpreter.clone();
        cfg.env = r.cfg.env.clone();
        cfg.env_files = r.cfg.env_files.clone();
        cfg.interpreter_options = r.cfg.interpreter_options.clone();
        Some(cfg)
    }

    /// ▶ on a tree node (and Ctrl+Shift+F10 in the tree): run just that node.
    pub fn run_node(&mut self, i: usize, cx: &mut Context<Self>) -> bool {
        match self.node_config(i) {
            Some(cfg) => {
                cx.emit(RunEvent::RunTests(Box::new(cfg)));
                true
            }
            None => false,
        }
    }

    /// Select the tree node named `name` (smoke tests).
    pub fn select_test_named(&mut self, name: &str) -> Option<usize> {
        let t = self.tests_mut()?;
        let i = t.tree().tree.nodes.iter().position(|n| n.name == name)?;
        t.select(i);
        Some(i)
    }

    /// "Rerun Failed Tests": the same configuration limited to the failed nodes, from the
    /// same working directory.
    pub fn rerun_failed(&mut self) -> Result<Option<String>, String> {
        let failed = self.failed_tests();
        let Some(r) = self.tabs.get(self.active).and_then(|t| t.run.as_ref()) else { return Ok(None) };
        if failed.is_empty() {
            return Ok(None);
        }
        let mut cfg = r.cfg.clone();
        cfg.target = failed.iter().map(|f| format!("\"{f}\"")).collect::<Vec<_>>().join(" ");
        cfg.keywords.clear();
        cfg.working_directory = r.cmd.cwd.display().to_string();
        let cmd = brisk_run::build(&self.root, &cfg, &r.cmd.program);
        let name = cfg.name.clone();
        self.run(cfg, cmd)?;
        Ok(Some(name))
    }

    /// Up/Down/Left/Right/Enter/F4 in the test tree. `None`: not a tree key.
    pub(super) fn tests_key(&mut self, key: &str, mods: gpui::Modifiers, cx: &mut Context<Self>) -> Option<bool> {
        if key == "f10" && mods.control && mods.shift && !mods.alt {
            let sel = self.tests()?.selected;
            return Some(self.run_node(sel, cx));
        }
        if mods.control || mods.alt || mods.platform {
            return None;
        }
        let t = self.tests()?;
        let (rows, sel, has_children, parent, expanded) = {
            let f = t.tree();
            let n = f.tree.nodes.get(t.selected)?;
            (f.tree.rows_view(&t.collapsed, t.view), t.selected, !n.children.is_empty(), n.parent, !t.collapsed.contains(&t.selected))
        };
        let pos = rows.iter().position(|&r| r == sel).unwrap_or(0);
        let t = self.tests_mut()?;
        match key {
            "up" => t.select(rows[pos.saturating_sub(1)]),
            "down" => t.select(rows[(pos + 1).min(rows.len() - 1)]),
            "home" => t.select(rows[0]),
            "end" => t.select(rows[rows.len() - 1]),
            "left" if has_children && expanded => {
                t.collapsed.insert(sel);
            }
            "left" => {
                if let Some(p) = parent {
                    t.select(p);
                }
            }
            "right" if has_children && !expanded => {
                t.collapsed.remove(&sel);
            }
            "right" => {
                let first = t.tree().tree.nodes[sel].children.first().copied();
                if let Some(c) = first {
                    t.select(c);
                }
            }
            "enter" | "f4" => {
                let loc = node_location(&t.tree().tree, sel);
                if let Some((p, l)) = loc {
                    cx.emit(RunEvent::Open(p, l));
                }
            }
            _ => return None,
        }
        cx.notify();
        Some(true)
    }

    pub(super) fn render_tests_tree(&self, t: &TestsView, cx: &mut Context<Self>) -> impl IntoElement {
        let (summary, root_status) = {
            let f = t.tree();
            (f.tree.summary(t.finished), f.tree.nodes[0].status)
        };
        let n_rows = t.rows().len();
        let root_icon = status_icon(root_status);
        let view = t.view;
        let tip = |s: &'static str| move |_: &mut gpui::Window, cx: &mut gpui::App| cx.new(|_| crate::workspace::Tip(s.into())).into();
        let tool = |id: &'static str, glyph: &'static str, on: bool| {
            let color = if on { Theme::text() } else { Theme::text_muted() };
            div()
                .id(id)
                .w(px(22.))
                .h(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .text_size(px(12.))
                .text_color(if on { Theme::text() } else { Theme::text_muted() })
                .when(on, |d| d.bg(Theme::selection_row()))
                .when(!on, |d| d.hover(|s| s.bg(Theme::hover_row())))
                .active(|s| s.bg(Theme::press()))
                .cursor_pointer()
                .child(crate::icons::glyph(glyph, 14., color))
        };
        let history = if self.history_open { history::list(&self.root) } else { Vec::new() };
        div()
            .w(px(320.))
            .flex_none()
            .h_full()
            .relative()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(Theme::divider())
            .child(
                div()
                    .h(px(26.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .px(px(4.))
                    .border_b_1()
                    .border_color(Theme::divider())
                    .child(tool("tests-show-passed", "✓", view.show_passed).tooltip(tip("Show Passed")).on_click(cx.listener(|this, _, _, cx| {
                        this.set_tests_view(|v| v.show_passed = !v.show_passed);
                        cx.notify();
                    })))
                    .child(tool("tests-show-ignored", "⊘", view.show_ignored).tooltip(tip("Show Ignored")).on_click(cx.listener(|this, _, _, cx| {
                        this.set_tests_view(|v| v.show_ignored = !v.show_ignored);
                        cx.notify();
                    })))
                    .child(tool("tests-by-duration", "⏱", view.by_duration).tooltip(tip("Sort by Duration")).on_click(cx.listener(|this, _, _, cx| {
                        this.set_tests_view(|v| v.by_duration = !v.by_duration);
                        cx.notify();
                    })))
                    .child(tool("tests-failed-first", "✗↑", self.failed_first).tooltip(tip("Rerun Failed Tests First")).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.failed_first = !this.failed_first;
                            cx.notify();
                        },
                    )))
                    .child(div().flex_1())
                    .child(tool("tests-history", "🕘", self.history_open).tooltip(tip("Test History")).on_click(cx.listener(|this, _, _, cx| {
                        this.history_open = !this.history_open;
                        cx.notify();
                    })))
                    .child(tool("tests-export", "⇪", false).tooltip(tip("Export Test Results (HTML)")).on_click(cx.listener(|this, _, _, cx| {
                        match this.export_tests() {
                            Ok(p) => {
                                this.error = None;
                                cx.emit(RunEvent::OpenUrl(format!("file:///{}", p.display().to_string().replace('\\', "/"))));
                            }
                            Err(e) => this.error = Some(e),
                        }
                        cx.notify();
                    }))),
            )
            .child(
                div()
                    .h(px(24.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(8.))
                    .text_size(px(12.))
                    .border_b_1()
                    .border_color(Theme::divider())
                    .child(root_icon)
                    .child(div().text_color(Theme::text_muted()).overflow_hidden().whitespace_nowrap().child(summary)),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        uniform_list(
                            "test-tree",
                            n_rows,
                            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                let Some(t) = this.tests() else { return Vec::new() };
                                let f = t.tree();
                                let rows = t.rows_of(&f);
                                range
                                    .filter_map(|ix| rows.get(ix).copied())
                                    .map(|i| {
                                        let n = &f.tree.nodes[i];
                                        let icon = status_icon(n.status);
                                        let arrow = crate::icons::tree_arrow(!n.children.is_empty(), !t.collapsed.contains(&i), Theme::text_muted());
                                        let done = matches!(n.status, TestStatus::Passed | TestStatus::Failed | TestStatus::Error | TestStatus::Skipped);
                                        let dur = (done && n.duration > 0.0).then(|| brisk_run::pytest::format_duration(n.duration));
                                        let sel = i == t.selected;
                                        div()
                                            .id(("test-row", i))
                                            .group("test-row")
                                            .h(px(ROW_H))
                                            .w_full()
                                            .flex()
                                            .items_center()
                                            .gap(px(5.))
                                            .pl(px(6. + n.depth as f32 * 14.))
                                            .pr(px(8.))
                                            .text_size(px(13.))
                                            .when(i == t.selected, |d| d.bg(Theme::selection_row()))
                                            .when(i != t.selected, |d| d.hover(|s| s.bg(Theme::hover_row())))
                                            .cursor_pointer()
                                            .child(div().id(("test-arrow", i)).w(px(12.)).child(arrow).on_click(cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                if let Some(t) = this.tests_mut() {
                                                    if !t.collapsed.remove(&i) {
                                                        t.collapsed.insert(i);
                                                    }
                                                }
                                                cx.notify();
                                            })))
                                            .child(icon)
                                            .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_color(Theme::text()).child(n.name.clone()))
                                            .when_some(dur, |d, s| d.child(div().text_size(px(11.)).text_color(Theme::text_dim()).child(s)))
                                            .child(
                                                // ▶: run this node alone (shown on the selected row and on hover).
                                                div()
                                                    .id(("test-run", i))
                                                    .w(px(16.))
                                                    .flex()
                                                    .justify_center()
                                                    .rounded(px(3.))
                                                    .text_size(px(11.))
                                                    .text_color(if sel { rgb(0x5fb865).into() } else { gpui::transparent_black() })
                                                    .group_hover("test-row", |s| s.text_color(rgb(0x5fb865)))
                                                    .hover(|s| s.bg(Theme::hover_row()))
                                                    .active(|s| s.bg(Theme::press()))
                                                    .tooltip(move |_, cx| cx.new(|_| crate::workspace::Tip("Run this node (Ctrl+Shift+F10)".into())).into())
                                                    .child("▶")
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.run_node(i, cx);
                                                    })),
                                            )
                                            .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                                                window.focus(&this.focus);
                                                let open = e.click_count() >= 2;
                                                if let Some(t) = this.tests_mut() {
                                                    t.selected = i;
                                                    if open {
                                                        let loc = node_location(&t.tree().tree, i);
                                                        if let Some((p, l)) = loc {
                                                            cx.emit(RunEvent::Open(p, l));
                                                        }
                                                    }
                                                }
                                                cx.notify();
                                            }))
                                    })
                                    .collect()
                            }),
                        )
                        .flex_1()
                        .track_scroll(t.scroll.clone()),
                    )
                    .child(crate::scrollbar::scrollbar(crate::scrollbar::Source::list(&t.scroll), t.bar.clone())),
            )
            .when(self.history_open, |d| {
                d.child(
                    div()
                        .id("tests-history-list")
                        .absolute()
                        .top(px(26.))
                        .left(px(4.))
                        .w(px(312.))
                        .max_h(px(260.))
                        .overflow_y_scroll()
                        .occlude()
                        .py(px(4.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(Theme::border())
                        .bg(Theme::panel())
                        .shadow_md()
                        .text_size(px(12.))
                        .when(history.is_empty(), |d| d.child(div().px(px(8.)).py(px(4.)).text_color(Theme::text_muted()).child("No test runs yet")))
                        .children(history.into_iter().enumerate().map(|(k, e)| {
                            let when = crate::workspace::fmt_time(e.time);
                            div()
                                .id(("tests-history-row", k))
                                .w_full()
                                .flex()
                                .gap(px(8.))
                                .px(px(8.))
                                .py(px(3.))
                                .hover(|s| s.bg(Theme::hover_row()))
                                .cursor_pointer()
                                .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_color(Theme::text()).child(e.name.clone()))
                                .child(div().text_color(Theme::text_dim()).child(when))
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.error = this.open_history(&e).err();
                                        cx.notify();
                                    }),
                                )
                        })),
                )
            })
    }

    /// The selected test's captured output and failure, links clickable.
    pub(super) fn render_test_details(&self, t: &TestsView, cx: &mut Context<Self>) -> impl IntoElement {
        let text = {
            let f = t.tree();
            f.tree.details(t.selected)
        };
        let rootdir = t.tree().tree.rootdir.clone();
        let lines: Vec<_> = text
            .lines()
            .enumerate()
            .map(|(ln, line)| {
                let red = line.starts_with("E ") || line.contains("Error") && !line.starts_with(' ');
                let chars: Vec<char> = line.chars().collect();
                let mut pieces = Vec::new();
                let mut at = 0;
                for (k, l) in brisk_run::find_links(line).into_iter().enumerate() {
                    let target = self.link_target(&l.path, l.line).or_else(|| {
                        let p = rootdir.as_ref()?.join(&l.path);
                        p.is_file().then_some((p, l.line))
                    });
                    let Some(target) = target else { continue };
                    if l.cols.start > at {
                        pieces.push(div().child(chars[at..l.cols.start].iter().collect::<String>()).into_any_element());
                    }
                    let label: String = chars[l.cols.clone()].iter().collect();
                    pieces.push(
                        div()
                            .id(("test-link", ln * 64 + k))
                            .text_color(super::hsla(LINK))
                            .underline()
                            .cursor_pointer()
                            .child(label)
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(RunEvent::Open(target.0.clone(), target.1))))
                            .into_any_element(),
                    );
                    at = l.cols.end;
                }
                if at < chars.len() {
                    pieces.push(div().child(chars[at..].iter().collect::<String>()).into_any_element());
                }
                div().min_h(px(super::CELL_H)).flex().whitespace_nowrap().text_color(if red { rgb(0xf05d5d).into() } else { Theme::text() }).children(pieces)
            })
            .collect();
        div()
            .id("test-details")
            .flex_1()
            .min_w_0()
            .h_full()
            .px(px(8.))
            .overflow_y_scroll()
            .track_scroll(&t.details_scroll)
            .font(font(mono_font()))
            .text_size(px(super::FONT_SIZE))
            .children(lines)
    }
}
