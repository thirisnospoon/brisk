//! Run icons in the editor gutter: the Run / Debug menu of a click, and pytest results
//! carried back from the Run window to the icons of the tests that ran.

use super::menu_ui::Entry;
use super::Workspace;
use crate::editor::run_marks::{scope_key, RunTarget};
use crate::editor::Editor;
use brisk_run::{RunConfig, TestStatus};
use gpui::{Context, Entity, Pixels, Point, Window};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// pytest results kept per file (normalized path) and node key (`TestA::test_b`).
pub type TestMarks = HashMap<String, HashMap<String, TestStatus>>;

pub(super) fn norm(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/").to_lowercase()
}

/// Failed beats error beats passed beats skipped: one failing parameter fails the test.
fn worse(a: TestStatus, b: TestStatus) -> TestStatus {
    let rank = |s: TestStatus| match s {
        TestStatus::Failed => 5,
        TestStatus::Error => 4,
        TestStatus::Running | TestStatus::Pending => 3,
        TestStatus::Passed => 2,
        TestStatus::Skipped => 1,
    };
    if rank(a) >= rank(b) {
        a
    } else {
        b
    }
}

/// Merge one run's leaf results (`(file, node id, status)`) into `marks`; classes get the
/// combined status of their tests.
pub fn merge_results(marks: &mut TestMarks, root: &Path, results: &[(Option<PathBuf>, String, TestStatus)]) {
    let mut fresh: TestMarks = HashMap::new();
    for (file, id, status) in results {
        if matches!(status, TestStatus::Pending | TestStatus::Running) {
            continue;
        }
        let parts = brisk_run::split_nodeid(id);
        let Some(first) = parts.first() else { continue };
        let file = match file {
            Some(f) if f.is_absolute() => f.clone(),
            Some(f) => root.join(f),
            None => root.join(first),
        };
        // `test_x[1-2]` → `test_x`.
        let scope: Vec<String> = parts[1..].iter().map(|p| p.split('[').next().unwrap_or(p).to_string()).collect();
        if scope.is_empty() {
            continue;
        }
        let entry = fresh.entry(norm(&file)).or_default();
        let key = scope.join("::");
        let s = entry.get(&key).map_or(*status, |old| worse(*old, *status));
        entry.insert(key, s);
        // Parameter sets are kept too (`test_x[1-2]`): the icon's menu offers each one.
        let full = parts[1..].join("::");
        if full != scope.join("::") {
            entry.insert(full, *status);
        }
    }
    for (file, tests) in fresh {
        let m = marks.entry(file).or_default();
        for (k, s) in tests {
            m.insert(k, s);
        }
        // Recompute classes from all their known tests.
        let mut classes: HashMap<String, TestStatus> = HashMap::new();
        for (k, s) in m.iter() {
            let parts = brisk_run::split_nodeid(k);
            if parts.len() > 1 {
                let c = classes.get(parts[0]).map_or(*s, |old| worse(*old, *s));
                classes.insert(parts[0].to_string(), c);
            }
        }
        m.extend(classes);
    }
}

impl Workspace {
    /// A click on the run icon of `line`: Run '…' / Debug '…'.
    pub(super) fn run_mark_menu(&mut self, ed: Entity<Editor>, line: usize, at: Point<Pixels>, cx: &mut Context<Self>) {
        let (path, mark) = ed.update(cx, |e, _| (e.path.clone(), e.run_marks().into_iter().find(|m| m.line == line)));
        let (Some(path), Some(mark)) = (path, mark) else { return };
        let kind = self.default_test_kind();
        let cfg = match &mark.target {
            RunTarget::Script => RunConfig::for_file(&self.root, &path),
            RunTarget::Test(scope) => RunConfig::for_tests_with(&self.root, &path, scope, kind),
        };
        let run_cfg = cfg.clone();
        let debug_cfg = cfg.clone();
        // The parameter sets of the last run of a parametrized test: ▶ on one of them.
        let params: Vec<(String, TestStatus, RunConfig)> = match &mark.target {
            RunTarget::Test(scope) if !scope.is_empty() => {
                let key = scope.join("::");
                let mut v: Vec<(String, TestStatus)> = self
                    .test_marks
                    .get(&norm(&path))
                    .map(|m| m.iter().filter(|(k, _)| k.starts_with(&format!("{key}[")) && k.ends_with(']')).map(|(k, s)| (k.clone(), *s)).collect())
                    .unwrap_or_default();
                v.sort_by(|a, b| a.0.cmp(&b.0));
                v.into_iter()
                    .map(|(k, s)| {
                        let scope: Vec<String> = brisk_run::split_nodeid(&k).into_iter().map(String::from).collect();
                        (k.rsplit("::").next().unwrap_or(&k).to_string(), s, RunConfig::for_tests_with(&self.root, &path, &scope, kind))
                    })
                    .collect()
            }
            _ => Vec::new(),
        };
        let mut entries = vec![
            Entry::Item {
                label: format!("Run '{}'", cfg.name),
                shortcut: "Ctrl+Shift+F10".into(),
                enabled: true,
                run: Rc::new(move |this: &mut Workspace, w: &mut Window, cx: &mut Context<Workspace>| this.launch_context(run_cfg.clone(), false, w, cx)),
            },
            Entry::Item {
                label: format!("Debug '{}'", cfg.name),
                shortcut: "Ctrl+Shift+F9".into(),
                enabled: true,
                run: Rc::new(move |this: &mut Workspace, w: &mut Window, cx: &mut Context<Workspace>| this.launch_context(debug_cfg.clone(), true, w, cx)),
            },
        ];
        for (label, status, c) in params {
            let glyph = match status {
                TestStatus::Passed => "✓ ",
                TestStatus::Failed | TestStatus::Error => "✗ ",
                TestStatus::Skipped => "⊘ ",
                _ => "",
            };
            entries.push(Entry::Item {
                label: format!("Run {glyph}{label}"),
                shortcut: String::new(),
                enabled: true,
                run: Rc::new(move |this: &mut Workspace, w: &mut Window, cx: &mut Context<Workspace>| this.launch_context(c.clone(), false, w, cx)),
            });
        }
        self.open_menu(at, entries, cx);
    }

