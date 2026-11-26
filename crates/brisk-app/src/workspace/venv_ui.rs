//! Add Interpreter: a new `.venv` in the project from a chosen base Python (in the
//! background, progress in the status bar), made the project interpreter; then an offer to
//! install the project's requirements into it.

use super::{PopupItem, Target, Workspace};
use crate::theme::ui_px;
use crate::theme::Theme;
use brisk_run::env::manager as py_manager;
use brisk_run::env::venv::{self as py_venv, Requirements};
use brisk_run::pyproj::{self, Manager};
use brisk_run::InterpKind;
use gpui::prelude::*;
#[cfg(feature = "smoke")]
use gpui::Window;
use gpui::{div, px, Context};
use std::path::PathBuf;

/// "Install requirements?" after a new environment was created.
pub struct VenvOffer {
    pub python: PathBuf,
    pub what: OfferKind,
    /// pip, uv or Poetry is running.
    pub installing: bool,
}

pub enum OfferKind {
    /// Install the project's requirements into a new environment.
    Install(Requirements),
    /// `pyproject.toml` changed after the lock file: sync with uv or Poetry.
    Sync(Manager),
}

impl VenvOffer {
    pub(super) fn text(&self) -> String {
        match &self.what {
            OfferKind::Install(req) => format!("The project lists its requirements in {}. Install them into the new environment?", req.label()),
            OfferKind::Sync(m) => format!("pyproject.toml changed and {} is out of date. Lock and sync the environment with {}?", m.lock_file(), m.name()),
        }
    }

    fn button(&self) -> &'static str {
        match self.what {
            OfferKind::Install(_) => "Install requirements",
            OfferKind::Sync(Manager::Uv) => "uv sync",
            OfferKind::Sync(Manager::Poetry) => "poetry lock && install",
        }
    }
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
#[cfg(feature = "smoke")]
fn offer_button_center(i: usize) -> Option<(f32, f32)> {
    OFFER_BUTTONS.with(|c| c.borrow()[i]).map(|b| (f32::from(b.center().x), f32::from(b.center().y)))
}

