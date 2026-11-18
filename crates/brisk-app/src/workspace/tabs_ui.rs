//! Editor tabs and the split editor, as in PyCharm: a tab context menu (Close, Close Others,
//! Close All, to the Left / Right, Split Right / Down, Move to Opposite Group, Unsplit),
//! Reopen Closed Tab, and two editor groups side by side or stacked.
//!
//! The focused group's tabs live in `Workspace::tabs` / `active`, so everything that works on
//! "the current tabs" keeps working; the other group waits in `split` and they swap places
//! when focus moves. A file is open in one group at a time.

use super::menu_ui::Entry;
use super::{Tab, Workspace};
use crate::editor::Editor;
use crate::theme::{Theme, MONO_FONT};
use brisk_keymap::Action;
use gpui::prelude::*;
use gpui::{div, px, Context, Entity, MouseButton, Pixels, Point, Window};
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Default)]
pub struct Pane {
    pub tabs: Vec<Tab>,
    pub active: Option<usize>,
}

pub struct Split {
    /// The group without focus.
    pub other: Pane,
    /// The focused group (in `Workspace::tabs`) is the second one (right or bottom).
    pub second: bool,
    /// Side by side (Split Right) rather than stacked (Split Down).
    pub vertical: bool,
}

impl Workspace {
    /// Tabs of both groups.
    pub(super) fn all_tabs(&self) -> impl Iterator<Item = &Tab> {
        self.tabs.iter().chain(self.split.iter().flat_map(|s| s.other.tabs.iter()))
    }

    fn tab_path(t: &Tab, cx: &gpui::App) -> Option<PathBuf> {
        match t {
            Tab::Editor(e) => e.read(cx).path.clone(),
            _ => None,
        }
    }

    /// Make a group the focused one (no-op without a split or when it already is).
    pub(super) fn focus_pane(&mut self, second: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.split.as_mut() else { return };
        if s.second == second {
            return;
        }
        std::mem::swap(&mut self.tabs, &mut s.other.tabs);
        std::mem::swap(&mut self.active, &mut s.other.active);
        s.second = second;
        self.focus_editor(window, cx);
        cx.notify();
    }

    /// If `path` is open in the other group, focus that group and its tab. True if found.
    pub(super) fn activate_in_other_pane(&mut self, path: &std::path::Path, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(s) = &self.split else { return false };
        let Some(i) = s.other.tabs.iter().position(|t| Self::tab_path(t, cx).as_deref() == Some(path)) else { return false };
        let second = !s.second;
        self.focus_pane(second, window, cx);
        self.active = Some(i);
        true
    }

    /// When a group runs out of tabs, the split goes away.
    pub(super) fn collapse_empty_pane(&mut self) {
        let Some(s) = &self.split else { return };
        if self.tabs.is_empty() {
            let s = self.split.take().expect("split");
            self.tabs = s.other.tabs;
            self.active = s.other.active;
        } else if s.other.tabs.is_empty() {
            self.split = None;
        }
    }

    /// Split Right / Split Down: the current tab moves to a new group. With a single tab,
    /// the most recent other file opens in the first group.
    pub(super) fn split_editor(&mut self, vertical: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.split.as_mut() {
            s.vertical = vertical;
            self.move_to_opposite(window, cx);
            return;
        }
        let Some(i) = self.active else { return };
        if self.tabs.len() < 2 {
            let open: Vec<PathBuf> = self.all_tabs().filter_map(|t| Self::tab_path(t, cx)).collect();
            let Some(p) = self.recent.iter().find(|p| !open.contains(p) && p.is_file()).cloned() else {
                self.status = "Open another file to split the editor".into();
                cx.notify();
                return;
            };
            self.open_file(&p, window, cx);
            self.active = Some(i);
        }
        let tab = self.tabs.remove(i);
        self.active = Some(i.min(self.tabs.len() - 1));
        let first = Pane { tabs: std::mem::take(&mut self.tabs), active: self.active };
        self.tabs = vec![tab];
        self.active = Some(0);
        self.split = Some(Split { other: first, second: true, vertical });
        self.focus_editor(window, cx);
        cx.notify();
    }

    pub(super) fn move_to_opposite(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.active else { return };
        let Some(second) = self.split.as_ref().map(|s| s.second) else {
            self.split_editor(true, window, cx);
            return;
        };
        let tab = self.tabs.remove(i);
        self.active = if self.tabs.is_empty() { None } else { Some(i.min(self.tabs.len() - 1)) };
        self.focus_pane(!second, window, cx);
        self.add_tab(tab);
        self.collapse_empty_pane();
        self.focus_editor(window, cx);
        cx.notify();
    }

