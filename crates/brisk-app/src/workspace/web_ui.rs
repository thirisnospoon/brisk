//! Django, FastAPI and Flask support in the editor, like PyCharm's:
//! - Ctrl+B / Ctrl+Click on a template name (`render(request, "polls/detail.html")`,
//!   `{% extends "base.html" %}`), a URL name (`reverse("polls:detail")`, `{% url %}`,
//!   `url_for()`), a URLconf in `include("polls.urls")` or a static file opens it;
//! - Go to Related Symbol (Ctrl+Alt+Home): from a template to the views that render it and
//!   the templates that extend or include it; from a view to its templates;
//! - the Endpoints tool window: every route of the project with its methods and view.
//!
//! The index (`brisk_web::Index`) is built in the background on first use and rebuilt
//! after a Python file or a template is saved.

use super::lsp_ui::NavPos;
use super::{BottomTool, Workspace};
use crate::editor::Editor;
use crate::theme::{mono_font, ui_px, Theme};
use brisk_web::{Index, Loc, Ref};
use gpui::prelude::*;
use gpui::{div, px, uniform_list, Context, Entity, FocusHandle, KeyDownEvent, ScrollStrategy, UniformListScrollHandle, Window};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Default)]
pub struct WebState {
    pub index: Option<Arc<Index>>,
    /// A file that can change routes or templates was saved since the scan.
    pub stale: bool,
    scanning: bool,
    /// Callbacks waiting for the scan in progress.
    waiting: Vec<WebCallback>,
}

/// A callback that runs with a current index.
type WebCallback = Box<dyn FnOnce(&mut Workspace, &Index, &mut Window, &mut Context<Workspace>)>;

pub struct Endpoints {
    pub focus: FocusHandle,
    pub selected: usize,
    scroll: UniformListScrollHandle,
}

impl Workspace {
    /// Runs `f` with a current index: at once when there is one, otherwise after a
    /// background scan.
    fn with_web(&mut self, window: &mut Window, cx: &mut Context<Self>, f: impl FnOnce(&mut Self, &Index, &mut Window, &mut Context<Self>) + 'static) {
        if let (Some(idx), false) = (self.web.index.clone(), self.web.stale) {
            return f(self, &idx, window, cx);
        }
        self.web.waiting.push(Box::new(f));
        self.scan_web(Some(window), cx);
    }

