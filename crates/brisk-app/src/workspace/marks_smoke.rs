//! BRISK_SMOKE_MARKS=1 on an empty folder (needs ty): `backend/app` is imported as `app`,
//! which only resolves once `backend` is a sources root. Checks ty's diagnostics before and
//! after marking it, a run that imports it, then marks `data` excluded and checks the tree,
//! the file index and the saved `.brisk/structure.json`.

use super::Workspace;
use brisk_project::Mark;
use gpui::{Context, Window};

impl Workspace {
    pub(super) fn marks_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let _ = std::fs::create_dir_all(root.join("backend/app"));
        let _ = std::fs::create_dir_all(root.join("data"));
        let _ = std::fs::write(root.join("backend/app/__init__.py"), "def hello() -> int:\n    return 42\n");
        let _ = std::fs::write(root.join("data/notes.txt"), "needle\n");
        let main = root.join("main.py");
        let _ = std::fs::write(&main, "import app\n\nx: str = app.hello()\nprint('hello', app.hello())\n");
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            let codes = |this: &Workspace, cx: &gpui::App| -> Vec<String> {
                this.active_editor().map(|e| e.read(cx).diagnostics.iter().map(|d| d.code.clone()).collect()).unwrap_or_default()
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.reindex_files(cx);
                this.tree.reload(0);
                this.open_file(&main, window, cx);
            });
            let mut before = Vec::new();
            for _ in 0..150 {
                pause(100).await;
                before = this.update(cx, |t, cx| codes(t, cx)).unwrap_or_default();
                if before.iter().any(|c| c == "import-not-found" || c == "unresolved-import") {
                    break;
                }
            }
            eprintln!("smoke-marks: before -> {before:?}");
            let _ = this.update(cx, |this, cx| {
                let id = this.tree.find(&this.root.join("backend"));
                let labels = id.and_then(|_| this.mark_entries(&this.root.join("backend"), true, false)).map(|e| e.labels()).unwrap_or_default();
                eprintln!("smoke-marks: menu {labels:?}");
                this.mark_dir(&root.join("backend"), Some(Mark::Sources), cx);
            });
            let mut after = Vec::new();
            for _ in 0..150 {
                pause(100).await;
                // A new ty asks for the open files again after it starts.
                let _ = this.update_in(cx, |this, window, cx| this.open_file(&main, window, cx));
                after = this.update(cx, |t, cx| codes(t, cx)).unwrap_or_default();
                if after.iter().any(|c| c == "invalid-assignment") {
                    break;
                }
            }
            eprintln!("smoke-marks: after marking backend -> {after:?}, status {:?}", this.update(cx, |t, _| t.status.clone()).ok());
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(cfg) = this.context_config(window, cx) {
                    this.run_config(cfg, window, cx);
                }
            });
            for _ in 0..150 {
                pause(100).await;
                let out = this.update(cx, |t, cx| t.runs.panel.as_ref().and_then(|p| p.read(cx).active_text())).ok().flatten().unwrap_or_default();
                if out.contains("Process finished") {
                    eprintln!(
                        "smoke-marks: run -> {:?}",
                        out.lines().filter(|l| l.starts_with("hello") || l.contains("Error") || l.starts_with("Process")).collect::<Vec<_>>()
                    );
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| this.mark_dir(&root.join("data"), Some(Mark::Excluded), cx));
            for _ in 0..50 {
                pause(100).await;
                if this.update(cx, |t, _| t.files.as_ref().is_some_and(|f| !f.files.iter().any(|p| p.starts_with("data/")))).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| {
                let files = this.files.as_ref().map(|f| f.files.clone()).unwrap_or_default();
                let data = this.tree.find(&this.root.join("data"));
                eprintln!(
                    "smoke-marks: data excluded in tree {:?}, mark {:?}, indexed {:?}",
                    data.map(|d| this.tree.nodes[d].excluded),
                    data.and_then(|d| this.tree.mark(d)),
                    files.iter().filter(|p| !p.starts_with(".brisk")).collect::<Vec<_>>()
                );
                eprintln!("smoke-marks: saved {:?}", std::fs::read_to_string(brisk_project::Structure::file(&this.root)).unwrap_or_default());
                cx.quit();
            });
        })
        .detach();
    }
}
