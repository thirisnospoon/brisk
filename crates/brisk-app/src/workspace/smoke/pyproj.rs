//! Smoke of uv and Poetry projects: syncing the environment, the Packages window editing
//! pyproject.toml, and the offer to sync after the dependencies change.

use crate::py_packages::Package;
use crate::workspace::packages_ui::Op;
use crate::workspace::smoke::packages::MAKE_WHEELS;
use crate::workspace::Workspace;
use brisk_keymap::Action;
use brisk_run::pyproj::Manager;
use gpui::{Context, Window};
use std::path::Path;

const UV_PYPROJECT: &str =
    "[project]\nname = \"smokeproj\"\nversion = \"0.1.0\"\nrequires-python = \">=3.9\"\ndependencies = []\n\n[tool.uv]\npackage = false\n";
const POETRY_PYPROJECT: &str =
    "[project]\nname = \"smokepoetry\"\nversion = \"0.1.0\"\nrequires-python = \">=3.9\"\ndependencies = []\n\n[tool.poetry]\npackage-mode = false\n";

/// Local wheels in `wheels/` and the environment that keeps uv and Poetry offline and their
/// files inside the smoke folder.
fn setup(root: &Path, manager: Manager) -> Result<(), String> {
    let base = brisk_run::discover_all(root).into_iter().find(|i| i.kind == brisk_run::InterpKind::System).ok_or("no Python on this machine")?.path;
    let wheels = root.join("wheels");
    let out = std::process::Command::new(&base).arg("-I").arg("-c").arg(MAKE_WHEELS).arg(&wheels).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    std::env::set_var("UV_FIND_LINKS", &wheels);
    std::env::set_var("UV_NO_INDEX", "1");
    std::env::set_var("UV_PYTHON_DOWNLOADS", "never");
    std::env::set_var("UV_PYTHON", &base);
    // Poetry's environments go to its cache: one next to the project, so the smoke can see
    // an environment outside the project without touching the user's.
    let cache = root.parent().unwrap_or(root).join(format!("{}-poetry-cache", root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    std::env::set_var("POETRY_CACHE_DIR", &cache);
    std::env::set_var("POETRY_VIRTUALENVS_IN_PROJECT", "false");
    std::env::set_var("POETRY_VIRTUALENVS_PREFER_ACTIVE_PYTHON", "false");
    std::env::set_var("POETRY_VIRTUALENVS_USE_POETRY_PYTHON", "false");
    let text = match manager {
        Manager::Uv => UV_PYPROJECT,
        Manager::Poetry => POETRY_PYPROJECT,
    };
    std::fs::write(root.join("pyproject.toml"), text).map_err(|e| e.to_string())
}

impl Workspace {
    /// BRISK_SMOKE_PYPROJ=uv|poetry on an empty folder, with uv or Poetry on PATH. Offline:
    /// local wheels only. uv: `uv sync` from Add Interpreter, then Packages adds and removes a
    /// dependency through pyproject.toml, an unrelated edit of pyproject.toml offers nothing,
    /// a new dependency offers `uv sync`, which the offer's button runs. Poetry: `poetry
    /// install` makes an environment outside the project, which becomes the interpreter and
    /// which Packages recognizes as the project's.
    pub(in crate::workspace) fn pyproj_smoke(&mut self, which: String, window: &mut Window, cx: &mut Context<Self>) {
        let manager = if which == "poetry" { Manager::Poetry } else { Manager::Uv };
        let root = self.root.clone();
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            let setup_root = root.clone();
            if let Err(e) = cx.background_executor().spawn(async move { setup(&setup_root, manager) }).await {
                eprintln!("smoke-pyproj: FAIL setup: {e}");
                let _ = this.update(cx, |_, cx| cx.quit());
                return;
            }
            let _ = this.update(cx, |this, _| this.remember_pyproject());
            let items = this
                .update(cx, |this, _| {
                    this.runs.all = Some(Vec::new());
                    this.add_interpreter_items("").into_iter().map(|i| i.label).take(2).collect::<Vec<_>>()
                })
                .unwrap_or_default();
            eprintln!("smoke-pyproj: detected {:?}, Add Interpreter starts with {items:?}", brisk_run::pyproj::detect(&root));
            let t0 = std::time::Instant::now();
            let _ = this.update(cx, |this, cx| this.sync_env(manager, cx));
            let wait_env = async |this: &gpui::WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext| {
                for _ in 0..1800 {
                    pause(100).await;
                    if this.update(cx, |t, _| !t.venv_busy).unwrap_or(true) {
                        break;
                    }
                }
            };
            wait_env(&this, cx).await;
            let (status, label, python) =
                this.update(cx, |t, _| (t.status.clone(), t.interpreter_label(), t.runs.interpreter(&t.root).map(|i| i.path))).unwrap_or_default();
            let inside = python.as_ref().is_some_and(|p| p.starts_with(&root));
            eprintln!("smoke-pyproj: sync in {:.1?} -> {status:?}", t0.elapsed());
            eprintln!("smoke-pyproj: interpreter {label:?}, inside the project {inside}, lock file {}", root.join(manager.lock_file()).is_file());

            // Packages: recognized as the project's environment.
            let _ = this.update_in(cx, |this, window, cx| this.dispatch(Action::TogglePythonPackages, window, cx));
            let wait_idle = async |this: &gpui::WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext| {
                for _ in 0..1200 {
                    pause(100).await;
                    if this
                        .update(cx, |t, _| t.packages.as_ref().is_some_and(|p| !p.loading && p.busy.is_none() && p.queue.is_empty() && p.project.is_some()))
                        .unwrap_or(false)
                    {
                        break;
                    }
                }
            };
            wait_idle(&this, cx).await;
            let tool = this.update(cx, |t, _| t.packages.as_ref().and_then(|p| p.project.clone().flatten()).map(|pt| pt.label())).ok().flatten();
            eprintln!("smoke-pyproj: Packages manages through {tool:?}");
            if manager == Manager::Poetry {
                eprintln!("smoke-pyproj: done");
                let _ = this.update(cx, |_, cx| cx.quit());
                return;
            }

            let pyproject = || std::fs::read_to_string(root.join("pyproject.toml")).unwrap_or_default();
            let names =
                |this: &mut Workspace| this.packages.as_ref().map(|p| p.list.iter().map(|x| x.name.to_lowercase()).collect::<Vec<_>>()).unwrap_or_default();
            let _ = this.update(cx, |this, cx| this.package_op(Op::Install("briskdemo".into(), String::new()), cx));
            wait_idle(&this, cx).await;
            let listed = this.update(cx, |t, _| names(t).contains(&"briskdemo".to_string())).unwrap_or(false);
            eprintln!(
                "smoke-pyproj: add briskdemo -> in pyproject.toml {}, installed {listed}, status {:?}",
                pyproject().contains("briskdemo"),
                this.update(cx, |t, _| t.status.clone()).unwrap_or_default()
            );
            let _ = this.update(cx, |this, cx| this.package_op(Op::Uninstall(Package { name: "briskdemo".into(), version: "1.0".into() }), cx));
            wait_idle(&this, cx).await;
            let listed = this.update(cx, |t, _| names(t).contains(&"briskdemo".to_string())).unwrap_or(true);
            eprintln!("smoke-pyproj: remove briskdemo -> in pyproject.toml {}, installed {listed}", pyproject().contains("briskdemo"));

            // Editing pyproject.toml in the editor: only a dependency change offers a sync.
            let file = root.join("pyproject.toml");
            let edit = async |this: &gpui::WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext, text: String| {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.open_file(&file, window, cx);
                    if let Some(ed) = this.active_editor().cloned() {
                        ed.update(cx, |e, cx| e.replace_all_text(&text, cx));
                    }
                    this.save_all(cx);
                });
                pause(300).await;
            };
            let text = pyproject();
            edit(&this, cx, format!("{text}\n[tool.ruff]\nline-length = 100\n")).await;
            eprintln!("smoke-pyproj: unrelated edit -> offer {:?}", this.update(cx, |t, _| t.venv_offer.is_some()).unwrap_or(true));
            let text = pyproject().replace("dependencies = []", "dependencies = [\"brisktwo\"]");
            edit(&this, cx, text).await;
            let offer = this.update(cx, |t, _| t.venv_offer.as_ref().map(|o| o.text())).ok().flatten();
            eprintln!("smoke-pyproj: dependency added in the editor -> offer {offer:?}");
            let _ = this.update(cx, |this, cx| this.install_requirements(cx));
            pause(200).await;
            wait_env(&this, cx).await;
            let lock = std::fs::read_to_string(root.join("uv.lock")).unwrap_or_default();
            eprintln!(
                "smoke-pyproj: after the offer's sync -> uv.lock has brisktwo {}, offer {:?}, status {:?}",
                lock.contains("brisktwo"),
                this.update(cx, |t, _| t.venv_offer.is_some()).unwrap_or(true),
                this.update(cx, |t, _| t.status.clone()).unwrap_or_default()
            );
            eprintln!("smoke-pyproj: done");
            let _ = this.update(cx, |_, cx| cx.quit());
        })
        .detach();
    }
}
