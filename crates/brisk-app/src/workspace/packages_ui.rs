//! Python Packages tool window: the project interpreter's packages (pip, or uv for an
//! interpreter without pip), a filter, Install by name and version (names suggested from
//! PyPI as you type), Uninstall after a confirmation, Upgrade for outdated packages
//! (`pip list --outdated` in the background). Operations queue up and run one at a time in
//! the background; the package's row shows a spinner meanwhile.

use crate::theme::ui_px;
use super::{BottomTool, Workspace};
use crate::editor::Editor;
use crate::py_packages::{self, Package, Tool};
use crate::theme::{Theme, mono_font};
use brisk_keymap::Action;
use gpui::prelude::*;
use gpui::{div, px, Animation, AnimationExt, Context, Entity, FocusHandle, Focusable, KeyDownEvent, ScrollHandle, Transformation, Window};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

/// One install / uninstall / upgrade, queued until the previous one finishes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Install(String, String),
    Uninstall(Package),
    Upgrade(String),
}

impl Op {
    pub fn name(&self) -> &str {
        match self {
            Op::Install(n, _) | Op::Upgrade(n) => n,
            Op::Uninstall(p) => &p.name,
        }
    }

    fn label(&self) -> String {
        match self {
            Op::Install(n, v) => format!("Installing {}…", py_packages::requirement(n, v)),
            Op::Uninstall(p) => format!("Uninstalling {} {}…", p.name, p.version),
            Op::Upgrade(n) => format!("Upgrading {n}…"),
        }
    }

    /// The word shown in the package's row while it runs.
    fn verb(&self) -> &'static str {
        match self {
            Op::Install(..) => "Installing…",
            Op::Uninstall(_) => "Uninstalling…",
            Op::Upgrade(_) => "Upgrading…",
        }
    }
}

pub struct Packages {
    pub filter: Entity<Editor>,
    pub name: Entity<Editor>,
    pub version: Entity<Editor>,
    pub focus: FocusHandle,
    pub python: Option<PathBuf>,
    pub tool: Option<Tool>,
    pub list: Vec<Package>,
    /// Index into the filtered list.
    pub selected: usize,
    pub loading: bool,
    /// The running operation's text ("Installing requests…").
    pub busy: Option<String>,
    pub running: Option<Op>,
    pub queue: VecDeque<Op>,
    /// The package waiting for the Uninstall confirmation.
    pub confirm: Option<Package>,
    pub error: Option<String>,
    /// Normalized name → newer version on the index.
    pub outdated: HashMap<String, String>,
    pub outdated_loading: bool,
    /// PyPI's project names (normalized, original), loaded the first time a name is typed.
    index: Option<Arc<Vec<(String, String)>>>,
    pub index_loading: bool,
    index_error: Option<String>,
    /// Names for the Package name field, and the highlighted one.
    pub suggestions: Vec<String>,
    pub suggestion: Option<usize>,
    /// The name the suggestions were computed for (or picked).
    suggested_for: String,
    scroll: ScrollHandle,
}

fn field(cx: &mut Context<Workspace>) -> Entity<Editor> {
    cx.new(|cx| {
        let mut e = Editor::scratch("", cx);
        e.buffer.auto_pairs = false;
        e.is_field = true;
        e
    })
}

impl Packages {
    fn new(cx: &mut Context<Workspace>) -> Self {
        let filter = field(cx);
        cx.observe(&filter, |this: &mut Workspace, _, cx| {
            if let Some(p) = this.packages.as_mut() {
                p.selected = 0;
            }
            cx.notify();
        })
        .detach();
        let name = field(cx);
        cx.observe(&name, |this: &mut Workspace, _, cx| this.update_package_suggestions(cx)).detach();
        Self {
            filter,
            name,
            version: field(cx),
            focus: cx.focus_handle(),
            python: None,
            tool: None,
            list: Vec::new(),
            selected: 0,
            loading: false,
            busy: None,
            running: None,
            queue: VecDeque::new(),
            confirm: None,
            error: None,
            outdated: HashMap::new(),
            outdated_loading: false,
            index: None,
            index_loading: false,
            index_error: None,
            suggestions: Vec::new(),
            suggestion: None,
            suggested_for: String::new(),
            scroll: ScrollHandle::new(),
        }
    }

