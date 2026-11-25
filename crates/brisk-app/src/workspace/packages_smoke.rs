//! Smokes of the Python Packages window.

use super::Workspace;
use super::packages_ui::Op;
use brisk_keymap::Action;
use gpui::{Context, Focusable, Window};
use std::path::Path;

/// Builds the smokes' wheels (briskdemo 1.0 and 2.0, brisktwo 1.0) and a Simple-API JSON
/// for the name suggestions into argv[1].
pub(super) const MAKE_WHEELS: &str = r#"
import base64, hashlib, json, os, sys, zipfile
out = sys.argv[1]
os.makedirs(out, exist_ok=True)
def wheel(name, version):
    files = {f"{name}/__init__.py": f"VERSION = {version!r}\n",
             f"{name}-{version}.dist-info/METADATA": f"Metadata-Version: 2.1\nName: {name}\nVersion: {version}\n",
             f"{name}-{version}.dist-info/WHEEL": "Wheel-Version: 1.0\nGenerator: brisk\nRoot-Is-Purelib: true\nTag: py3-none-any\n"}
    rec = []
    for p, t in files.items():
        d = hashlib.sha256(t.encode()).digest()
        rec.append(f"{p},sha256={base64.urlsafe_b64encode(d).rstrip(b'=').decode()},{len(t.encode())}")
    rec.append(f"{name}-{version}.dist-info/RECORD,,")
    files[f"{name}-{version}.dist-info/RECORD"] = "\n".join(rec) + "\n"
    with zipfile.ZipFile(os.path.join(out, f"{name}-{version}-py3-none-any.whl"), "w") as z:
        for p, t in files.items():
            z.writestr(p, t)
for n, v in [("briskdemo", "1.0"), ("briskdemo", "2.0"), ("brisktwo", "1.0")]:
    wheel(n, v)
names = ["requests", "brisktwo", "briskdemo", "flask", "brisk-extra-long-name"]
json.dump({"meta": {"api-version": "1.0"}, "projects": [{"name": n} for n in names]}, open(os.path.join(out, "simple.json"), "w"))
"#;

/// The smokes' own Python, made offline in the smoke folder: a `.venv` from an interpreter
/// on this machine, local wheels in `wheels/` that pip installs from (`PIP_FIND_LINKS`,
/// `PIP_NO_INDEX`: it never asks PyPI) and a local index for the name suggestions
/// (`BRISK_PYPI_INDEX`). `preinstall` goes into the venv first. Blocks for seconds.
fn setup_smoke_python(root: &Path, preinstall: &[&str]) -> Result<(), String> {
    let base = brisk_run::discover_all(root)
        .into_iter()
        .find(|i| i.kind == brisk_run::InterpKind::System)
        .ok_or("no Python on this machine")?
        .path;
    let wheels = root.join("wheels");
    let out = std::process::Command::new(&base).arg("-I").arg("-c").arg(MAKE_WHEELS).arg(&wheels).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    let url = format!("file:///{}", wheels.join("simple.json").to_string_lossy().replace('\\', "/").trim_start_matches('/'));
    // Read by pip and by Brisk's own index fetch; set before anything starts them.
    std::env::set_var("PIP_FIND_LINKS", &wheels);
    std::env::set_var("PIP_NO_INDEX", "1");
    std::env::set_var("BRISK_PYPI_INDEX", url);
    let py = crate::py_venv::create(&base, &root.join(".venv"))?;
    for req in preinstall {
        let out = std::process::Command::new(&py).args(["-m", "pip", "install", "--disable-pip-version-check", req]).output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
    }
    Ok(())
}