    /// Rescans the project (once at a time); callbacks waiting for it run when it ends.
    pub(super) fn scan_web(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        if self.web.scanning {
            return;
        }
        self.web.scanning = true;
        self.web.stale = false;
        let root = self.root.clone();
        // The project's file list when it is built: no second walk of the project.
        let files = self.files.clone();
        let task = cx.background_executor().spawn(async move {
            match files {
                Some(list) => Index::scan_files(&list),
                None => Index::scan(&root),
            }
        });
        let finish = |this: &mut Self, idx: Index| -> Arc<Index> {
            let idx = Arc::new(idx);
            this.web.index = Some(idx.clone());
            this.web.scanning = false;
            if let Some(e) = this.endpoints.as_mut() {
                e.selected = e.selected.min(idx.routes.len().saturating_sub(1));
            }
            idx
        };
        if let Some(window) = window {
            cx.spawn_in(window, async move |this, cx| {
                let idx = task.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    let idx = finish(this, idx);
                    for f in std::mem::take(&mut this.web.waiting) {
                        f(this, &idx, window, cx);
                    }
                    cx.notify();
                });
            })
            .detach();
        } else {
            cx.spawn(async move |this, cx| {
                let idx = task.await;
                let _ = this.update(cx, |this, cx| {
                    finish(this, idx);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// Saved files: Python files and templates can change routes and template folders.
    pub(super) fn web_files_saved(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let relevant = paths.iter().any(|p| {
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            matches!(ext.as_str(), "py" | "html" | "htm" | "jinja" | "jinja2" | "j2" | "txt")
        });
        if relevant && self.web.index.is_some() {
            self.web.stale = true;
            if self.bottom == Some(BottomTool::Endpoints) {
                self.scan_web(None, cx);
            }
        }
    }

    /// The editor, its file, text and caret (or `at`) as a byte offset.
    fn web_context(ed: &Entity<Editor>, at: Option<usize>, cx: &gpui::App) -> Option<(PathBuf, String, usize)> {
        let e = ed.read(cx);
        let path = e.path.clone()?;
        let rope = e.buffer.rope();
        let pos = at.unwrap_or_else(|| e.buffer.primary().head).min(rope.len_chars());
        Some((path, e.text_string(), rope.char_to_byte(pos)))
    }

    /// Ctrl+B / Ctrl+Click on a web reference; false when there is none at the place (the
    /// caller goes on with Go to Declaration).
    pub(super) fn web_goto(&mut self, ed: &Entity<Editor>, at: Option<usize>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((path, text, byte)) = Self::web_context(ed, at, cx) else { return false };
        let is_py = path.extension().is_some_and(|e| e == "py" || e == "pyi");
        let r = if is_py { brisk_web::python_ref(&text, byte) } else { brisk_web::template_ref(&text, byte) };
        let Some(r) = r else { return false };
        self.with_web(window, cx, move |this, idx, window, cx| {
            let locs = idx.resolve(&path, &r);
            let (what, name) = match &r {
                Ref::Template(n) => ("template", n),
                Ref::Url(n) => ("URL name", n),
                Ref::Module(n) => ("module", n),
                Ref::Static(n) => ("static file", n),
            };
            if locs.is_empty() {
                this.status = format!("Cannot find {what} '{name}'");
                if let Ref::Template(_) = r {
                    if idx.template_dirs.is_empty() {
                        this.status.push_str(": the project has no templates folder");
                    }
                }
            } else {
                this.go_to_locs(format!("{} '{name}'", capitalize(what)), locs, window, cx);
            }
            cx.notify();
        });
        true
    }

    /// One place: go there. Several: list them in the Find window.
    fn go_to_locs(&mut self, title: String, locs: Vec<Loc>, window: &mut Window, cx: &mut Context<Self>) {
        if let [l] = locs.as_slice() {
            self.navigate_to(NavPos { path: l.path.clone(), line: l.line, col: l.col }, window, cx);
            return;
        }
        let usages = locs.into_iter().map(|l| (l.path, l.line, l.col..l.col)).collect();
        self.show_usages(title, usages, window, cx);
    }

    /// Ctrl+Alt+Home: a template's views (and templates that extend or include it), or a
    /// view's templates.
    pub(super) fn goto_related(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.focused_editor(window, cx).or_else(|| self.active_editor().cloned()) else { return };
        let Some((path, text, byte)) = Self::web_context(&ed, None, cx) else { return };
        let is_py = path.extension().is_some_and(|e| e == "py");
        self.with_web(window, cx, move |this, idx, window, cx| {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let locs = if is_py {
                let names = brisk_web::templates_in_scope(&text, byte);
                let mut v: Vec<Loc> = names.iter().flat_map(|n| idx.resolve(&path, &Ref::Template(n.clone()))).collect();
                v.dedup();
                v
            } else {
                idx.template_usages(&path)
            };
            if locs.is_empty() {
                this.status = if is_py { "No templates used here".into() } else { format!("No views or templates use {name}") };
            } else {
                let title = if is_py { "Templates".to_string() } else { format!("Related to {name}") };
                this.go_to_locs(title, locs, window, cx);
            }
            cx.notify();
        });
    }

    // --- Endpoints tool window ------------------------------------------------------------

    pub(super) fn toggle_endpoints_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.endpoints.is_none() {
            self.endpoints = Some(Endpoints { focus: cx.focus_handle(), selected: 0, scroll: UniformListScrollHandle::new() });
        }
        self.toggle_bottom(BottomTool::Endpoints, window, cx);
        if self.bottom == Some(BottomTool::Endpoints) {
            if self.web.index.is_none() || self.web.stale {
                self.scan_web(Some(window), cx);
            }
            if let Some(e) = &self.endpoints {
                window.focus(&e.focus);
            }
        }
        cx.notify();
    }

    fn endpoint_open(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.web.index.as_ref().and_then(|x| x.routes.get(i)).cloned() else { return };
        if let Some(e) = self.endpoints.as_mut() {
            e.selected = i;
        }
        let l = r.handler_at.unwrap_or(r.at);
        self.navigate_to(NavPos { path: l.path, line: l.line, col: l.col }, window, cx);
        cx.notify();
    }

    pub(super) fn endpoints_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(ep) = &self.endpoints else { return false };
        if !ep.focus.is_focused(window) {
            return false;
        }
        let m = &e.keystroke.modifiers;
        if m.control || m.alt || m.platform {
            return false;
        }
        let n = self.web.index.as_ref().map(|x| x.routes.len()).unwrap_or(0);
        let sel = ep.selected;
        match e.keystroke.key.as_str() {
            "up" | "down" if n > 0 => {
                let ep = self.endpoints.as_mut().unwrap();
                ep.selected = if e.keystroke.key == "up" { sel.saturating_sub(1) } else { (sel + 1).min(n - 1) };
                ep.scroll.scroll_to_item(ep.selected, ScrollStrategy::Top);
            }
            "enter" => self.endpoint_open(sel, window, cx),
            "escape" => self.focus_editor(window, cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    pub(super) fn render_endpoints(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(ep) = &self.endpoints else { return div().into_any_element() };
        let n = self.web.index.as_ref().map(|x| x.routes.len()).unwrap_or(0);
        let summary = match (&self.web.index, self.web.scanning) {
            (None, _) => "Scanning…".to_string(),
            (Some(idx), _) => {
                let mut fws: Vec<&str> = idx.routes.iter().map(|r| r.framework.name()).collect();
                fws.dedup();
                format!("{n} endpoint{}{}", if n == 1 { "" } else { "s" }, if fws.is_empty() { String::new() } else { format!(" · {}", fws.join(", ")) })
            }
        };
        let toolbar = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(6.))
            .pl(px(12.))
            .pr(px(6.))
            .h(px(crate::ui_kit::header_h()))
            .border_b_1()
            .border_color(Theme::divider())
            .child(crate::ui_kit::tool_title("Endpoints"))
            .child(
                div()
                    .id("endpoints-refresh")
                    .size(ui_px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .text_color(Theme::accent())
                    .hover(|s| s.bg(Theme::hover_row()))
                    .active(|s| s.bg(Theme::press()))
                    .child("↻")
                    .tooltip(|_, cx| cx.new(|_| super::Tip("Rescan the project".into())).into())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.web.stale = true;
                        this.scan_web(Some(window), cx);
                    })),
            )
            .child(div().text_size(ui_px(12.5)).text_color(Theme::text_muted()).child(summary));
        let empty = n == 0 && self.web.index.is_some();
        let list = div()
            .id("endpoints-list")
            .track_focus(&ep.focus)
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .text_size(ui_px(13.))
            .when(empty, |d| d.child(div().p(px(10.)).text_color(Theme::text_dim()).child("No Django, FastAPI or Flask routes found in the project")))
            .when(n > 0, |d| {
                d.child(
                    uniform_list(
                        "endpoint-rows",
                        n,
                        cx.processor(|this, range: std::ops::Range<usize>, _window, cx| range.filter_map(|i| this.endpoint_row(i, cx)).collect()),
                    )
                    .flex_1()
                    .track_scroll(ep.scroll.clone()),
                )
            });
        div().id("endpoints-window").size_full().flex().flex_col().child(toolbar).child(list).into_any_element()
    }

    fn endpoint_row(&self, i: usize, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let r = self.web.index.as_ref()?.routes.get(i)?;
        let sel = self.endpoints.as_ref().is_some_and(|e| e.selected == i);
        let rel = |p: &Path| p.strip_prefix(&self.root).unwrap_or(p).to_string_lossy().replace('\\', "/");
        let methods = if r.methods.is_empty() { "ANY".to_string() } else { r.methods.join(" ") };
        let color = match r.methods.first().map(String::as_str) {
            Some("GET") | None => Theme::syntax(brisk_syntax::Highlight::String),
            Some("POST") => Theme::syntax(brisk_syntax::Highlight::Keyword),
            Some("DELETE") => Theme::error(),
            _ => Theme::syntax(brisk_syntax::Highlight::Number),
        };
        let place = r.handler_at.as_ref().unwrap_or(&r.at);
        let detail = format!("{}  {}:{}", r.name.clone().unwrap_or_else(|| r.handler.clone()), rel(&place.path), place.line + 1);
        Some(
            div()
                .id(("endpoint-row", i))
                .w_full()
                .h(ui_px(22.))
                .flex()
                .items_center()
                .gap(px(10.))
                .pl(px(12.))
                .whitespace_nowrap()
                .overflow_hidden()
                .cursor_pointer()
                .when(sel, |d| d.bg(Theme::selection_row()))
                .when(!sel, |d| d.hover(|s| s.bg(Theme::hover_row())))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _, w, cx| {
                        if let Some(e) = this.endpoints.as_mut() {
                            e.selected = i;
                            w.focus(&e.focus.clone());
                        }
                        this.endpoint_open(i, w, cx);
                    }),
                )
                .child(div().w(px(84.)).flex_shrink_0().text_size(ui_px(11.5)).font_family(mono_font()).text_color(color).child(methods))
                .child(div().font_family(mono_font()).text_color(Theme::text()).child(r.path.clone()))
                .child(div().text_size(ui_px(12.)).text_color(Theme::text_dim()).child(detail))
                .into_any_element(),
        )
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}