    pub fn fields(&self) -> [Entity<Editor>; 3] {
        [self.filter.clone(), self.name.clone(), self.version.clone()]
    }

    pub fn visible(&self, cx: &gpui::App) -> Vec<Package> {
        let q = self.filter.read(cx).text_string().trim().to_lowercase();
        self.list.iter().filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q)).cloned().collect()
    }

    /// The newer version of `name`, if the index has one.
    pub fn latest(&self, name: &str) -> Option<&String> {
        self.outdated.get(&py_packages::normalize(name))
    }

    /// What is happening to `name`: running (`true`) or queued (`false`).
    pub fn op_on(&self, name: &str) -> Option<(&Op, bool)> {
        let k = py_packages::normalize(name);
        let same = |o: &&Op| py_packages::normalize(o.name()) == k;
        self.running.as_ref().filter(same).map(|o| (o, true)).or_else(|| self.queue.iter().find(same).map(|o| (o, false)))
    }

    /// Installs of packages that are not in the list yet (shown as rows above it).
    pub fn pending_new(&self) -> Vec<(&Op, bool)> {
        let known = |n: &str| self.list.iter().any(|p| py_packages::normalize(&p.name) == py_packages::normalize(n));
        self.running.iter().map(|o| (o, true)).chain(self.queue.iter().map(|o| (o, false))).filter(|(o, _)| matches!(o, Op::Install(..)) && !known(o.name())).collect()
    }
}

fn text(e: &Entity<Editor>, cx: &gpui::App) -> String {
    e.read(cx).text_string().replace(['\n', '\r'], " ").trim().to_string()
}

/// A turning loader icon.
fn spinner(id: impl Into<gpui::ElementId>) -> impl IntoElement {
    crate::icons::ui_icon("loader-circle", 13., Theme::text_muted())
        .with_animation(id, Animation::new(std::time::Duration::from_millis(900)).repeat(), |svg, t| svg.with_transformation(Transformation::rotate(gpui::percentage(t))))
}