impl Workspace {
    /// Set up the smoke's Python in the background, then point the project at its venv.
    async fn smoke_python(this: &gpui::WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext, preinstall: &'static [&'static str]) -> bool {
        let Ok(root) = this.update(cx, |t, _| t.root.clone()) else { return false };
        let t0 = std::time::Instant::now();
        let res = cx.background_executor().spawn(async move { setup_smoke_python(&root, preinstall) }).await;
        eprintln!("smoke-python: venv and local wheels in {:.1?} -> {res:?}", t0.elapsed());
        let _ = this.update(cx, |t, cx| {
            t.runs.venvs = brisk_run::project_venvs(&t.root);
            cx.notify();
        });
        res.is_ok()
    }

    /// BRISK_SMOKE_PACKAGES=1 on an empty folder (it makes its own venv and local wheels, and
    /// never reaches the network): list, filter, install, uninstall.
    pub(super) fn python_packages_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            if !Self::smoke_python(&this, cx, &[]).await {
                let _ = this.update(cx, |_, cx| cx.quit());
                return;
            }
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
                // As if pip / uv were not detected yet: the install waits in the queue.
                this.packages.as_mut().unwrap().tool = None;
                window.focus(&name.read(cx).focus_handle(cx));
                this.smoke_type("briskdemo", window, cx);
                this.smoke_press("tab", window, cx);
                this.smoke_type("1.0", window, cx);
                this.smoke_press("enter", window, cx);
                let p = this.packages.as_ref().unwrap();
                eprintln!("smoke-packages: install before the tool is known -> queued {:?}, detecting {}, status {:?}", p.queue, p.loading, this.status);
            });
            wait_idle(&this, cx).await;
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-packages: after install toast {:?} list {:?}", this.toast.as_ref().map(|t| t.0.clone()), names(this, cx));
                // Down from the filter field selects a package; the caret blinking in the
                // field must not move the selection back.
                let f = this.packages.as_ref().unwrap().filter.clone();
                window.focus(&f.read(cx).focus_handle(cx));
                this.smoke_press("down", window, cx);
            });
            pause(1500).await;
            let _ = this.update_in(cx, |this, window, cx| {
                let p = this.packages.as_ref().unwrap();
                let want = 1.min(p.visible(cx).len().saturating_sub(1));
                eprintln!("smoke-packages: down in the filter, 1.5 s later -> selected {}{}", p.selected, if p.selected == want { "" } else { " FAIL" });
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

    /// BRISK_SMOKE_PKGQUEUE=1 on an empty folder (its own venv holding briskdemo 1.0, local
    /// wheels briskdemo 2.0 and brisktwo 1.0, a local index; no network): the outdated check,
    /// name suggestions, an install and an upgrade queued behind it (spinner, "Queued"), then
    /// the final list. Also the working set around the suggestions, and one search over a
    /// PyPI-sized list (944 000 names).
    pub(super) fn python_packages_queue_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            if !Self::smoke_python(&this, cx, &["briskdemo==1.0"]).await {
                let _ = this.update(cx, |_, cx| cx.quit());
                return;
            }
            let mb = || brisk_proc::memory().map(|m| m.0 as f64 / 1048576.0).unwrap_or(0.0);
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
                eprintln!("smoke-pkgqueue: working set before suggestions {:.1} MB", mb());
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
            eprintln!("smoke-pkgqueue: working set with suggestions {:.1} MB", mb());
            // A list the size of PyPI's: one search reads it from disk, nothing stays in memory.
            let big = this.update(cx, |t, _| t.root.join("names-big.txt")).unwrap_or_default();
            let (ms, found, peak, size) = bg
                .spawn(async move {
                    let mut text = String::with_capacity(16 << 20);
                    for i in 0..944_000 {
                        text.push_str(&format!("project-{i:06}-name\tProject_{i:06}.Name\n"));
                    }
                    text.push_str("brisk-big-one\tBrisk_Big_One\n");
                    let _ = std::fs::write(&big, text);
                    let t = std::time::Instant::now();
                    let found = crate::py_packages::suggest_file(&big, "brisk-big", 12, &|| true);
                    let ms = t.elapsed().as_secs_f64() * 1000.0;
                    let peak = brisk_proc::memory().map(|m| m.0 as f64 / 1048576.0).unwrap_or(0.0);
                    let size = std::fs::metadata(&big).map(|m| m.len() / 1_000_000).unwrap_or(0);
                    let _ = std::fs::remove_file(&big);
                    (ms, found, peak, size)
                })
                .await;
            eprintln!("smoke-pkgqueue: search over 944 001 names ({size} MB file) -> {found:?} in {ms:.0} ms, working set right after {peak:.1} MB");
            pause(2000).await;
            eprintln!("smoke-pkgqueue: working set 2 s later {:.1} MB", mb());
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
