//! Filters of the Git log: a text field (message or hash), Branch, User, Date and Paths
//! popups. The log is reloaded through `git log` with the filters, so the graph follows the
//! filtered list.

use super::menu_ui::Entry;
use super::Workspace;
use crate::editor::Editor;
use crate::theme::ui_px;
use crate::theme::Theme;
use brisk_git::log_query::{self, LogQuery, DATES};
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
        let g = self.git(cx)?;
        Some(g.filter.field.read(cx).buffer.text().replace(['\n', '\r'], " ").trim().to_string())
    }

    fn log_filter_edited(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.log_filter_text(cx) else { return };
        let Some(mut g) = self.git_mut(cx) else { return };
        if g.filter.query.text == text {
            return;
        }
        g.filter.query.text = text;
        g.filter.generation += 1;
        let gen = g.filter.generation;
        drop(g);
        let timer = cx.background_executor().timer(Duration::from_millis(250));
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                if this.git(cx).is_some_and(|g| g.filter.generation == gen) {
                    this.refresh_git_now(cx);
                }
            });
        })
        .detach();
    }

    pub(super) fn set_log_branch(&mut self, branch: Option<String>, cx: &mut Context<Self>) {
        if let Some(mut g) = self.git_mut(cx) {
            g.filter.query.branch = branch;
        }
        self.refresh_git_now(cx);
        cx.notify();
    }

    pub(super) fn set_log_author(&mut self, author: Option<String>, cx: &mut Context<Self>) {
        if let Some(mut g) = self.git_mut(cx) {
            g.filter.query.author = author;
        }
        self.refresh_git_now(cx);
        cx.notify();
    }

    pub(super) fn set_log_since(&mut self, days: Option<u32>, cx: &mut Context<Self>) {
        if let Some(mut g) = self.git_mut(cx) {
            g.filter.query.since_days = days;
        }
        self.refresh_git_now(cx);
        cx.notify();
    }

    pub(super) fn set_log_path(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        if let Some(mut g) = self.git_mut(cx) {
            g.filter.query.path = path;
        }
        self.refresh_git_now(cx);
        cx.notify();
    }

    /// All filters off (the ✕ of the filter bar).
    pub(super) fn clear_log_filters(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut g) = self.git_mut(cx) else { return };
        g.filter.query = LogQuery::default();
        g.filter.generation += 1;
        let field = g.filter.field.clone();
        drop(g);
        field.update(cx, |e, cx| e.replace_all_text("", cx));
        self.refresh_git_now(cx);
        cx.notify();
    }

    /// Enter in the text field reloads now and moves to the list; Esc clears it, then leaves.
    pub(super) fn log_filter_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(g) = self.git(cx) else { return false };
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
                if let Some(mut g) = self.git_mut(cx) {
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
        let Some(g) = self.git(cx) else { return };
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
        let Some(repo) = self.git(cx).map(|g| g.repo.clone()) else { return };
        let task = cx.background_executor().spawn(async move { repo.authors().unwrap_or_default() });
        cx.spawn(async move |this, cx| {
            let authors = task.await;
            let _ = this.update(cx, |this, cx| this.author_filter_menu_with(pos, authors, cx));
        })
        .detach();
    }

    fn author_filter_menu_with(&mut self, pos: gpui::Point<gpui::Pixels>, authors: Vec<String>, cx: &mut Context<Self>) {
        let Some(g) = self.git(cx) else { return };
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
        let Some(g) = self.git(cx) else { return };
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
        let Some(root) = self.git(cx).map(|g| g.repo.root().to_path_buf()) else { return };
        let task = cx.background_executor().spawn(async move { log_query::top_level_dirs(&root) });
        cx.spawn(async move |this, cx| {
            let dirs = task.await;
            let _ = this.update(cx, |this, cx| this.path_filter_menu_with(pos, dirs, cx));
        })
        .detach();
    }

    fn path_filter_menu_with(&mut self, pos: gpui::Point<gpui::Pixels>, dirs: Vec<String>, cx: &mut Context<Self>) {
        let Some(g) = self.git(cx) else { return };
        let cur = g.filter.query.path.clone();
        let item = |label: String, value: Option<String>| Entry::Item {
            label: if cur == value { format!("✓ {label}") } else { format!("   {label}") },
            shortcut: String::new(),
            enabled: true,
            run: Rc::new(move |this: &mut Workspace, _, cx| this.set_log_path(value.clone(), cx)),
        };
        let mut v = vec![item("All".into(), None), Entry::Separator];
        let file = self.active_editor().and_then(|e| e.read(cx).path.clone()).and_then(|p| g.repo.rel(&p));
        let (shown, n) = log_query::path_choices(file, cur.as_deref(), dirs);
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
        let Some(g) = self.git(cx) else { return div().into_any_element() };
        let q = &g.filter.query;
        let focused_hint = q.text.is_empty();
        let h = crate::ui_kit::COMPACT_H;
        let field = crate::ui_kit::input_frame(false, false, h)
            .id("log-filter-text")
            .w(px(220.))
            // On a narrow window the field gives way first, so the chips and the
            // header's commit count and refresh stay on screen.
            .flex_shrink()
            .min_w(px(110.))
            .pl(px(6.))
            .child(crate::icons::ui_icon("search", 14., Theme::text_dim()))
            .child(crate::ui_kit::editor_slot(&g.filter.field, "Text or hash", focused_hint, h - 2.));
        let chip = |id: &'static str, label: String, active: bool| {
            div()
                .id(id)
                .flex_none()
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
        let date_label = q.date_label();
        let date = chip("log-filter-date", format!("Date: {date_label}"), q.since_days.is_some())
            .child(crate::icons::ui_icon("chevron-down", 12., Theme::text_muted()))
            .relative()
            .child(track(2))
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, e: &gpui::MouseDownEvent, _, cx| this.date_filter_menu(e.position, cx)));
        let path_label = q.path_label();
        let paths = chip("log-filter-path", format!("Paths: {path_label}"), q.path.is_some())
            .child(crate::icons::ui_icon("chevron-down", 12., Theme::text_muted()))
            .relative()
            .child(track(3))
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, e: &gpui::MouseDownEvent, _, cx| this.path_filter_menu(e.position, cx)));
        div()
            .flex()
            .flex_shrink()
            .min_w_0()
            .overflow_hidden()
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