impl Workspace {
    pub(super) fn toggle_python_packages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.packages.is_none() {
            self.packages = Some(Packages::new(cx));
        }
        self.toggle_bottom(BottomTool::Packages, window, cx);
        if self.bottom == Some(BottomTool::Packages) {
            let python = self.runs.interpreter(&self.root).map(|i| i.path);
            let stale = self.packages.as_ref().is_some_and(|p| p.list.is_empty() || p.python != python);
            if stale {
                self.refresh_packages(cx);
            }
            if let Some(p) = &self.packages {
                window.focus(&p.filter.read(cx).focus_handle(cx));
            }
        }
        cx.notify();
    }

    pub(super) fn refresh_packages(&mut self, cx: &mut Context<Self>) {
        let python = self.runs.interpreter(&self.root).map(|i| i.path);
        let Some(p) = self.packages.as_mut() else { return };
        let Some(py) = python.clone() else {
            p.error = Some("No Python interpreter: set one for the project to manage its packages.".into());
            return;
        };
        if p.python != python {
            p.tool = None;
            p.list.clear();
            p.outdated.clear();
        }
        p.python = python;
        p.loading = true;
        p.error = None;
        let tool = p.tool.clone();
        let task = cx.background_executor().spawn(async move {
            let tool = tool.unwrap_or_else(|| py_packages::tool_for(&py));
            let list = py_packages::list(&py, &tool);
            (tool, list)
        });
        cx.spawn(async move |this, cx| {
            let (tool, list) = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(p) = this.packages.as_mut() else { return };
                p.loading = false;
                p.tool = Some(tool);
                let ok = list.is_ok();
                match list {
                    Ok(l) => p.list = l,
                    Err(e) => p.error = Some(e),
                }
                p.selected = p.selected.min(p.list.len().saturating_sub(1));
                if ok {
                    this.check_outdated(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// `pip list --outdated` in the background (it asks the index: seconds, or nothing
    /// offline); the Version column then shows "1.0 → 1.2" and Upgrade.
    fn check_outdated(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.packages.as_mut() else { return };
        let (Some(py), Some(tool)) = (p.python.clone(), p.tool.clone()) else { return };
        if p.outdated_loading {
            return;
        }
        p.outdated_loading = true;
        let task = cx.background_executor().spawn(async move { py_packages::outdated(&py, &tool) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(p) = this.packages.as_mut() {
                    p.outdated_loading = false;
                    // Offline or no index: no upgrade information, nothing to report.
                    if let Ok(o) = res {
                        p.outdated = o;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Queue an operation; it starts at once when nothing else runs.
    pub(super) fn package_op(&mut self, op: Op, cx: &mut Context<Self>) {
        let Some(p) = self.packages.as_mut() else { return };
        if p.op_on(op.name()).is_some_and(|(o, _)| *o == op) {
            return;
        }
        p.error = None;
        p.queue.push_back(op);
        self.start_next_package_op(cx);
    }

    fn start_next_package_op(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.packages.as_mut() else { return };
        if p.running.is_some() {
            return;
        }
        let Some(op) = p.queue.pop_front() else { return };
        let (Some(py), Some(tool)) = (p.python.clone(), p.tool.clone()) else {
            p.queue.clear();
            return;
        };
        let label = op.label();
        p.busy = Some(label.clone());
        p.running = Some(op.clone());
        self.status = label;
        let task = cx.background_executor().spawn({
            let op = op.clone();
            async move {
                match &op {
                    Op::Install(n, v) => py_packages::install(&py, &tool, n, v).map(|_| format!("Installed {}", py_packages::requirement(n, v))),
                    Op::Uninstall(pk) => py_packages::uninstall(&py, &tool, &pk.name).map(|_| format!("Uninstalled {}", pk.name)),
                    Op::Upgrade(n) => py_packages::upgrade(&py, &tool, n).map(|_| format!("Upgraded {n}")),
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                let ok = res.is_ok();
                let msg = res.unwrap_or_else(|e| format!("Failed: {e}"));
                let mut more = false;
                if let Some(p) = this.packages.as_mut() {
                    p.busy = None;
                    p.running = None;
                    more = !p.queue.is_empty();
                    match (&op, ok) {
                        (Op::Install(n, v), true) => {
                            // Clear the fields if they still hold what was installed.
                            if text(&p.name, cx) == *n && text(&p.version, cx) == *v {
                                p.name.update(cx, |e, cx| e.replace_all_text("", cx));
                                p.version.update(cx, |e, cx| e.replace_all_text("", cx));
                            }
                        }
                        (Op::Uninstall(pk), true) => p.list.retain(|x| x.name != pk.name),
                        (Op::Upgrade(n), true) => {
                            p.outdated.remove(&py_packages::normalize(n));
                        }
                        (_, false) => p.error = Some(msg.clone()),
                    }
                }
                this.status = msg.clone();
                this.show_toast(msg, !ok, cx);
                if more {
                    this.start_next_package_op(cx);
                } else {
                    this.refresh_packages(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn install_package(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &self.packages else { return };
        let (name, version) = (text(&p.name, cx), text(&p.version, cx));
        if name.is_empty() {
            if let Some(p) = self.packages.as_mut() {
                p.error = Some("Type a package name to install.".into());
            }
            return;
        }
        if let Some(p) = self.packages.as_mut() {
            p.suggestions.clear();
            p.suggestion = None;
        }
        self.package_op(Op::Install(name, version), cx);
    }

    fn upgrade_selected(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &self.packages else { return };
        if let Some(pk) = p.visible(cx).get(p.selected).filter(|pk| p.latest(&pk.name).is_some()) {
            let name = pk.name.clone();
            self.package_op(Op::Upgrade(name), cx);
        }
    }

    fn ask_uninstall(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &self.packages else { return };
        let pk = p.visible(cx).get(p.selected).cloned();
        if let (Some(p), Some(pk)) = (self.packages.as_mut(), pk) {
            if p.op_on(&pk.name).is_none() {
                p.confirm = Some(pk);
            }
        }
    }

    fn confirm_uninstall(&mut self, yes: bool, cx: &mut Context<Self>) {
        let Some(pk) = self.packages.as_mut().and_then(|p| p.confirm.take()) else { return };
        if yes {
            self.package_op(Op::Uninstall(pk), cx);
        }
    }

    /// The Package name field changed: suggest PyPI names (the list loads on first use).
    fn update_package_suggestions(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.packages.as_mut() else { return };
        let q = text(&p.name, cx);
        if q == p.suggested_for {
            return;
        }
        p.suggested_for = q.clone();
        p.suggestion = None;
        if q.is_empty() {
            p.suggestions.clear();
            cx.notify();
            return;
        }
        match p.index.clone() {
            // A pass over a million names: off the UI thread, the newest query wins.
            Some(index) => {
                let task = cx.background_executor().spawn({
                    let q = q.clone();
                    async move { py_packages::suggest_in(&index, &q, 12) }
                });
                cx.spawn(async move |this, cx| {
                    let found = task.await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(p) = this.packages.as_mut().filter(|p| p.suggested_for == q) {
                            p.suggestions = found;
                            p.suggestion = None;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            None if !p.index_loading && p.index_error.is_none() => self.load_package_index(cx),
            None => {}
        }
        cx.notify();
    }

    fn load_package_index(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.packages.as_mut() else { return };
        let Some(py) = p.python.clone() else { return };
        p.index_loading = true;
        let task = cx.background_executor().spawn(async move { py_packages::index_names(&py).map(py_packages::name_index) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(p) = this.packages.as_mut() else { return };
                p.index_loading = false;
                match res {
                    Ok(names) => p.index = Some(Arc::new(names)),
                    Err(e) => p.index_error = Some(e),
                }
                p.suggested_for.clear();
                this.update_package_suggestions(cx);
            });
        })
        .detach();
    }

    /// Put a suggested name into the field (the version field gets the focus).
    pub(super) fn pick_package_suggestion(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.packages.as_mut() else { return };
        let Some(name) = p.suggestions.get(i).cloned() else { return };
        p.suggested_for = name.clone();
        p.suggestions.clear();
        p.suggestion = None;
        let (field, version) = (p.name.clone(), p.version.clone());
        field.update(cx, |e, cx| e.replace_all_text(&name, cx));
        window.focus(&version.read(cx).focus_handle(cx));
        cx.notify();
    }

    pub(super) fn python_packages_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            Action::RefreshPythonPackages => self.refresh_packages(cx),
            Action::InstallPythonPackage => {
                if self.bottom != Some(BottomTool::Packages) {
                    self.toggle_python_packages(window, cx);
                }
                if let Some(p) = &self.packages {
                    window.focus(&p.name.read(cx).focus_handle(cx));
                }
            }
            Action::UninstallPythonPackage => self.ask_uninstall(cx),
            _ => {}
        }
        cx.notify();
    }

    /// Keys in the window's fields and list; true when handled.
    pub(super) fn packages_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(p) = &self.packages else { return false };
        let k = &e.keystroke;
        let m = &k.modifiers;
        let plain = !(m.control || m.alt || m.shift || m.platform);
        let in_filter = p.filter.read(cx).focus_handle(cx).is_focused(window);
        let in_name = p.name.read(cx).focus_handle(cx).is_focused(window);
        let in_install = in_name || p.version.read(cx).focus_handle(cx).is_focused(window);
        let in_list = p.focus.is_focused(window);
        if !(in_filter || in_install || in_list) {
            return false;
        }
        if p.confirm.is_some() {
            match k.key.as_str() {
                "enter" => self.confirm_uninstall(true, cx),
                "escape" => self.confirm_uninstall(false, cx),
                _ => {}
            }
            cx.notify();
            return true;
        }
        let n = p.visible(cx).len();
        let s = p.suggestions.len();
        match k.key.as_str() {
            "up" | "down" if plain && in_name && s > 0 => {
                let p = self.packages.as_mut().unwrap();
                p.suggestion = Some(match (p.suggestion, k.key == "up") {
                    (None, true) => s - 1,
                    (None, false) => 0,
                    (Some(i), true) => (i + s - 1) % s,
                    (Some(i), false) => (i + 1) % s,
                });
            }
            "enter" if plain && in_name && p.suggestion.is_some() => {
                let i = p.suggestion.unwrap_or(0);
                self.pick_package_suggestion(i, window, cx);
            }
            "escape" if plain && in_name && s > 0 => {
                let p = self.packages.as_mut().unwrap();
                p.suggestions.clear();
                p.suggestion = None;
            }
            "enter" if plain && in_install => self.install_package(cx),
            "tab" if plain && in_install => {
                let next = if in_name { p.version.clone() } else { p.name.clone() };
                if let Some(p) = self.packages.as_mut() {
                    p.suggestions.clear();
                }
                window.focus(&next.read(cx).focus_handle(cx));
            }
            "up" | "down" if plain && (in_filter || in_list) && n > 0 => {
                let p = self.packages.as_mut().unwrap();
                p.selected = if k.key == "up" { p.selected.saturating_sub(1) } else { (p.selected + 1).min(n - 1) };
                p.scroll.scroll_to_item(p.selected);
            }
            "delete" if plain && in_list => self.ask_uninstall(cx),
            "escape" if plain => self.focus_editor(window, cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    pub(super) fn render_python_packages(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(p) = &self.packages else { return div().into_any_element() };
        let visible = p.visible(cx);
        use crate::ui_kit::{self, CONTROL_H};
        let input = |e: &Entity<Editor>, w: f32, hint: &'static str, cx: &gpui::App| {
            let focused = e.read(cx).focus_handle(cx).is_focused(window);
            ui_kit::editor_field(e, hint, focused, false, CONTROL_H, cx).w(px(w))
        };
        let tool = p.tool.as_ref().map(|t| t.label()).unwrap_or("pip");
        let interp = p.python.as_ref().map(|x| x.display().to_string()).unwrap_or_else(|| "<No interpreter>".into());
        let selected = p.selected.min(visible.len().saturating_sub(1));
        let sel_pkg = visible.get(selected);
        let can_uninstall = sel_pkg.is_some_and(|pk| p.op_on(&pk.name).is_none());
        let can_upgrade = sel_pkg.is_some_and(|pk| p.latest(&pk.name).is_some() && p.op_on(&pk.name).is_none());
        // Suggestions under the Package name field.
        let name_text = text(&p.name, cx);
        let name_focused = p.name.read(cx).focus_handle(cx).is_focused(window);
        let note = if p.index_loading {
            Some("Loading the PyPI package list…".to_string())
        } else {
            p.index_error.clone()
        };
        let show_popup = name_focused && !name_text.is_empty() && p.suggested_for == name_text && (!p.suggestions.is_empty() || note.is_some());
        let popup: Option<gpui::AnyElement> = show_popup.then(|| {
            let rows = p.suggestions.iter().enumerate().map(|(i, n)| {
                let sel = p.suggestion == Some(i);
                div()
                    .id(("pkg-suggest", i))
                    .w_full()
                    .h(ui_px(22.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .when(sel, |d| d.bg(Theme::selection_row()))
                    .when(!sel, |d| d.hover(|s| s.bg(Theme::hover_row())))
                    .child(n.clone())
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, w, cx| {
                        cx.stop_propagation();
                        this.pick_package_suggestion(i, w, cx);
                    }))
            });
            gpui::deferred(
                div()
                    .id("pkg-suggestions")
                    .absolute()
                    .top(px(CONTROL_H + 2.))
                    .left_0()
                    .w(px(260.))
                    .py(px(4.))
                    .bg(Theme::panel())
                    .border_1()
                    .border_color(Theme::divider())
                    .rounded(px(6.))
                    .shadow_md()
                    .text_size(ui_px(13.))
                    .when_some(note.clone(), |d, n| d.child(div().px(px(8.)).py(px(3.)).text_size(ui_px(12.)).text_color(Theme::text_muted()).child(n)))
                    .children(rows),
            )
            .with_priority(1)
            .into_any_element()
        });
        let toolbar = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .h(ui_px(CONTROL_H + 10.))
            .border_b_1()
            .border_color(Theme::divider())
            .child(input(&p.filter, 200., "Filter packages", cx))
            .child(
                ui_kit::icon_button("pkg-refresh", "rotate-cw", !p.loading)
                    .tooltip(|_, cx| cx.new(|_| super::Tip("Reload the list".into())).into())
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_packages(cx))),
            )
            .child(div().w(px(1.)).h(ui_px(18.)).mx(px(2.)).bg(Theme::divider()))
            .child(div().relative().child(input(&p.name, 180., "Package name", cx)).children(popup))
            .child(input(&p.version, 160., "Version", cx))
            .child(ui_kit::button("pkg-install", "Install", true, true).on_click(cx.listener(|this, _, _, cx| {
                this.install_package(cx);
                cx.notify();
            })))
            .child(ui_kit::button("pkg-uninstall", "Uninstall", can_uninstall, false).on_click(cx.listener(move |this, _, _, cx| {
                if can_uninstall {
                    this.ask_uninstall(cx);
                    cx.notify();
                }
            })))
            .child(ui_kit::button("pkg-upgrade", "Upgrade", can_upgrade, false).on_click(cx.listener(move |this, _, _, cx| {
                if can_upgrade {
                    this.upgrade_selected(cx);
                    cx.notify();
                }
            })))
            .child(div().flex_1().min_w_0().overflow_hidden().text_size(ui_px(12.)).text_color(Theme::text_muted()).whitespace_nowrap().text_ellipsis().child(format!("{tool} · {interp}")));
        let notice: Option<gpui::AnyElement> = if let Some(pk) = &p.confirm {
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .h(px(crate::ui_kit::header_h()))
                    .bg(Theme::panel())
                    .border_b_1()
                    .border_color(Theme::divider())
                    .child(crate::icons::ui_icon("triangle-alert", 14., Theme::warning()))
                    .child(format!("Uninstall {} {} from this interpreter?", pk.name, pk.version))
                    .child(ui_kit::button("pkg-confirm", "Uninstall", true, true).on_click(cx.listener(|this, _, _, cx| {
                        this.confirm_uninstall(true, cx);
                        cx.notify();
                    })))
                    .child(ui_kit::button("pkg-cancel", "Cancel", true, false).on_click(cx.listener(|this, _, _, cx| {
                        this.confirm_uninstall(false, cx);
                        cx.notify();
                    })))
                    .child(div().text_size(ui_px(12.)).text_color(Theme::text_dim()).child("Enter — uninstall, Esc — cancel"))
                    .into_any_element(),
            )
        } else {
            p.error.clone().map(|e| div().px(px(10.)).h(ui_px(24.)).flex().items_center().text_size(ui_px(12.5)).text_color(Theme::error()).border_b_1().border_color(Theme::divider()).child(e).into_any_element())
        };
        // Right of the version: a spinner and what runs, or "Queued".
        let op_state = |op: &Op, running: bool, id: (&'static str, usize)| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(ui_px(12.))
                .text_color(if running { Theme::text_muted() } else { Theme::text_dim() })
                .when(running, |d| d.child(spinner(id)).child(op.verb()))
                .when(!running, |d| d.child("Queued"))
        };
        let pending = p.pending_new();
        let pending_rows: Vec<gpui::AnyElement> = pending
            .iter()
            .enumerate()
            .map(|(i, (op, running))| {
                let version = match op {
                    Op::Install(_, v) if !v.is_empty() => v.clone(),
                    _ => "latest".into(),
                };
                div()
                    .id(("pkg-new", i))
                    .w_full()
                    .h(ui_px(22.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .child(div().w(px(320.)).child(op.name().to_string()))
                    .child(div().w(px(200.)).font_family(mono_font()).text_size(ui_px(12.5)).text_color(Theme::text_muted()).child(version))
                    .child(op_state(op, *running, ("pkg-new-spin", i)))
                    .into_any_element()
            })
            .collect();
        let rows = div()
            .id("pkg-list")
            .track_focus(&p.focus)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&p.scroll)
            .text_size(ui_px(13.))
            .children(pending_rows)
            .when(visible.is_empty() && pending.is_empty(), |d| {
                let msg = if p.loading { "Loading packages…" } else if p.list.is_empty() { "No packages" } else { "No packages match the filter" };
                d.child(div().p(px(10.)).text_color(Theme::text_dim()).child(msg))
            })
            .children(visible.iter().enumerate().map(|(i, pk)| {
                let sel = i == selected;
                let latest = p.latest(&pk.name).cloned();
                let busy_row = p.op_on(&pk.name).map(|(op, running)| op_state(op, running, ("pkg-spin", i)));
                let can_up = latest.is_some() && busy_row.is_none();
                div()
                    .id(("pkg-row", i))
                    .group("pkg-row")
                    .w_full()
                    .h(ui_px(22.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .when(sel, |d| d.bg(Theme::selection_row()))
                    .when(!sel, |d| d.hover(|s| s.bg(Theme::hover_row())))
                    .child(div().w(px(320.)).child(pk.name.clone()))
                    .child(
                        div()
                            .w(px(200.))
                            .flex()
                            .gap(px(6.))
                            .font_family(mono_font())
                            .text_size(ui_px(12.5))
                            .text_color(Theme::text_muted())
                            .child(pk.version.clone())
                            .when_some(latest, |d, l| d.child(div().text_color(Theme::accent()).child(format!("→ {l}")))),
                    )
                    .children(busy_row)
                    .when(can_up, |d| {
                        let name = pk.name.clone();
                        d.child(
                            div().invisible().when(sel, |d| d.visible()).group_hover("pkg-row", |s| s.visible()).child(
                                ui_kit::button("pkg-row-upgrade", "Upgrade", true, false).h(ui_px(18.)).on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.package_op(Op::Upgrade(name.clone()), cx);
                                    cx.notify();
                                })),
                            ),
                        )
                    })
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, w, cx| {
                        if let Some(p) = this.packages.as_mut() {
                            p.selected = i;
                            w.focus(&p.focus.clone());
                        }
                        cx.notify();
                    }))
            }));
        let busy_note = match (&p.busy, p.queue.len()) {
            (Some(b), 0) => b.clone(),
            (Some(b), q) => format!("{b} ({q} more queued)"),
            (None, _) if p.outdated_loading => "Checking for updates…".into(),
            _ => String::new(),
        };
        let header = div()
            .flex()
            .flex_shrink_0()
            .px(px(10.))
            .h(ui_px(22.))
            .items_center()
            .text_size(ui_px(12.))
            .text_color(Theme::text_muted())
            .border_b_1()
            .border_color(Theme::divider())
            .child(div().w(px(320.)).child(format!("Package ({})", visible.len())))
            .child(div().w(px(200.)).child("Version"))
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(busy_note));
        div()
            .id("pkg-window")
            .size_full()
            .flex()
            .flex_col()
            .text_size(ui_px(13.))
            .child(toolbar)
            .children(notice)
            .child(header)
            .child(rows)
            .into_any_element()
    }
}
