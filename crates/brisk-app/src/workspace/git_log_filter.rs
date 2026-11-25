//! Filters of the Git log: a text field (message or hash), Branch, User, Date and Paths
//! popups. The log is reloaded through `git log` with the filters, so the graph follows the
//! filtered list.

use super::menu_ui::Entry;
use super::Workspace;
use crate::editor::Editor;
use crate::theme::ui_px;
use crate::theme::Theme;
use brisk_git::LogQuery;
use gpui::prelude::*;
use gpui::{div, px, Context, Entity, Focusable, KeyDownEvent, Subscription, Window};
use std::rc::Rc;
use std::time::Duration;

pub struct LogFilter {
    pub field: Entity<Editor>,
    pub query: LogQuery,
    /// Typing restarts this: the log reloads 250 ms after the last key.
    generation: u64,
    _sub: Subscription,
}

impl LogFilter {
    pub fn new(cx: &mut Context<Workspace>) -> Self {
        let field = cx.new(|cx| {
            let mut e = Editor::scratch("", cx);
            e.buffer.auto_pairs = false;
            e.is_field = true;
            e
        });
        let sub = cx.observe(&field, |this: &mut Workspace, _, cx| this.log_filter_edited(cx));
        Self { field, query: LogQuery::default(), generation: 0, _sub: sub }
    }
}

thread_local! {
    /// Where the Branch, User, Date and Paths chips were painted (window pixels): smoke clicks
    /// aim here.
    pub static CHIP_BOUNDS: std::cell::RefCell<[Option<gpui::Bounds<gpui::Pixels>>; 4]> = const { std::cell::RefCell::new([None; 4]) };
}