    /// A pytest run finished: keep its results and show them on the open editors' icons.
    pub(super) fn take_test_results(&mut self, results: &[(Option<PathBuf>, String, TestStatus)], cx: &mut Context<Self>) {
        merge_results(&mut self.test_marks, &self.root, results);
        let eds: Vec<_> = self.editors().cloned().collect();
        for ed in eds {
            self.push_test_results(&ed, cx);
        }
    }

    pub(super) fn push_test_results(&self, ed: &Entity<Editor>, cx: &mut Context<Self>) {
        let Some(path) = ed.read(cx).path.clone() else { return };
        if let Some(m) = self.test_marks.get(&norm(&path)) {
            let m = m.clone();
            ed.update(cx, |e, cx| {
                e.set_test_results(m);
                cx.notify();
            });
        }
    }

    // --- Smoke --------------------------------------------------------------------------

    /// BRISK_SMOKE_RUNMARKS=1|hold on an empty folder: writes `app.py` (a `__main__` block)
    /// and `test_calc.py`, clicks the run icons with real mouse messages, runs a test class
    /// from the menu and checks the icons turn ✓ / ✗. The ▶ of `app.py` must run its saved
    /// configuration (its parameters reach the script), as PyCharm does.
    pub(super) fn run_marks_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let _ = std::fs::write(
            root.join("app.py"),
            "def greet(name):\n    return f\"hi {name}\"\n\n\nif __name__ == \"__main__\":\n    import sys\n    print(greet(\"run icon\"), *sys.argv[1:])\n",
        );
        let saved = RunConfig { name: "app".into(), target: "app.py".into(), parameters: "from-config".into(), ..Default::default() };
        let _ = brisk_run::save(&root, &brisk_run::RunFile { configurations: vec![saved], ..Default::default() });
        self.reload_runs();
        let _ = std::fs::write(
            root.join("test_calc.py"),
            "def add(a, b):\n    return a + b\n\n\ndef test_add():\n    assert add(2, 2) == 4\n\n\nclass TestCalc:\n    def test_ok(self):\n        assert add(1, 1) == 2\n\n    def test_broken(self):\n        assert add(1, 1) == 3\n",
        );
        let app = root.join("app.py");
        self.open_file(&app, window, cx);
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
            pause(400).await;
            // Real click on the ▶ of `if __name__ == "__main__":` (line 5), then the first item.
            let click_icon = |this: &mut Workspace, line: usize, window: &mut Window, cx: &mut Context<Workspace>| -> Option<(f32, f32)> {
                let ed = this.active_editor()?.clone();
                let (x, y) = ed.read(cx).run_icon_point(line)?;
                let scale = window.scale_factor();
                super::win_input::click(x * scale, y * scale);
                Some((x, y))
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let marks = this.active_editor().map(|e| e.update(cx, |e, _| e.run_marks())).unwrap_or_default();
                eprintln!("smoke-runmarks: app.py marks {:?}", marks.iter().map(|m| (m.line + 1, format!("{:?}", m.target))).collect::<Vec<_>>());
                let at = click_icon(this, 4, window, cx);
                eprintln!("smoke-runmarks: click ▶ at {at:?}");
            });
            let menu = |this: &Workspace| this.menu_labels();
            // The menu opens on the click's mouse-down: wait for it (up to 5 s).
            for _ in 0..50 {
                pause(100).await;
                if this.update(cx, |t, _| !t.menu_labels().is_empty()).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-runmarks: menu {:?}", menu(this));
                if std::env::var("BRISK_SMOKE_RUNMARKS").as_deref() == Ok("menu") {
                    return;
                }
                this.smoke_press("down enter", window, cx);
            });
            // The script prints and exits: wait for its output (up to 30 s; a cold debug start is slow).
            let output = |t: &Workspace, cx: &gpui::App| t.runs.panel.as_ref().and_then(|p| p.read(cx).active_text()).unwrap_or_default();
            let mut waited = 0;
            while waited < 30_000 {
                pause(100).await;
                waited += 100;
                if this.update(cx, |t, cx| output(t, cx).contains("hi run icon")).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                let out = output(this, cx);
                eprintln!("smoke-runmarks: run output has 'hi run icon': {} (after {waited} ms)", out.contains("hi run icon"));
                eprintln!("smoke-runmarks: saved configuration used: {}", out.contains("hi run icon from-config"));
                if !out.contains("hi run icon") {
                    eprintln!("smoke-runmarks: run output was {:?}, status {:?}", out.chars().take(400).collect::<String>(), this.status);
                }
                let t = this.root.join("test_calc.py");
                this.open_file(&t, window, cx);
            });
            pause(800).await;
            let _ = this.update_in(cx, |this, window, cx| {
                let marks = this.active_editor().map(|e| e.update(cx, |e, _| e.run_marks())).unwrap_or_default();
                eprintln!("smoke-runmarks: test_calc.py marks {:?}", marks.iter().map(|m| (m.line + 1, format!("{:?}", m.target))).collect::<Vec<_>>());
                // ▶ of class TestCalc (line 9).
                let at = click_icon(this, 8, window, cx);
                eprintln!("smoke-runmarks: click ▶ TestCalc at {at:?}");
            });
            // The menu opens on the click's mouse-down: wait for it (up to 5 s).
            for _ in 0..50 {
                pause(100).await;
                if this.update(cx, |t, _| !t.menu_labels().is_empty()).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-runmarks: menu {:?}", menu(this));
                this.smoke_press("down enter", window, cx);
            });
            // pytest runs; wait for the results to reach the editor.
            for _ in 0..150 {
                pause(200).await;
                if this.update(cx, |t, _| !t.test_marks.is_empty()).unwrap_or(false) {
                    break;
                }
            }
            pause(300).await;
            let _ = this.update_in(cx, |this, _, cx| {
                let ed = this.active_editor().cloned();
                let st = ed.map(|e| {
                    let e = e.read(cx);
                    [vec!["TestCalc"], vec!["TestCalc", "test_ok"], vec!["TestCalc", "test_broken"], vec!["test_add"]]
                        .iter()
                        .map(|s| {
                            let scope: Vec<String> = s.iter().map(|x| x.to_string()).collect();
                            (scope_key(&scope), e.test_status(&RunTarget::Test(scope)))
                        })
                        .collect::<Vec<_>>()
                });
                eprintln!("smoke-runmarks: statuses {st:?}");
                eprintln!("smoke-runmarks: status bar {:?}", this.status);
            });
            // Line numbers still take breakpoint clicks: a real click left of the ▶ of
            // `def test_add` (line 5) toggles a breakpoint there.
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(ed) = this.active_editor().cloned() else { return };
                // Breakpoints are kept per project path (in %APPDATA%), so a scratch folder
                // recreated at the same path brings the last run's one back: start clean,
                // or the click removes it.
                ed.update(cx, |e, cx| {
                    if e.breakpoint_list().iter().any(|(l, _)| *l == 4) {
                        e.toggle_breakpoint(4, cx);
                    }
                });
                let Some((x, y)) = ed.read(cx).run_icon_point(4) else { return };
                let scale = window.scale_factor();
                super::win_input::click((x - 18.) * scale, y * scale);
            });
            pause(500).await;
            let _ = this.update_in(cx, |this, _, cx| {
                let bps = this.active_editor().map(|e| e.read(cx).breakpoint_list().iter().map(|(l, _)| l + 1).collect::<Vec<_>>());
                eprintln!("smoke-runmarks: click on line number 5 -> breakpoints {bps:?}, menu {:?}", this.menu_labels());
                if std::env::var("BRISK_SMOKE_RUNMARKS").as_deref() != Ok("hold") {
                    cx.quit();
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_merge_by_file_and_class() {
        let root = Path::new("C:/p");
        let mut m = TestMarks::new();
        let r = |id: &str, s| (None, id.to_string(), s);
        merge_results(
            &mut m,
            root,
            &[
                r("t/test_a.py::TestX::test_ok", TestStatus::Passed),
                r("t/test_a.py::TestX::test_p[1]", TestStatus::Passed),
                r("t/test_a.py::TestX::test_p[2]", TestStatus::Failed),
                r("t/test_a.py::test_f", TestStatus::Passed),
            ],
        );
        let f = &m[&norm(&root.join("t/test_a.py"))];
        assert_eq!(f["TestX::test_ok"], TestStatus::Passed);
        assert_eq!(f["TestX::test_p"], TestStatus::Failed);
        assert_eq!(f["TestX"], TestStatus::Failed);
        assert_eq!(f["test_f"], TestStatus::Passed);
        // A later run of one test keeps the others and updates the class.
        merge_results(&mut m, root, &[r("t/test_a.py::TestX::test_p[1]", TestStatus::Passed), r("t/test_a.py::TestX::test_p[2]", TestStatus::Passed)]);
        let f = &m[&norm(&root.join("t/test_a.py"))];
        assert_eq!(f["TestX"], TestStatus::Passed);
        assert_eq!(f["test_f"], TestStatus::Passed);
    }
}
