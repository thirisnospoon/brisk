//! The balloon that offers to install a missing package (coverage.py, debugpy) into the
//! interpreter a launch needs, then starts that launch again.

use super::Workspace;
use crate::theme::{ui_px, Theme};
use brisk_run::RunConfig;
use gpui::prelude::*;
use gpui::{div, px, Context, Window};
use std::path::PathBuf;

/// What starts once the package is installed.
pub enum Then {
    /// Run with Coverage of this configuration.
    Coverage(RunConfig),
    /// Debug this configuration.
    Debug(RunConfig),
    /// Attach to Process `.0` (shown as `.1`).
    Attach(u32, String),
}

pub struct InstallOffer {
    /// The pip name, also shown in the balloon.
    pub package: &'static str,
    pub python: PathBuf,
    pub then: Then,
}

impl Workspace {
    pub(super) fn render_install_offer(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let offer = self.install_offer.as_ref()?;
        let label = self.env.read(cx).lookup(&offer.python).label(&self.root);
        let shown = match offer.package {
            "coverage" => "coverage.py",
            p => p,
        };
        let question = match &offer.then {
            Then::Coverage(cfg) => format!("{shown} is not installed in {label}. Install it and run '{}' with coverage?", cfg.name),
            Then::Debug(cfg) => format!("{shown} is not installed in {label}. Install it and debug '{}'?", cfg.name),
            Then::Attach(_, process) => format!("{shown} is not installed in {label}. Install it and attach to {process}?"),
        };
        let button = |id: &'static str, text: String, primary: bool| {
            div()
                .id(id)
                .h(ui_px(24.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .cursor_pointer()
                .when(primary, |d| d.bg(Theme::accent()).text_color(gpui::white()))
                .when(!primary, |d| d.border_1().border_color(Theme::border()))
                .map(|d| crate::ui_kit::button_states(d, primary))
                .child(text)
        };
        Some(
            div()
                .id("install-offer")
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
                .child(div().child(question))
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            button("install-offer-ok", format!("Install {}", offer.package), true)
                                .on_click(cx.listener(|this, _, w, cx| this.install_offered(w, cx))),
                        )
                        .child(button("install-offer-cancel", "Cancel".into(), false).on_click(cx.listener(|this, _, _, cx| {
                            this.install_offer = None;
                            cx.notify();
                        }))),
                )
                .into_any_element(),
        )
    }

    /// "Install …": pip in the background, then the launch that needed the package.
    pub(super) fn install_offered(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(InstallOffer { package, python, then }) = self.install_offer.take() else { return };
        self.status = format!("Installing {package}…");
        let (py, root) = (python.clone(), self.root.clone());
        let task = cx
            .background_executor()
            .spawn(async move { brisk_run::env::venv::install(&py, &root, &brisk_run::env::venv::Requirements::Pyproject(vec![package.into()])) });
        cx.spawn_in(window, async move |this, cx| {
            let res = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match res {
                    Ok(()) => {
                        this.status = format!("{package} installed");
                        match then {
                            Then::Coverage(cfg) => this.start_coverage_run(cfg, python, window, cx),
                            Then::Debug(cfg) => {
                                // Its Before launch steps already ran for the first attempt.
                                this.before_passed = Some(cfg.name.clone());
                                this.start_debug(cfg, window, cx);
                                this.before_passed = None;
                            }
                            Then::Attach(pid, process) => this.reattach(pid, process, window, cx),
                        }
                    }
                    Err(e) => {
                        let msg = format!("Installing {package} failed: {e}");
                        this.status = msg.clone();
                        this.show_toast(msg, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    // --- Smoke --------------------------------------------------------------------------

    /// BRISK_SMOKE_DEBUGPY_OFFER=1|offer on an empty folder, with a project interpreter that
    /// has no debugpy and a build without a bundled copy (prints SKIP otherwise): debugging
    /// `hello.py` shows the install offer instead of an error toast; Install runs pip (offline
    /// under a smoke, so it reports pip's error). `=offer` stops on the offer.
    #[cfg(feature = "smoke")]
    pub(super) fn debugpy_offer_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let file = self.root.join("hello.py");
        let _ = std::fs::write(&file, "print('hello')\n");
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, cx| t.interpreter(cx).is_some()).unwrap_or(false) {
                    break;
                }
            }
            let python = this.update(cx, |t, cx| t.interpreter(cx).map(|i| i.path)).ok().flatten();
            let Some(python) = python else {
                eprintln!("smoke-debugpy-offer: SKIP no interpreter");
                let _ = this.update(cx, |_, cx| cx.quit());
                return;
            };
            let check = python.clone();
            let found = bg.spawn(async move { brisk_dap::adapter_command(&check).is_ok() }).await;
            if found {
                eprintln!("smoke-debugpy-offer: SKIP debugpy is available ({})", python.display());
                let _ = this.update(cx, |_, cx| cx.quit());
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                let cfg = RunConfig::for_file(&this.root, &file);
                this.start_debug(cfg, window, cx);
            });
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.install_offer.is_some()).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update(cx, |this, _| {
                let offer = this.install_offer.as_ref().map(|o| (o.package, matches!(o.then, Then::Debug(_))));
                eprintln!("smoke-debugpy-offer: offer {offer:?} (package, debug), toast {:?}", this.toast.as_ref().map(|t| t.0.clone()));
            });
            if std::env::var("BRISK_SMOKE_DEBUGPY_OFFER").as_deref() == Ok("offer") {
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| this.install_offered(window, cx));
            for _ in 0..600 {
                pause(100).await;
                if this.update(cx, |t, _| t.toast.is_some()).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| {
                eprintln!(
                    "smoke-debugpy-offer: after install -> offer {}, toast {:?}, status {:?}",
                    this.install_offer.is_some(),
                    this.toast.as_ref().map(|t| t.0.clone()),
                    this.status
                );
                cx.quit();
            });
        })
        .detach();
    }
}
