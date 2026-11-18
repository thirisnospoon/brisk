//! Python Packages tool window: the project interpreter's packages (pip, or uv for an
//! interpreter without pip), a filter, Install by name and version, Uninstall after a
//! confirmation. pip runs in the background; progress shows in the status bar.

use super::{BottomTool, Workspace};
use crate::editor::Editor;
use crate::py_packages::{self, Package, Tool};
use crate::theme::{Theme, mono_font};
use brisk_keymap::Action;
use gpui::prelude::*;
use gpui::{div, px, Context, Entity, FocusHandle, Focusable, KeyDownEvent, ScrollHandle, Window};
use std::path::PathBuf;

pub struct Packages {
    pub filter: Entity<Editor>,
    pub name: Entity<Editor>,
    pub version: Entity<Editor>,
    pub focus: FocusHandle,
    python: Option<PathBuf>,
    tool: Option<Tool>,
    pub list: Vec<Package>,
    /// Index into the filtered list.
    pub selected: usize,
    pub loading: bool,
    /// The running install / uninstall ("Installing requests…").
    pub busy: Option<String>,
    /// The package waiting for the Uninstall confirmation.
    pub confirm: Option<Package>,
    pub error: Option<String>,
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
        Self {
            filter,
            name: field(cx),
            version: field(cx),
            focus: cx.focus_handle(),
            python: None,
            tool: None,
            list: Vec::new(),
            selected: 0,
            loading: false,
            busy: None,
            confirm: None,
            error: None,
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
}

fn text(e: &Entity<Editor>, cx: &gpui::App) -> String {
    e.read(cx).text_string().replace(['\n', '\r'], " ").trim().to_string()
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
                match list {
                    Ok(l) => p.list = l,
                    Err(e) => p.error = Some(e),
                }
                p.selected = p.selected.min(p.list.len().saturating_sub(1));
                cx.notify();
            });
        })
        .detach();
    }

    /// Install (`uninstall`: None) or uninstall a package in the background.
    fn package_op(&mut self, install: Option<(String, String)>, uninstall: Option<Package>, cx: &mut Context<Self>) {
        let Some(p) = self.packages.as_mut() else { return };
        if p.busy.is_some() {
            return;
        }
        let (Some(py), Some(tool)) = (p.python.clone(), p.tool.clone()) else { return };
        let label = match (&install, &uninstall) {
            (Some((n, v)), _) => format!("Installing {}…", py_packages::requirement(n, v)),
            (_, Some(pk)) => format!("Uninstalling {} {}…", pk.name, pk.version),
            _ => return,
        };
        let was_install = install.is_some();
        p.busy = Some(label.clone());
        p.error = None;
        self.status = label;
        let task = cx.background_executor().spawn(async move {
            match (&install, &uninstall) {
                (Some((n, v)), _) => py_packages::install(&py, &tool, n, v).map(|_| format!("Installed {}", py_packages::requirement(n, v))),
                (_, Some(pk)) => py_packages::uninstall(&py, &tool, &pk.name).map(|_| format!("Uninstalled {}", pk.name)),
                _ => Ok(String::new()),
            }
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                let ok = res.is_ok();
                let msg = res.unwrap_or_else(|e| format!("Failed: {e}"));
                if let Some(p) = this.packages.as_mut() {
                    p.busy = None;
                    if ok && was_install {
                        p.name.update(cx, |e, cx| e.replace_all_text("", cx));
                        p.version.update(cx, |e, cx| e.replace_all_text("", cx));
                    } else if !ok {
                        p.error = Some(msg.clone());
                    }
                }
                this.status = msg.clone();
                this.show_toast(msg, !ok, cx);
                this.refresh_packages(cx);
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
        self.package_op(Some((name, version)), None, cx);
    }

    fn ask_uninstall(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &self.packages else { return };
        let pk = p.visible(cx).get(p.selected).cloned();
        if let (Some(p), Some(pk)) = (self.packages.as_mut(), pk) {
            if p.busy.is_none() {
                p.confirm = Some(pk);
            }
        }
    }

    fn confirm_uninstall(&mut self, yes: bool, cx: &mut Context<Self>) {
        let Some(pk) = self.packages.as_mut().and_then(|p| p.confirm.take()) else { return };
        if yes {
            self.package_op(None, Some(pk), cx);
        }
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
        let in_install = p.name.read(cx).focus_handle(cx).is_focused(window) || p.version.read(cx).focus_handle(cx).is_focused(window);
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
        match k.key.as_str() {
            "enter" if plain && in_install => self.install_package(cx),
            "tab" if plain && in_install => {
                let next = if p.name.read(cx).focus_handle(cx).is_focused(window) { p.version.clone() } else { p.name.clone() };
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
        let busy = p.busy.is_some();
        use crate::ui_kit::{self, CONTROL_H};
        let input = |e: &Entity<Editor>, w: f32, hint: &'static str, cx: &gpui::App| {
            let focused = e.read(cx).focus_handle(cx).is_focused(window);
            ui_kit::editor_field(e, hint, focused, false, CONTROL_H, cx).w(px(w))
        };
        let tool = p.tool.as_ref().map(|t| t.label()).unwrap_or("pip");
        let interp = p.python.as_ref().map(|x| x.display().to_string()).unwrap_or_else(|| "<No interpreter>".into());
        let can_uninstall = !busy && !visible.is_empty();
        let toolbar = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .h(px(CONTROL_H + 10.))
            .border_b_1()
            .border_color(Theme::divider())
            .child(input(&p.filter, 200., "Filter packages", cx))
            .child(
                ui_kit::icon_button("pkg-refresh", "rotate-cw", !p.loading && !busy)
                    .tooltip(|_, cx| cx.new(|_| super::Tip("Reload the list".into())).into())
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_packages(cx))),
            )
            .child(div().w(px(1.)).h(px(18.)).mx(px(2.)).bg(Theme::divider()))
            .child(input(&p.name, 180., "Package name", cx))
            .child(input(&p.version, 160., "Version", cx))
            .child(ui_kit::button("pkg-install", "Install", !busy, true).on_click(cx.listener(|this, _, _, cx| {
                this.install_package(cx);
                cx.notify();
            })))
            .child(ui_kit::button("pkg-uninstall", "Uninstall", can_uninstall, false).on_click(cx.listener(move |this, _, _, cx| {
                if can_uninstall {
                    this.ask_uninstall(cx);
                    cx.notify();
                }
            })))
            .child(div().flex_1().min_w_0().overflow_hidden().text_size(px(12.)).text_color(Theme::text_muted()).whitespace_nowrap().text_ellipsis().child(format!("{tool} · {interp}")));
        let notice: Option<gpui::AnyElement> = if let Some(pk) = &p.confirm {
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .h(px(34.))
                    .bg(Theme::panel())
                    .border_b_1()
                    .border_color(Theme::divider())
                    .child(div().text_color(Theme::warning()).child("⚠"))
                    .child(format!("Uninstall {} {} from this interpreter?", pk.name, pk.version))
                    .child(ui_kit::button("pkg-confirm", "Uninstall", true, true).on_click(cx.listener(|this, _, _, cx| {
                        this.confirm_uninstall(true, cx);
                        cx.notify();
                    })))
                    .child(ui_kit::button("pkg-cancel", "Cancel", true, false).on_click(cx.listener(|this, _, _, cx| {
                        this.confirm_uninstall(false, cx);
                        cx.notify();
                    })))
                    .child(div().text_size(px(12.)).text_color(Theme::text_dim()).child("Enter — uninstall, Esc — cancel"))
                    .into_any_element(),
            )
        } else {
            let line = p.busy.clone().map(|b| (b, Theme::text_muted())).or_else(|| p.error.clone().map(|e| (e, Theme::error())));
            line.map(|(t, c)| div().px(px(10.)).h(px(24.)).flex().items_center().text_size(px(12.5)).text_color(c).border_b_1().border_color(Theme::divider()).child(t).into_any_element())
        };
        let selected = p.selected.min(visible.len().saturating_sub(1));
        let rows = div()
            .id("pkg-list")
            .track_focus(&p.focus)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&p.scroll)
            .text_size(px(13.))
            .when(visible.is_empty(), |d| {
                let msg = if p.loading { "Loading packages…" } else if p.list.is_empty() { "No packages" } else { "No packages match the filter" };
                d.child(div().p(px(10.)).text_color(Theme::text_dim()).child(msg))
            })
            .children(visible.iter().enumerate().map(|(i, pk)| {
                let sel = i == selected;
                div()
                    .id(("pkg-row", i))
                    .w_full()
                    .h(px(22.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .when(sel, |d| d.bg(Theme::selection_row()))
                    .when(!sel, |d| d.hover(|s| s.bg(Theme::hover_row())))
                    .child(div().w(px(320.)).child(pk.name.clone()))
                    .child(div().font_family(mono_font()).text_size(px(12.5)).text_color(Theme::text_muted()).child(pk.version.clone()))
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, w, cx| {
                        if let Some(p) = this.packages.as_mut() {
                            p.selected = i;
                            w.focus(&p.focus.clone());
                        }
                        cx.notify();
                    }))
            }));
        let header = div()
            .flex()
            .flex_shrink_0()
            .px(px(10.))
            .h(px(22.))
            .items_center()
            .text_size(px(12.))
            .text_color(Theme::text_muted())
            .border_b_1()
            .border_color(Theme::divider())
            .child(div().w(px(320.)).child(format!("Package ({})", visible.len())))
            .child("Version");
        div()
            .id("pkg-window")
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(13.))
            .child(toolbar)
            .children(notice)
            .child(header)
            .child(rows)
            .into_any_element()
    }

    // --- Smoke --------------------------------------------------------------------------

    /// BRISK_SMOKE_PACKAGES=1 on a folder with a `.venv` and `PIP_FIND_LINKS` / `PIP_NO_INDEX`
    /// pointing at a local wheel named `briskdemo`: list, filter, install, uninstall.
    pub(super) fn python_packages_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            // Interpreter discovery runs at startup.
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.runs.interpreter(&t.root).is_some()).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| this.dispatch(Action::TogglePythonPackages, window, cx));
            let wait_idle = async |this: &gpui::WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext| {
                for _ in 0..600 {
                    pause(100).await;
                    if this.update(cx, |t, _| t.packages.as_ref().is_some_and(|p| !p.loading && p.busy.is_none())).unwrap_or(false) {
                        break;
                    }
                }
            };
            wait_idle(&this, cx).await;
            let names = |this: &Workspace, cx: &gpui::App| this.packages.as_ref().map(|p| p.visible(cx).iter().map(|p| format!("{} {}", p.name, p.version)).collect::<Vec<_>>()).unwrap_or_default();
            let _ = this.update_in(cx, |this, window, cx| {
                let p = this.packages.as_ref().unwrap();
                eprintln!("smoke-packages: tool {:?} python {:?} list {:?} error {:?}", p.tool, p.python, names(this, cx), p.error);
                let name = p.name.clone();
                window.focus(&name.read(cx).focus_handle(cx));
                this.smoke_type("briskdemo", window, cx);
                this.smoke_press("tab", window, cx);
                this.smoke_type("1.0", window, cx);
                this.smoke_press("enter", window, cx);
                eprintln!("smoke-packages: install -> status {:?}", this.status);
            });
            wait_idle(&this, cx).await;
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-packages: after install toast {:?} list {:?}", this.toast.as_ref().map(|t| t.0.clone()), names(this, cx));
                let f = this.packages.as_ref().unwrap().filter.clone();
                window.focus(&f.read(cx).focus_handle(cx));
                this.smoke_type("demo", window, cx);
            });
            pause(300).await;
            let hold = std::env::var("BRISK_SMOKE_PACKAGES").as_deref() == Ok("hold");
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-packages: filter demo -> {:?}", names(this, cx));
                let f = this.packages.as_ref().unwrap().focus.clone();
                window.focus(&f);
                this.smoke_press("delete", window, cx);
                eprintln!("smoke-packages: delete -> confirm {:?}", this.packages.as_ref().unwrap().confirm);
                if !hold {
                    this.smoke_press("enter", window, cx);
                }
            });
            if hold {
                return;
            }
            pause(200).await;
            wait_idle(&this, cx).await;
            let _ = this.update_in(cx, |this, _, cx| {
                eprintln!("smoke-packages: after uninstall toast {:?} filtered {:?} total {}", this.toast.as_ref().map(|t| t.0.clone()), names(this, cx), this.packages.as_ref().unwrap().list.len());
                cx.quit();
            });
        })
        .detach();
    }
}