/// A canvas filling its parent that records the parent's bounds in `CHIP_BOUNDS[i]`.
fn track(i: usize) -> impl IntoElement {
    gpui::canvas(move |b, _, _| CHIP_BOUNDS.with(|c| c.borrow_mut()[i] = Some(b)), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
}

impl Workspace {
    fn log_filter_text(&self, cx: &Context<Self>) -> Option<String> {
        let g = self.git.as_ref()?;
        Some(g.filter.field.read(cx).buffer.text().replace(['\n', '\r'], " ").trim().to_string())
    }

    fn log_filter_edited(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.log_filter_text(cx) else { return };
        let Some(g) = self.git.as_mut() else { return };
        if g.filter.query.text == text {
            return;
        }
        g.filter.query.text = text;
        g.filter.generation += 1;
        let gen = g.filter.generation;
        let timer = cx.background_executor().timer(Duration::from_millis(250));
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                if this.git.as_ref().is_some_and(|g| g.filter.generation == gen) {
                    this.refresh_git_now(cx);
                }
            });
        })
        .detach();
    }

    pub(super) fn set_log_branch(&mut self, branch: Option<String>, cx: &mut Context<Self>) {
        if let Some(g) = self.git.as_mut() {
            g.filter.query.branch = branch;
        }
        self.refresh_git_now(cx);
        cx.notify();
    }

    pub(super) fn set_log_author(&mut self, author: Option<String>, cx: &mut Context<Self>) {
        if let Some(g) = self.git.as_mut() {
            g.filter.query.author = author;
        }
        self.refresh_git_now(cx);
        cx.notify();
    }

    pub(super) fn set_log_since(&mut self, days: Option<u32>, cx: &mut Context<Self>) {
        if let Some(g) = self.git.as_mut() {
            g.filter.query.since_days = days;
        }
        self.refresh_git_now(cx);
        cx.notify();
    }

    pub(super) fn set_log_path(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        if let Some(g) = self.git.as_mut() {
            g.filter.query.path = path;
        }
        self.refresh_git_now(cx);
        cx.notify();
    }

    /// All filters off (the ✕ of the filter bar).
    pub(super) fn clear_log_filters(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_mut() else { return };
        g.filter.query = LogQuery::default();
        g.filter.generation += 1;
        let field = g.filter.field.clone();
        field.update(cx, |e, cx| e.replace_all_text("", cx));
        self.refresh_git_now(cx);
        cx.notify();
    }

    /// Enter in the text field reloads now and moves to the list; Esc clears it, then leaves.
    pub(super) fn log_filter_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(g) = self.git.as_ref() else { return false };
        if !g.filter.field.read(cx).focus_handle(cx).is_focused(window) {
            return false;
        }
        let k = &e.keystroke;
        if k.modifiers.control || k.modifiers.alt {
            return false;
        }
        match k.key.as_str() {
            "enter" | "down" => {
                let focus = g.log_focus.clone();
                if let Some(g) = self.git.as_mut() {
                    g.filter.generation += 1;
                }
                self.refresh_git_now(cx);
                window.focus(&focus);
                true
            }
            "escape" => {
                if g.filter.query.text.is_empty() {
                    let focus = g.log_focus.clone();
                    window.focus(&focus);
                } else {
                    let field = g.filter.field.clone();
                    field.update(cx, |e, cx| e.replace_all_text("", cx));
                }
                cx.notify();
                true
            }
            _ => false,
        }
    }

    fn branch_filter_menu(&mut self, pos: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_ref() else { return };
        let cur = g.filter.query.branch.clone();
        let item = |label: String, value: Option<String>, on: bool| Entry::Item {
            label: if on { format!("✓ {label}") } else { format!("   {label}") },
            shortcut: String::new(),
            enabled: true,
            run: Rc::new(move |this: &mut Workspace, _, cx| this.set_log_branch(value.clone(), cx)),
        };
        let mut v = vec![item("All branches".into(), None, cur.is_none()), Entry::Separator];
        for b in g.branches.iter() {
            let name = b.name.clone();
            let on = cur.as_deref() == Some(name.as_str());
            v.push(item(if b.current { format!("{name} (current)") } else { name.clone() }, Some(name), on));
        }
        self.open_menu(pos, v, cx);
    }

    fn author_filter_menu(&mut self, pos: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        // Listing authors walks the whole history: do it off the UI thread, then open the menu.
        let Some(repo) = self.git.as_ref().map(|g| g.repo.clone()) else { return };
        let task = cx.background_executor().spawn(async move { repo.authors().unwrap_or_default() });
        cx.spawn(async move |this, cx| {
            let authors = task.await;
            let _ = this.update(cx, |this, cx| this.author_filter_menu_with(pos, authors, cx));
        })
        .detach();
    }

    fn author_filter_menu_with(&mut self, pos: gpui::Point<gpui::Pixels>, authors: Vec<String>, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_ref() else { return };
        let cur = g.filter.query.author.clone();
        let item = |label: String, value: Option<String>, on: bool| Entry::Item {
            label: if on { format!("✓ {label}") } else { format!("   {label}") },
            shortcut: String::new(),
            enabled: true,
            run: Rc::new(move |this: &mut Workspace, _, cx| this.set_log_author(value.clone(), cx)),
        };
        let mut v = vec![item("All users".into(), None, cur.is_none()), Entry::Separator];
        for a in authors.into_iter().take(40) {
            let on = cur.as_deref() == Some(a.as_str());
            v.push(item(a.clone(), Some(a), on));
        }
        self.open_menu(pos, v, cx);
    }

    /// PyCharm's Date filter: All, Last 24 hours, Last 7 days, and so on.
    pub(super) fn date_filter_menu(&mut self, pos: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_ref() else { return };
        let cur = g.filter.query.since_days;
        let v = DATES
            .iter()
            .map(|&(label, days)| Entry::Item {
                label: if cur == days { format!("✓ {label}") } else { format!("   {label}") },
                shortcut: String::new(),
                enabled: true,
                run: Rc::new(move |this: &mut Workspace, _, cx| this.set_log_since(days, cx)),
            })
            .collect();
        self.open_menu(pos, v, cx);
    }

    /// The Paths filter: the active file, its folder, the project's top-level folders (listed
    /// off the UI thread, then the menu opens).
    pub(super) fn path_filter_menu(&mut self, pos: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let Some(root) = self.git.as_ref().map(|g| g.repo.root().to_path_buf()) else { return };
        let task = cx.background_executor().spawn(async move {
            let mut dirs: Vec<String> = std::fs::read_dir(root)
                .map(|rd| {
                    rd.flatten()
                        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .filter(|n| !n.starts_with('.'))
                        .map(|n| format!("{n}/"))
                        .collect()
                })
                .unwrap_or_default();
            dirs.sort_by_key(|d| d.to_lowercase());
            dirs
        });
        cx.spawn(async move |this, cx| {
            let dirs = task.await;
            let _ = this.update(cx, |this, cx| this.path_filter_menu_with(pos, dirs, cx));
        })
        .detach();
    }

    fn path_filter_menu_with(&mut self, pos: gpui::Point<gpui::Pixels>, mut dirs: Vec<String>, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_ref() else { return };
        let cur = g.filter.query.path.clone();
        let item = |label: String, value: Option<String>| Entry::Item {
            label: if cur == value { format!("✓ {label}") } else { format!("   {label}") },
            shortcut: String::new(),
            enabled: true,
            run: Rc::new(move |this: &mut Workspace, _, cx| this.set_log_path(value.clone(), cx)),
        };
        let mut v = vec![item("All".into(), None), Entry::Separator];
        let file = self.active_editor().and_then(|e| e.read(cx).path.clone()).and_then(|p| g.repo.rel(&p));
        let mut shown: Vec<String> = Vec::new();
        if let Some(f) = file {
            if let Some((dir, _)) = f.rsplit_once('/') {
                shown.push(format!("{dir}/"));
            }
            shown.insert(0, f);
        }
        // The chosen path stays in the list even when it is none of the others.
        if let Some(c) = cur.clone().filter(|c| !shown.contains(c)) {
            shown.push(c);
        }
        let n = shown.len();
        dirs.retain(|d| !shown.contains(d));
        shown.extend(dirs.into_iter().take(30));
        for (i, p) in shown.into_iter().enumerate() {
            if i == n && i > 0 {
                v.push(Entry::Separator);
            }
            v.push(item(p.clone(), Some(p)));
        }
        self.open_menu(pos, v, cx);
    }

    /// The filter bar in the Git log header.
    pub(super) fn render_log_filters(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(g) = self.git.as_ref() else { return div().into_any_element() };
        let q = &g.filter.query;
        let focused_hint = q.text.is_empty();
        let h = crate::ui_kit::COMPACT_H;
        let field = crate::ui_kit::input_frame(false, false, h)
            .id("log-filter-text")
            .w(px(220.))
            .pl(px(6.))
            .child(crate::icons::ui_icon("search", 14., Theme::text_dim()))
            .child(crate::ui_kit::editor_slot(&g.filter.field, "Text or hash", focused_hint, h - 2.));
        let chip = |id: &'static str, label: String, active: bool| {
            div()
                .id(id)
                .h(ui_px(22.))
                .px(px(8.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .cursor_pointer()
                .text_size(ui_px(12.))
                .when(active, |d| d.bg(Theme::selection_row()).text_color(Theme::text()))
                .when(!active, |d| d.text_color(Theme::text_muted()).hover(|s| s.bg(Theme::hover_row())))
                .active(|s| s.bg(Theme::press()))
                .gap(px(3.))
                .child(crate::icons::glyph(label, 14., if active { Theme::text() } else { Theme::text_muted() }))
        };
        let branch = chip("log-filter-branch", format!("Branch: {}", q.branch.as_deref().unwrap_or("All")), q.branch.is_some())
            .child(crate::icons::ui_icon("chevron-down", 12., Theme::text_muted()))
            .relative()
            .child(track(0))
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, e: &gpui::MouseDownEvent, _, cx| this.branch_filter_menu(e.position, cx)));
        let user = chip("log-filter-user", format!("User: {}", q.author.as_deref().unwrap_or("All")), q.author.is_some())
            .child(crate::icons::ui_icon("chevron-down", 12., Theme::text_muted()))
            .relative()
            .child(track(1))
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, e: &gpui::MouseDownEvent, _, cx| this.author_filter_menu(e.position, cx)));
        let date_label = DATES.iter().find(|d| d.1 == q.since_days).map_or("All", |d| d.0);
        let date = chip("log-filter-date", format!("Date: {date_label}"), q.since_days.is_some())
            .child(crate::icons::ui_icon("chevron-down", 12., Theme::text_muted()))
            .relative()
            .child(track(2))
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, e: &gpui::MouseDownEvent, _, cx| this.date_filter_menu(e.position, cx)));
        let path_label = match q.path.as_deref() {
            None => "All".to_string(),
            // Long paths: the end of them.
            Some(p) if p.chars().count() > 28 => format!("…{}", p.chars().rev().take(27).collect::<Vec<_>>().into_iter().rev().collect::<String>()),
            Some(p) => p.to_string(),
        };
        let paths = chip("log-filter-path", format!("Paths: {path_label}"), q.path.is_some())
            .child(crate::icons::ui_icon("chevron-down", 12., Theme::text_muted()))
            .relative()
            .child(track(3))
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, e: &gpui::MouseDownEvent, _, cx| this.path_filter_menu(e.position, cx)));
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(field)
            .child(branch)
            .child(user)
            .child(date)
            .child(paths)
            .when(!q.is_empty(), |d| d.child(chip("log-filter-clear", "✕".into(), false).on_click(cx.listener(|this, _, w, cx| this.clear_log_filters(w, cx)))))
            .into_any_element()
    }
}

/// The Date filter's choices (PyCharm's list), as days back.
const DATES: [(&str, Option<u32>); 6] =
    [("All", None), ("Last 24 hours", Some(1)), ("Last 7 days", Some(7)), ("Last 30 days", Some(30)), ("Last 90 days", Some(90)), ("Last year", Some(365))];