impl Workspace {
    /// Items of the Add Interpreter popup: base interpreters (system and conda).
    pub(super) fn add_interpreter_items(&self, q: &str) -> Vec<PopupItem> {
        let ql = q.to_lowercase();
        // A uv or Poetry project gets its environment from the lock file first.
        let mut managed = Vec::new();
        if let Some(m) = pyproj::detect(&self.root) {
            let (label, detail) = match m {
                Manager::Uv => ("uv sync", "create or update .venv from pyproject.toml and uv.lock"),
                Manager::Poetry => ("poetry install", "create or update the Poetry environment from poetry.lock"),
            };
            if label.contains(&ql) || m.name().to_lowercase().contains(&ql) {
                managed.push(PopupItem { label: label.into(), detail: detail.into(), hint: String::new(), target: Target::SyncEnv(m) });
            }
        }
        let dir = py_venv::free_dir(&self.root).and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()));
        let Some(dir) = dir else {
            managed.push(PopupItem {
                label: "The project already has .venv and venv".into(),
                detail: "select one of them in the interpreter list".into(),
                hint: String::new(),
                target: Target::Cancel,
            });
            return managed;
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
        } else if v.is_empty() && managed.is_empty() {
            v.push(PopupItem {
                label: "No base Python found".into(),
                detail: "install Python from python.org".into(),
                hint: String::new(),
                target: Target::Cancel,
            });
        }
        managed.extend(v);
        managed
    }

    /// `uv sync` / `poetry install` in the background, then make the environment the project
    /// interpreter, as after creating a venv.
    pub(super) fn sync_env(&mut self, m: Manager, cx: &mut Context<Self>) {
        if self.venv_busy {
            return;
        }
        let current = self.runs.interpreter(&self.root).map(|i| i.path);
        let tool = match py_manager::tool(m, current.as_deref()) {
            Ok(t) => t,
            Err(e) => {
                self.status = e.clone();
                self.show_toast(e, true, cx);
                return;
            }
        };
        let what = m.sync_args().join(" ");
        let what = format!("{} {what}", m.name().to_lowercase());
        self.venv_busy = true;
        self.status = format!("{what}…");
        let root = self.root.clone();
        let task = cx.background_executor().spawn(async move { py_manager::sync(m, &tool, &root) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.venv_busy = false;
                match res {
                    Ok(python) => this.use_new_environment(python, format!("{what}: done"), cx),
                    Err(e) => {
                        let msg = format!("{what} failed: {e}");
                        this.status = msg.clone();
                        this.show_toast(msg, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// After a save of `pyproject.toml` whose dependencies changed while the lock file is older:
    /// the balloon offering `uv sync` / `poetry lock && install`.
    pub(super) fn pyproject_saved(&mut self, saved: &[PathBuf], cx: &mut Context<Self>) {
        let file = self.root.join("pyproject.toml");
        if !saved.contains(&file) {
            return;
        }
        let fp = std::fs::read_to_string(&file).ok().map(|t| pyproj::deps_fingerprint(&t));
        let changed = fp != self.pyproject_fp;
        self.pyproject_fp = fp;
        let Some(m) = pyproj::detect(&self.root) else { return };
        if !changed || !pyproj::lock_outdated(&self.root, m) || self.venv_busy || self.venv_offer.is_some() {
            return;
        }
        let python = self.runs.interpreter(&self.root).map(|i| i.path).unwrap_or_default();
        self.venv_offer = Some(VenvOffer { python, what: OfferKind::Sync(m), installing: false });
        cx.notify();
    }

    /// Remembers the dependency part of `pyproject.toml`, so a later save can tell whether
    /// the dependencies changed.
    pub(super) fn remember_pyproject(&mut self) {
        self.pyproject_fp = std::fs::read_to_string(self.root.join("pyproject.toml")).ok().map(|t| pyproj::deps_fingerprint(&t));
    }

    /// Makes a freshly created or synced environment the project interpreter and tells every
    /// part that depends on it (ty, the terminal, Packages, the interpreter list).
    fn use_new_environment(&mut self, python: PathBuf, msg: String, cx: &mut Context<Self>) {
        self.remember_pyproject();
        self.runs.venvs = brisk_run::project_venvs(&self.root);
        self.runs.file.interpreter = Some(brisk_run::relativize(&self.root, &python));
        self.save_runs();
        self.restart_ty(cx);
        self.sync_terminal_env(cx);
        self.start_interpreter_discovery(cx);
        let label = self.runs.lookup(&python).label(&self.root);
        let msg = format!("{msg}; the project interpreter is {label}");
        self.status = msg.clone();
        self.show_toast(msg, false, cx);
        if self.packages.is_some() {
            self.refresh_packages(cx);
        }
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
                        this.use_new_environment(python.clone(), format!("Created {name}"), cx);
                        this.venv_offer = req.map(|req| VenvOffer { python, what: OfferKind::Install(req), installing: false });
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
        let req = match &o.what {
            OfferKind::Install(req) => req.clone(),
            OfferKind::Sync(m) => {
                let m = *m;
                self.venv_offer = None;
                self.sync_env(m, cx);
                return;
            }
        };
        o.installing = true;
        let (python, root) = (o.python.clone(), self.root.clone());
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
        // The balloon swaps its buttons for "Installing…" at once.
        cx.notify();
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
                .map(|d| crate::ui_kit::button_states(d, primary))
                .child(label)
                .child(track(if primary { 0 } else { 1 }))
        };
        Some(
            div()
                .id("venv-offer")
                .occlude()
                .absolute()
                .bottom(ui_px(28.) + px(60.))
                .right(super::toast_right())
                .w(ui_px(420.))
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
                .child(div().child(o.text()))
                .child(if o.installing {
                    div().text_color(Theme::text_muted()).child("Installing… (progress in the status bar)").into_any_element()
                } else {
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(button("venv-install", o.button(), true).on_click(cx.listener(|this, _, _, cx| this.install_requirements(cx))))
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
    #[cfg(feature = "smoke")]
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
                let labels: Vec<String> =
                    this.popup.as_ref().map(|p| p.items.iter().map(|i| format!("{} | {}", i.label, i.detail)).collect()).unwrap_or_default();
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
                eprintln!(
                    "smoke-venv: created -> interpreter {:?}, .venv exists {}, toast {:?}",
                    this.interpreter_label(),
                    this.root.join(".venv").join("pyvenv.cfg").is_file(),
                    this.toast.as_ref().map(|t| t.0.clone())
                );
                eprintln!("smoke-venv: offer {:?}", this.venv_offer.as_ref().map(|o| o.text()));
            });
            if hold {
                return;
            }
            // A real click on Install requirements (the balloon fades in: wait for its paint).
            let click = |i: usize, window: &mut Window| -> Option<(f32, f32)> {
                let (x, y) = offer_button_center(i)?;
                let scale = window.scale_factor();
                super::smoke::win_input::click(x * scale, y * scale);
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
                this.venv_offer =
                    python.zip(py_venv::requirements(&this.root)).map(|(python, req)| VenvOffer { python, what: OfferKind::Install(req), installing: false });
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
                eprintln!("smoke-venv: after Not now offer {:?}, status {:?}", this.venv_offer.as_ref().map(|o| o.text()), this.status);
                cx.quit();
            });
        })
        .detach();
    }
}
