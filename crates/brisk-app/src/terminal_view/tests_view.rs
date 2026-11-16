//! The test runner part of a Run tab, like PyCharm's: a live tree of tests on the left,
//! the console (or the selected test's output and failure) on the right.

use super::{RunEvent, TerminalPanel, TermEvent, LINK};
use crate::theme::{Theme, MONO_FONT};
use brisk_run::{TestFeed, TestStatus, TestTree};
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
    pub details_scroll: ScrollHandle,
    pub finished: bool,
}

impl Drop for TestsView {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// What a finished test run reports to the workspace.
pub struct TestsOutcome {
    pub summary: String,
    pub failed: bool,
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
        TestsView { feed, stop, selected: 0, collapsed: HashSet::new(), scroll: UniformListScrollHandle::new(), details_scroll: ScrollHandle::new(), finished: false }
    }

    pub fn tree(&self) -> std::sync::MutexGuard<'_, TestFeed> {
        self.feed.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The process exited: read the rest and select the first failed test, as PyCharm does.
    pub fn finish(&mut self) -> TestsOutcome {
        self.stop.store(true, Ordering::SeqCst);
        self.finished = true;
        let (summary, failed, first) = {
            let mut f = self.tree();
            f.poll();
            let c = f.tree.counts();
            (f.tree.summary(true), c.failed > 0, f.tree.first_failed())
        };
        if let Some(i) = first {
            self.select(i);
        }
        TestsOutcome { summary, failed }
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
            f.tree.rows(&self.collapsed)
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
        for i in f.tree.rows(&t.collapsed) {
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
        if mods.control || mods.alt || mods.platform {
            return None;
        }
        let t = self.tests()?;
        let (rows, sel, has_children, parent, expanded) = {
            let f = t.tree();
            let n = f.tree.nodes.get(t.selected)?;
            (f.tree.rows(&t.collapsed), t.selected, !n.children.is_empty(), n.parent, !t.collapsed.contains(&t.selected))
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
        let n_rows = t.tree().tree.rows(&t.collapsed).len();
        let (g, c) = status_glyph(root_status);
        div()
            .w(px(320.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(Theme::divider())
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
                    .child(div().text_color(c).child(g))
                    .child(div().text_color(Theme::text_muted()).overflow_hidden().whitespace_nowrap().child(summary)),
            )
            .child(
                uniform_list(
                    "test-tree",
                    n_rows,
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        let Some(t) = this.tests() else { return Vec::new() };
                        let f = t.tree();
                        let rows = f.tree.rows(&t.collapsed);
                        range
                            .filter_map(|ix| rows.get(ix).copied())
                            .map(|i| {
                                let n = &f.tree.nodes[i];
                                let (g, c) = status_glyph(n.status);
                                let arrow = if n.children.is_empty() { " " } else if t.collapsed.contains(&i) { "▸" } else { "▾" };
                                let done = matches!(n.status, TestStatus::Passed | TestStatus::Failed | TestStatus::Error | TestStatus::Skipped);
                                let dur = (done && n.duration > 0.0).then(|| brisk_run::pytest::format_duration(n.duration));
                                div()
                                    .id(("test-row", i))
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
                                    .child(
                                        div().id(("test-arrow", i)).w(px(10.)).text_color(Theme::text_muted()).child(arrow).on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            if let Some(t) = this.tests_mut() {
                                                if !t.collapsed.remove(&i) {
                                                    t.collapsed.insert(i);
                                                }
                                            }
                                            cx.notify();
                                        })),
                                    )
                                    .child(div().w(px(12.)).text_color(c).child(g))
                                    .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_color(Theme::text()).child(n.name.clone()))
                                    .when_some(dur, |d, s| d.child(div().text_size(px(11.)).text_color(Theme::text_dim()).child(s)))
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
                div()
                    .min_h(px(super::CELL_H))
                    .flex()
                    .whitespace_nowrap()
                    .text_color(if red { rgb(0xf05d5d).into() } else { Theme::text() })
                    .children(pieces)
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
            .font(font(MONO_FONT))
            .text_size(px(super::FONT_SIZE))
            .children(lines)
    }
}
