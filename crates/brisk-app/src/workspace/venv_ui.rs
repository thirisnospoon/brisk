//! Add Interpreter: the popup of base Pythons for a new `.venv` (or uv / Poetry sync), the
//! "Install requirements?" offer that follows, and the VENV smoke. Creating and syncing are
//! `PythonEnv`'s (`python_env.rs`).

use super::{PopupItem, Target, Workspace};
use brisk_run::env::venv::{self as py_venv, Requirements};
use brisk_run::pyproj::{self, Manager};
use brisk_run::InterpKind;
use gpui::prelude::*;
#[cfg(feature = "smoke")]
use gpui::{Context, Window};
use std::path::PathBuf;

/// "Install requirements?" after a new environment was created.
pub struct VenvOffer {
    pub python: PathBuf,
    pub what: OfferKind,
    /// pip, uv or Poetry is running.
    pub installing: bool,
}

#[derive(Clone)]
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

    pub(super) fn button(&self) -> &'static str {
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
pub(super) fn track(i: usize) -> impl IntoElement {
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
            .env_view
            .machine
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
        if self.env_view.machine.is_none() {
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
                if this.update(cx, |t, cx| t.env.read(cx).all.is_some()).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-venv: interpreter before {:?}", this.interpreter_label(cx));
                this.open_env_popup(super::PopupKind::Interpreters, cx);
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
                if this.update(cx, |t, cx| !t.env.read(cx).busy).unwrap_or(true) {
                    break;
                }
            }
            pause(200).await;
            let hold = std::env::var("BRISK_SMOKE_VENV").as_deref() == Ok("hold");
            let _ = this.update_in(cx, |this, _, cx| {
                eprintln!(
                    "smoke-venv: created -> interpreter {:?}, .venv exists {}, toast {:?}",
                    this.interpreter_label(cx),
                    this.root.join(".venv").join("pyvenv.cfg").is_file(),
                    this.toast.as_ref().map(|t| t.0.clone())
                );
                eprintln!("smoke-venv: offer {:?}", this.env.read(cx).offer.as_ref().map(|o| o.text()));
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
                if this.update(cx, |t, cx| t.env.read(cx).offer.as_ref().is_some_and(|o| o.installing)).unwrap_or(false) {
                    break;
                }
            }
            eprintln!("smoke-venv: after click installing {:?}", this.update(cx, |t, cx| t.env.read(cx).offer.as_ref().map(|o| o.installing)).ok().flatten());
            for _ in 0..600 {
                pause(100).await;
                if this.update(cx, |t, cx| t.env.read(cx).offer.is_none()).unwrap_or(true) {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, _, cx| {
                let site = this.root.join(".venv").join("Lib").join("site-packages").join("briskdemo");
                eprintln!("smoke-venv: install -> status {:?}, briskdemo installed {}", this.status, site.is_dir());
                // The offer again (as after another Add Interpreter), to click Not now.
                OFFER_BUTTONS.with(|c| *c.borrow_mut() = [None, None]);
                let python = brisk_run::venv_python(&this.root.join(".venv"));
                let offer =
                    python.zip(py_venv::requirements(&this.root)).map(|(python, req)| VenvOffer { python, what: OfferKind::Install(req), installing: false });
                this.env.update(cx, |e, cx| e.set_offer(offer, cx));
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
                if this.update(cx, |t, cx| t.env.read(cx).offer.is_none()).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, _, cx| {
                eprintln!("smoke-venv: after Not now offer {:?}, status {:?}", this.env.read(cx).offer.as_ref().map(|o| o.text()), this.status);
                cx.quit();
            });
        })
        .detach();
    }
}