    pub(super) fn unsplit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.split.take() else { return };
        let current = self.active.and_then(|i| self.tabs.get(i)).cloned();
        let mut tabs = s.other.tabs;
        if s.second {
            tabs.extend(std::mem::take(&mut self.tabs));
        } else {
            let mut mine = std::mem::take(&mut self.tabs);
            mine.extend(tabs);
            tabs = mine;
        }
        self.tabs = tabs;
        self.active = current.and_then(|c| self.tabs.iter().position(|t| same_tab(t, &c))).or(Some(0)).filter(|_| !self.tabs.is_empty());
        self.focus_editor(window, cx);
        cx.notify();
    }

    /// Close the tabs of the current group for which `pick(index)` is true.
    /// Tabs at the start of the strip that are pinned.
    pub(super) fn pinned_count(&self) -> usize {
        self.tabs.iter().take_while(|t| self.pinned.contains(&t.entity_id())).count()
    }

    /// Close the picked tabs of the focused group; pinned tabs stay (as in PyCharm).
    fn close_where(&mut self, pick: impl Fn(usize) -> bool, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.tabs.len();
        let had_split = self.split.is_some();
        let pins: Vec<bool> = self.tabs.iter().map(|t| self.pinned.contains(&t.entity_id())).collect();
        for j in (0..n).rev() {
            if pick(j) && !pins[j] && j < self.tabs.len() {
                self.close_tab(j, window, cx);
                // The group closed: what is left belongs to the other one.
                if had_split && self.split.is_none() {
                    break;
                }
            }
        }
    }

    /// Remember a closed file for Reopen Closed Tab.
    pub(super) fn remember_closed(&mut self, p: PathBuf) {
        self.closed.retain(|c| *c != p);
        self.closed.push(p);
        if self.closed.len() > 30 {
            self.closed.remove(0);
        }
    }

    /// Tab and split actions. True if handled.
    pub(super) fn tab_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) -> bool {
        use Action as A;
        let cur = self.active;
        match action {
            A::CloseOtherTabs => self.close_where(|j| Some(j) != cur, window, cx),
            A::CloseAllTabs => {
                self.close_where(|_| true, window, cx);
                if self.split.is_none() {
                    self.close_where(|_| true, window, cx);
                }
            }
            A::CloseTabsToRight => self.close_where(|j| cur.is_some_and(|c| j > c), window, cx),
            A::CloseTabsToLeft => self.close_where(|j| cur.is_some_and(|c| j < c), window, cx),
            A::PinTab => {
                let Some(i) = cur.filter(|&i| i < self.tabs.len()) else { return true };
                let id = self.tabs[i].entity_id();
                let tab = self.tabs.remove(i);
                let pin = !self.pinned.remove(&id);
                if pin {
                    self.pinned.insert(id);
                }
                // Pinned tabs come first; a newly pinned tab goes last among them, an unpinned
                // one first after them.
                let at = self.pinned_count();
                self.tabs.insert(at, tab);
                self.active = Some(at);
                self.status = if pin { "Tab pinned" } else { "Tab unpinned" }.into();
            }
            A::ReopenClosedTab => {
                while let Some(p) = self.closed.pop() {
                    if p.is_file() {
                        self.open_file(&p, window, cx);
                        break;
                    }
                }
            }
            A::SplitVertically => self.split_editor(true, window, cx),
            A::SplitHorizontally => self.split_editor(false, window, cx),
            A::Unsplit => self.unsplit(window, cx),
            A::MoveToOppositeGroup => self.move_to_opposite(window, cx),
            A::NextSplitter => {
                if let Some(s) = &self.split {
                    let second = !s.second;
                    self.focus_pane(second, window, cx);
                }
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Right click on a tab.
    pub(super) fn tab_menu(&mut self, second: bool, i: usize, pos: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_pane(second, window, cx);
        self.active = Some(i);
        let n = self.tabs.len();
        let split = self.split.is_some();
        let mut v = vec![
            self.action_entry(
                if self.tabs.get(i).is_some_and(|t| self.pinned.contains(&t.entity_id())) { "Unpin Tab" } else { "Pin Tab" },
                Action::PinTab,
                true,
                false,
            ),
            Entry::Separator,
            self.action_entry("Close", Action::CloseTab, true, false),
            self.action_entry("Close Other Tabs", Action::CloseOtherTabs, n > 1, false),
            self.action_entry("Close All Tabs", Action::CloseAllTabs, true, false),
            self.action_entry("Close Tabs to the Left", Action::CloseTabsToLeft, i > 0, false),
            self.action_entry("Close Tabs to the Right", Action::CloseTabsToRight, i + 1 < n, false),
            self.action_entry("Reopen Closed Tab", Action::ReopenClosedTab, !self.closed.is_empty(), false),
            Entry::Separator,
            self.action_entry("Split Right", Action::SplitVertically, true, false),
            self.action_entry("Split Down", Action::SplitHorizontally, true, false),
        ];
        if split {
            v.push(self.action_entry("Move to Opposite Group", Action::MoveToOppositeGroup, true, false));
            v.push(self.action_entry("Unsplit", Action::Unsplit, true, false));
        }
        if let Some(p) = self.tabs.get(i).and_then(|t| Self::tab_path(t, cx)) {
            v.push(Entry::Separator);
            v.push(self.path_entries(p.clone()));
            v.push(Entry::Item {
                label: "Show in Explorer".into(),
                shortcut: String::new(),
                enabled: true,
                run: Rc::new(move |_, _, cx| cx.reveal_path(&p)),
            });
        }
        self.open_menu(pos, v, cx);
    }

    fn pane_content(&self, tabs: &[Tab], active: Option<usize>) -> gpui::AnyElement {
        match active.and_then(|i| tabs.get(i)) {
            Some(Tab::Editor(e)) => e.clone().into_any_element(),
            Some(Tab::Diff(d)) => d.clone().into_any_element(),
            Some(Tab::Grid(g)) => g.clone().into_any_element(),
            None => self.render_welcome().into_any_element(),
        }
    }

    fn render_pane(&self, tabs: &[Tab], active: Option<usize>, focused: bool, second: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .id(("pane", second as usize))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| this.focus_pane(second, window, cx)))
            .when(!tabs.is_empty(), |d| d.child(self.render_tabs(tabs, active, focused, second, cx)))
            .child(div().flex_1().min_h_0().font_family(MONO_FONT).child(self.pane_content(tabs, active)))
            .into_any_element()
    }

    /// The editor area: one group, or two with a divider.
    pub(super) fn render_editor_area(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(s) = &self.split else {
            return self.render_pane(&self.tabs, self.active, true, false, cx);
        };
        let mine = self.render_pane(&self.tabs, self.active, true, s.second, cx);
        let other = self.render_pane(&s.other.tabs, s.other.active, false, !s.second, cx);
        let (a, b) = if s.second { (other, mine) } else { (mine, other) };
        let line = div().flex_shrink_0().bg(Theme::divider());
        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .when(s.vertical, |d| d.flex_row())
            .when(!s.vertical, |d| d.flex_col())
            .child(a)
            .child(if s.vertical { line.w(px(1.)).h_full() } else { line.h(px(1.)).w_full() })
            .child(b)
            .into_any_element()
    }

    /// Close an editor wherever it is (the disk poll, deleted files).
    pub(super) fn close_editor(&mut self, e: &Entity<Editor>, window: &mut Window, cx: &mut Context<Self>) {
        let find = |tabs: &[Tab]| tabs.iter().position(|t| matches!(t, Tab::Editor(x) if x == e));
        if let Some(i) = find(&self.tabs) {
            self.close_tab(i, window, cx);
        } else if let Some(s) = &self.split {
            if find(&s.other.tabs).is_some() {
                let second = !s.second;
                self.focus_pane(second, window, cx);
                if let Some(i) = find(&self.tabs) {
                    self.close_tab(i, window, cx);
                }
            }
        }
    }

    /// Smoke checks: the groups as file names, the focused one first, and the split direction.
    pub(super) fn panes_for_smoke(&self, cx: &gpui::App) -> String {
        let names = |tabs: &[Tab], active: Option<usize>| {
            tabs.iter()
                .enumerate()
                .map(|(i, t)| {
                    let n = Self::tab_path(t, cx).and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "?".into());
                    if Some(i) == active { format!("[{n}]") } else { n }
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        match &self.split {
            None => names(&self.tabs, self.active),
            Some(s) => format!(
                "{} | other: {} ({}, focused={})",
                names(&self.tabs, self.active),
                names(&s.other.tabs, s.other.active),
                if s.vertical { "right" } else { "down" },
                if s.second { "second" } else { "first" }
            ),
        }
    }
}

fn same_tab(a: &Tab, b: &Tab) -> bool {
    match (a, b) {
        (Tab::Editor(x), Tab::Editor(y)) => x == y,
        (Tab::Diff(x), Tab::Diff(y)) => x == y,
        (Tab::Grid(x), Tab::Grid(y)) => x == y,
        _ => false,
    }
}
