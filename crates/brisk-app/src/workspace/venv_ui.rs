//! Add Interpreter: a new `.venv` in the project from a chosen base Python (in the
//! background, progress in the status bar), made the project interpreter; then an offer to
//! install the project's requirements into it.

use crate::theme::ui_px;
use super::{PopupItem, Target, Workspace};
use crate::py_venv::{self, Requirements};
use crate::theme::Theme;
use brisk_run::InterpKind;
use gpui::prelude::*;
use gpui::{div, px, Context, Window};
use std::path::PathBuf;

/// "Install requirements?" after a new environment was created.
pub struct VenvOffer {
    pub python: PathBuf,
    pub req: Requirements,
    /// pip is running.
    pub installing: bool,
}

thread_local! {
    /// Where Install requirements (0) and Not now (1) were painted: smoke clicks aim here.
    static OFFER_BUTTONS: std::cell::RefCell<[Option<gpui::Bounds<gpui::Pixels>>; 2]> = const { std::cell::RefCell::new([None, None]) };
}

/// A canvas filling its parent that records the parent's bounds in `OFFER_BUTTONS[i]`.
fn track(i: usize) -> impl IntoElement {
    gpui::canvas(move |b, _, _| OFFER_BUTTONS.with(|c| c.borrow_mut()[i] = Some(b)), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
}

/// The center of a painted offer button (window logical pixels).
fn offer_button_center(i: usize) -> Option<(f32, f32)> {
    OFFER_BUTTONS.with(|c| c.borrow()[i]).map(|b| (f32::from(b.center().x), f32::from(b.center().y)))
}

impl Workspace {
    /// Items of the Add Interpreter popup: base interpreters (system and conda).
    pub(super) fn add_interpreter_items(&self, q: &str) -> Vec<PopupItem> {
        let ql = q.to_lowercase();
        let dir = py_venv::free_dir(&self.root).and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()));
        let Some(dir) = dir else {
            return vec![PopupItem { label: "The project already has .venv and venv".into(), detail: "select one of them in the interpreter list".into(), hint: String::new(), target: Target::Cancel }];
        };
        let mut v: Vec<PopupItem> = self
            .runs
            .all
            .iter()
            .flatten()
            .filter(|i| i.kind != InterpKind::Venv)
            .filter(|i| i.path.to_string_lossy().to_lowercase().contains(&ql) || i.label(&self.root).to_lowercase().contains(&ql))
            .map(|i| PopupItem {
                label: format!("New {dir} with {}", i.label(&self.root)),
                detail: i.path.display().to_string(),
                hint: String::new(),
                target: Target::CreateVenv(i.path.clone()),
            })
            .collect();
        v.dedup_by(|a, b| a.detail == b.detail);
        if self.runs.all.is_none() {
            v.push(PopupItem { label: "Detecting interpreters…".into(), detail: String::new(), hint: String::new(), target: Target::Cancel });
        } else if v.is_empty() {
            v.push(PopupItem { label: "No base Python found".into(), detail: "install Python from python.org".into(), hint: String::new(), target: Target::Cancel });
        }
        v
    }

    /// `base -m venv .venv` in the background, then make it the project interpreter.
    pub(super) fn create_venv(&mut self, base: PathBuf, cx: &mut Context<Self>) {
        if self.venv_busy {
            return;
        }
        let Some(dir) = py_venv::free_dir(&self.root) else {
            self.show_toast("The project already has .venv and venv".into(), true, cx);
            return;
        };
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let base_label = self.runs.lookup(&base).label(&self.root);
        self.venv_busy = true;
        self.status = format!("Creating {name} with {base_label}…");
        let root = self.root.clone();
        let task = cx.background_executor().spawn(async move {
            let python = py_venv::create(&base, &dir)?;
            Ok::<_, String>((python, py_venv::requirements(&root)))
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.venv_busy = false;
                match res {
                    Ok((python, req)) => {
                        this.runs.venvs = brisk_run::project_venvs(&this.root);
                        this.runs.file.interpreter = Some(brisk_run::relativize(&this.root, &python));
                        this.save_runs();
                        this.restart_ty(cx);
                        this.sync_terminal_env(cx);
                        let msg = format!("Created {name}: it is the project interpreter now");
                        this.status = msg.clone();
                        this.show_toast(msg, false, cx);
                        this.venv_offer = req.map(|req| VenvOffer { python, req, installing: false });
                    }
                    Err(e) => {
                        let msg = format!("Cannot create {name}: {e}");
                        this.status = msg.clone();
                        this.show_toast(msg, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn install_requirements(&mut self, cx: &mut Context<Self>) {
        let Some(o) = self.venv_offer.as_mut().filter(|o| !o.installing) else { return };
        o.installing = true;
        let (python, req, root) = (o.python.clone(), o.req.clone(), self.root.clone());
        self.status = format!("Installing requirements from {}…", req.label());
        let task = cx.background_executor().spawn(async move { py_venv::install(&python, &root, &req) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.venv_offer = None;
                let (msg, err) = match res {
                    Ok(()) => ("Requirements installed".to_string(), false),
                    Err(e) => (format!("Installing requirements failed: {e}"), true),
                };
                this.status = msg.clone();
                this.show_toast(msg, err, cx);
                if this.packages.is_some() {
                    this.refresh_packages(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The balloon offering to install requirements into the new environment.
    pub(super) fn render_venv_offer(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let o = self.venv_offer.as_ref()?;
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .relative()
                .h(ui_px(24.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .cursor_pointer()
                .when(primary, |d| d.bg(Theme::accent()).text_color(gpui::white()))
                .when(!primary, |d| d.border_1().border_color(Theme::border()))
                .hover(|s| s.opacity(0.9))
                .active(|s| s.bg(Theme::press()))
                .child(label)
                .child(track(if primary { 0 } else { 1 }))
        };
        Some(
            div()
                .id("venv-offer")
                .occlude()
                .absolute()
                .bottom(px(96.))
                .right(px(16.))
                .w(px(420.))
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .rounded(px(8.))
                .bg(Theme::panel())
                .border_1()
                .border_color(Theme::divider())
                .map(crate::ui_kit::floating)
                .text_size(ui_px(13.))
                .text_color(Theme::text())
                .child(div().child(format!("The project lists its requirements in {}. Install them into the new environment?", o.req.label())))
                .child(if o.installing {
                    div().text_color(Theme::text_muted()).child("Installing… (progress in the status bar)").into_any_element()
                } else {
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(button("venv-install", "Install requirements", true).on_click(cx.listener(|this, _, _, cx| this.install_requirements(cx))))
                        .child(button("venv-dismiss", "Not now", false).on_click(cx.listener(|this, _, _, cx| {
                            this.venv_offer = None;
                            cx.notify();
                        })))
                        .into_any_element()
                })
                .into_any_element(),
        )
    }

    // --- Smoke --------------------------------------------------------------------------

    /// BRISK_SMOKE_VENV=1|hold on a folder without a venv but with a requirements.txt (for
    /// offline runs PIP_NO_INDEX / PIP_FIND_LINKS point at a local wheel): Add Interpreter,
    /// the first base Python, wait for .venv, then real clicks on Install requirements and,
    /// on the offer shown again, Not now.
    pub(super) fn venv_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        OFFER_BUTTONS.with(|c| *c.borrow_mut() = [None, None]);
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..150 {
                pause(100).await;
                if this.update(cx, |t, _| t.runs.all.is_some()).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-venv: interpreter before {:?}", this.interpreter_label());
                this.open_popup(super::PopupKind::Interpreters);
                let labels: Vec<String> = this.popup.as_ref().map(|p| p.items.iter().map(|i| i.label.clone()).collect()).unwrap_or_default();
                eprintln!("smoke-venv: interpreter popup {labels:?}");
                if let Some(p) = this.popup.as_mut() {
                    p.selected = p.items.iter().position(|i| i.label == "Add Interpreter…").unwrap_or(0);
                }
                this.smoke_press("enter", window, cx);
                let labels: Vec<String> = this.popup.as_ref().map(|p| p.items.iter().map(|i| format!("{} | {}", i.label, i.detail)).collect()).unwrap_or_default();
                eprintln!("smoke-venv: add interpreter popup {labels:?}");
                this.smoke_press("enter", window, cx);
                eprintln!("smoke-venv: status {:?}", this.status);
            });
            for _ in 0..600 {
                pause(100).await;
                if this.update(cx, |t, _| !t.venv_busy).unwrap_or(true) {
                    break;
                }
            }
            pause(200).await;
            let hold = std::env::var("BRISK_SMOKE_VENV").as_deref() == Ok("hold");
            let _ = this.update_in(cx, |this, _, _| {
                eprintln!("smoke-venv: created -> interpreter {:?}, .venv exists {}, toast {:?}", this.interpreter_label(), this.root.join(".venv").join("pyvenv.cfg").is_file(), this.toast.as_ref().map(|t| t.0.clone()));
                eprintln!("smoke-venv: offer {:?}", this.venv_offer.as_ref().map(|o| o.req.label()));
            });
            if hold {
                return;
            }
            // A real click on Install requirements (the balloon fades in: wait for its paint).
            let click = |i: usize, window: &mut Window| -> Option<(f32, f32)> {
                let (x, y) = offer_button_center(i)?;
                let scale = window.scale_factor();
                super::win_input::click(x * scale, y * scale);
                Some((x, y))
            };
            for _ in 0..30 {
                pause(100).await;
                if offer_button_center(0).is_some() {
                    break;
                }
            }
            pause(300).await;
            let _ = this.update_in(cx, |_, window, _| {
                let at = click(0, window);
                eprintln!("smoke-venv: click Install requirements at {at:?}");
            });
            for _ in 0..20 {
                pause(100).await;
                if this.update(cx, |t, _| t.venv_offer.as_ref().is_some_and(|o| o.installing)).unwrap_or(false) {
                    break;
                }
            }
            eprintln!("smoke-venv: after click installing {:?}", this.update(cx, |t, _| t.venv_offer.as_ref().map(|o| o.installing)).ok().flatten());
            for _ in 0..600 {
                pause(100).await;
                if this.update(cx, |t, _| t.venv_offer.is_none()).unwrap_or(true) {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, _, cx| {
                let site = this.root.join(".venv").join("Lib").join("site-packages").join("briskdemo");
                eprintln!("smoke-venv: install -> status {:?}, briskdemo installed {}", this.status, site.is_dir());
                // The offer again (as after another Add Interpreter), to click Not now.
                OFFER_BUTTONS.with(|c| *c.borrow_mut() = [None, None]);
                let python = brisk_run::venv_python(&this.root.join(".venv"));
                this.venv_offer = python.zip(py_venv::requirements(&this.root)).map(|(python, req)| VenvOffer { python, req, installing: false });
                cx.notify();
            });
            for _ in 0..30 {
                pause(100).await;
                if offer_button_center(1).is_some() {
                    break;
                }
            }
            pause(300).await;
            let _ = this.update_in(cx, |_, window, _| {
                let at = click(1, window);
                eprintln!("smoke-venv: click Not now at {at:?}");
            });
            for _ in 0..20 {
                pause(100).await;
                if this.update(cx, |t, _| t.venv_offer.is_none()).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, _, cx| {
                eprintln!("smoke-venv: after Not now offer {:?}, status {:?}", this.venv_offer.as_ref().map(|o| o.req.label()), this.status);
                cx.quit();
            });
        })
        .detach();
    }
}
