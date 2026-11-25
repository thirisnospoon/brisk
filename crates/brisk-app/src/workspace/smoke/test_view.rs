//! BRISK_SMOKE_TESTVIEW=1 on an empty folder: the test tree's Show Passed / Show Ignored /
//! Sort by Duration, parameter sets under their function, Export Test Results (HTML), the
//! Test History, and Rerun Failed Tests First.

use crate::workspace::Workspace;
use brisk_run::{RunConfig, RunKind};
use gpui::{Context, Window};

const TESTS: &str = "import time, pytest

def mark(name):
    with open('order.txt', 'a') as f:
        f.write(name + '\\n')

def test_fast():
    mark('fast')

def test_slow():
    mark('slow')
    time.sleep(0.3)

@pytest.mark.parametrize('n', [1, 2, 3])
def test_param(n):
    mark(f'param{n}')
    assert n != 2

@pytest.mark.skip(reason='later')
def test_skipped():
    pass

def test_broken():
    mark('broken')
    assert 1 == 2
";

impl Workspace {
    pub(in crate::workspace) fn test_view_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let _ = std::fs::write(root.join("test_view.py"), TESTS);
        let _ = std::fs::remove_dir_all(brisk_run::pytest::history::dir(&root));
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.runs.interpreter(&t.root).is_some()).unwrap_or(false) {
                    break;
                }
            }
            let cfg = RunConfig { name: "view tests".into(), kind: RunKind::Pytest, target: "test_view.py".into(), ..Default::default() };
            let finished = async |this: &gpui::WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext| {
                for _ in 0..200 {
                    pause(100).await;
                    let done = this
                        .update(cx, |t, cx| t.runs.panel.as_ref().and_then(|p| p.read(cx).active_text()).unwrap_or_default().contains("Process finished"))
                        .unwrap_or(false);
                    if done {
                        return true;
                    }
                }
                false
            };
            let _ = this.update_in(cx, |this, window, cx| this.run_config(cfg.clone(), window, cx));
            let ok = finished(&this, cx).await;
            let tree = |this: &Workspace, cx: &gpui::App| -> Vec<String> {
                let text = this.runs.panel.as_ref().and_then(|p| p.read(cx).tests_tree_text()).unwrap_or_default();
                text.lines().skip(1).map(|l| l.trim_end().to_string()).collect()
            };
            let _ = this.update(cx, |this, cx| {
                let Some(p) = this.runs.panel.clone() else { return };
                eprintln!("smoke-testview: finished {ok}, tree {:?}", tree(this, cx));
                for (what, f) in [
                    ("hide passed", (|v: &mut brisk_run::TreeView| v.show_passed = false) as fn(&mut brisk_run::TreeView)),
                    ("hide ignored", |v| v.show_ignored = false),
                    ("all, by duration", |v| *v = brisk_run::TreeView { by_duration: true, ..Default::default() }),
                ] {
                    p.update(cx, |p, _| p.set_tests_view(f));
                    eprintln!("smoke-testview: {what} -> {:?}", tree(this, cx));
                }
                p.update(cx, |p, _| p.set_tests_view(|v| *v = Default::default()));
                match p.read(cx).export_tests() {
                    Ok(path) => {
                        let html = std::fs::read_to_string(&path).unwrap_or_default();
                        eprintln!(
                            "smoke-testview: export {:?} -> summary {}, failure {}, params {}",
                            path.strip_prefix(&this.root).unwrap_or(&path),
                            html.contains("Tests failed: 2, passed: 4, ignored: 1 of 7 tests"),
                            html.contains("assert 1 == 2"),
                            html.contains("✗ [2]")
                        );
                    }
                    Err(e) => eprintln!("smoke-testview: export failed {e}"),
                }
                let h = brisk_run::pytest::history::list(&this.root);
                eprintln!("smoke-testview: history {:?}", h.iter().map(|e| e.name.clone()).collect::<Vec<_>>());
                let _ = std::fs::remove_file(this.root.join("order.txt"));
                p.update(cx, |p, _| p.failed_first = true);
            });
            if std::env::var("BRISK_SMOKE_TESTVIEW").as_deref() == Ok("shot") {
                // Left open for a screenshot: the history list over the tree.
                let _ = this.update(cx, |this, cx| {
                    if let Some(p) = this.runs.panel.clone() {
                        p.update(cx, |p, cx| {
                            p.history_open = true;
                            cx.notify();
                        });
                    }
                });
                return;
            }
            // Rerun with the failures first.
            let _ = this.update_in(cx, |this, window, cx| this.run_config(cfg.clone(), window, cx));
            finished(&this, cx).await;
            let _ = this.update(cx, |this, cx| {
                let order = std::fs::read_to_string(this.root.join("order.txt")).unwrap_or_default();
                eprintln!("smoke-testview: failed first -> order {:?}", order.lines().collect::<Vec<_>>());
                let Some(p) = this.runs.panel.clone() else { return };
                let h = brisk_run::pytest::history::list(&this.root);
                eprintln!("smoke-testview: history now {}", h.len());
                if let Some(e) = h.last() {
                    let r = p.update(cx, |p, _| p.open_history(e));
                    let tabs = p.read(cx).run_tabs();
                    eprintln!(
                        "smoke-testview: open history {r:?} -> tabs {tabs:?}, summary {:?}, selected {:?}",
                        p.read(cx).tests_summary(),
                        p.read(cx).selected_test().map(|s| s.0)
                    );
                    eprintln!("smoke-testview: history console {:?}", p.read(cx).active_text().map(|t| t.lines().take(2).collect::<Vec<_>>().join(" | ")));
                }
                cx.quit();
            });
        })
        .detach();
    }
}
