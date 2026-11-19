//! Smokes of the Python Packages window.

use super::Workspace;
use super::packages_ui::Op;
use brisk_keymap::Action;
use gpui::{Context, Focusable, Window};

impl Workspace {

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

    /// BRISK_SMOKE_PKGQUEUE=1 on a folder with a `.venv` holding briskdemo 1.0, with
    /// `PIP_FIND_LINKS` / `PIP_NO_INDEX` at local wheels (briskdemo 2.0, brisktwo 1.0) and
    /// `BRISK_PYPI_INDEX` at a local Simple-API JSON: the outdated check, name suggestions,
    /// an install and an upgrade queued behind it (spinner, "Queued"), then the final list.
    pub(super) fn python_packages_queue_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.runs.interpreter(&t.root).is_some()).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| this.dispatch(Action::TogglePythonPackages, window, cx));
            let idle = |t: &Workspace| t.packages.as_ref().is_some_and(|p| !p.loading && p.busy.is_none() && p.queue.is_empty() && !p.outdated_loading && !p.index_loading);
            for _ in 0..600 {
                pause(100).await;
                if this.update(cx, |t, _| idle(t)).unwrap_or(false) {
                    break;
                }
            }
            let names = |this: &Workspace, cx: &gpui::App| this.packages.as_ref().map(|p| p.visible(cx).iter().filter(|p| p.name.starts_with("brisk")).map(|p| format!("{} {}", p.name, p.version)).collect::<Vec<_>>()).unwrap_or_default();
            let _ = this.update_in(cx, |this, window, cx| {
                let p = this.packages.as_ref().unwrap();
                eprintln!("smoke-pkgqueue: list {:?} outdated {:?} error {:?}", names(this, cx), p.outdated, p.error);
                let name = p.name.clone();
                window.focus(&name.read(cx).focus_handle(cx));
                this.smoke_type("brisk", window, cx);
            });
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.packages.as_ref().is_some_and(|p| !p.index_loading && !p.suggestions.is_empty())).unwrap_or(false) {
                    break;
                }
            }
            let mode = std::env::var("BRISK_SMOKE_PKGQUEUE").unwrap_or_default();
            if mode == "popup" {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.smoke_press("down", window, cx);
                    cx.notify();
                });
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-pkgqueue: typed brisk -> suggestions {:?}", this.packages.as_ref().map(|p| p.suggestions.clone()));
                this.smoke_press("down enter", window, cx);
                let p = this.packages.as_ref().unwrap();
                let in_version = p.version.read(cx).focus_handle(cx).is_focused(window);
                eprintln!("smoke-pkgqueue: down+enter -> name {:?}, focus in version {in_version}", p.name.read(cx).text_string());
                this.smoke_press("enter", window, cx);
                // Upgrade briskdemo while the install runs: it waits in the queue.
                this.package_op(Op::Upgrade("briskdemo".into()), cx);
                let p = this.packages.as_ref().unwrap();
                eprintln!(
                    "smoke-pkgqueue: queued -> running {:?}, queue {:?}, new rows {:?}, briskdemo {:?}",
                    p.running,
                    p.queue,
                    p.pending_new().iter().map(|(o, r)| (o.name().to_string(), *r)).collect::<Vec<_>>(),
                    p.op_on("briskdemo").map(|(_, r)| r)
                );
            });
            if mode == "queue" {
                return;
            }
            for _ in 0..1200 {
                pause(100).await;
                if this.update(cx, |t, _| idle(t)).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| {
                let p = this.packages.as_ref().unwrap();
                eprintln!("smoke-pkgqueue: done -> list {:?} outdated {:?} error {:?} toast {:?}", names(this, cx), p.outdated, p.error, this.toast.as_ref().map(|t| t.0.clone()));
                cx.quit();
            });
        })
        .detach();
    }
}
