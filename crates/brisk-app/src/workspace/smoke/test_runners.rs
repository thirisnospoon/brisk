//! BRISK_SMOKE_TESTRUNNERS=1 on an empty folder: ▶ on one parameter set of a parametrized
//! test (in the tree and in the gutter icon's menu), the Default test runner (unittest for
//! Ctrl+Shift+F10, chosen in the templates view of Edit Configurations) and a doctest run.

use crate::workspace::Workspace;
use brisk_run::{RunConfig, RunKind};
use gpui::{Context, Window};

impl Workspace {
    pub(in crate::workspace) fn test_runners_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let _ = std::fs::write(
            root.join("test_p.py"),
            "import pytest\n\n\n@pytest.mark.parametrize('n', [1, 2, 3])\ndef test_x(n):\n    print('param', n)\n    assert n != 2\n",
        );
        let _ = std::fs::write(
            root.join("test_u.py"),
            "import unittest\n\n\nclass TestU(unittest.TestCase):\n    def test_ok(self):\n        self.assertTrue(True)\n\n    def test_bad(self):\n        self.assertEqual(1, 2)\n",
        );
        let _ = std::fs::write(root.join("mathx.py"), "def double(x):\n    \"\"\"\n    >>> double(2)\n    4\n    \"\"\"\n    return x * 2\n");
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, cx| t.interpreter(cx).is_some()).unwrap_or(false) {
                    break;
                }
            }
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
            let state = |this: &Workspace, cx: &gpui::App| {
                let p = this.runs.panel.as_ref().map(|p| p.read(cx));
                let tabs = p.map(|p| p.run_tabs().into_iter().map(|t| t.0).collect::<Vec<_>>()).unwrap_or_default();
                let summary = p.and_then(|p| p.tests_summary()).unwrap_or_default();
                let tree: Vec<String> = p.and_then(|p| p.tests_tree_text()).unwrap_or_default().lines().skip(1).map(|l| l.trim_end().to_string()).collect();
                format!("tabs {tabs:?}, {summary:?}, tree {tree:?}")
            };
            // pytest: the whole file, then ▶ on the [2] node (Ctrl+Shift+F10 in the tree).
            let _ = this.update_in(cx, |this, window, cx| {
                this.run_config(RunConfig { name: "params".into(), kind: RunKind::Pytest, target: "test_p.py".into(), ..Default::default() }, window, cx)
            });
            finished(&this, cx).await;
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-testrunners: params -> {}", state(this, cx));
                let Some(p) = this.runs.panel.clone() else { return };
                window.focus(&gpui::Focusable::focus_handle(p.read(cx), cx));
                let sel = p.update(cx, |p, _| p.select_test_named("[2]"));
                let k = gpui::Keystroke::parse("ctrl-shift-f10").unwrap();
                p.update(cx, |p, cx| p.key(&k, cx));
                eprintln!("smoke-testrunners: selected {sel:?}, ctrl+shift+f10 in the tree");
            });
            pause(300).await;
            finished(&this, cx).await;
            let _ = this.update(cx, |this, cx| {
                let tmp = this.runs.file.configurations.iter().filter(|c| c.temporary).map(|c| (c.name.clone(), c.target.clone())).collect::<Vec<_>>();
                eprintln!("smoke-testrunners: one parameter -> {}, temporary {tmp:?}", state(this, cx));
            });
            // The gutter icon of test_x lists the parameter sets of the last run.
            let _ = this.update_in(cx, |this, window, cx| {
                this.open_file(&this.root.join("test_p.py"), window, cx);
                if let Some(ed) = this.active_editor().cloned() {
                    this.run_mark_menu(ed, 4, gpui::point(gpui::px(100.), gpui::px(100.)), cx);
                }
                eprintln!("smoke-testrunners: gutter menu {:?}", this.menu_labels());
                this.menu = None;
            });
            // Default test runner: Unittests, picked in the templates view and saved.
            let _ = this.update_in(cx, |this, window, cx| {
                this.dispatch(brisk_keymap::Action::EditRunConfigurations, window, cx);
                if let Some(d) = this.runs.dialog.as_mut() {
                    d.toggle_templates();
                    d.cycle_test_runner();
                    d.cycle_test_runner();
                }
                this.smoke_press("enter", window, cx);
                let saved = brisk_run::load(&this.root).0.test_runner;
                eprintln!("smoke-testrunners: default test runner saved {saved:?}, kind {:?}", this.default_test_kind(cx));
                this.open_file(&this.root.join("test_u.py"), window, cx);
                this.dispatch(brisk_keymap::Action::RunContext, window, cx);
            });
            finished(&this, cx).await;
            let _ = this.update(cx, |this, cx| {
                let first = this.runs.panel.as_ref().and_then(|p| p.read(cx).active_text()).unwrap_or_default().lines().next().unwrap_or("").to_string();
                eprintln!(
                    "smoke-testrunners: ctrl+shift+f10 on test_u.py -> {}, command ends {:?}",
                    state(this, cx),
                    first.rsplit(".exe").next().unwrap_or("")
                );
            });
            // Doctest.
            let _ = this.update_in(cx, |this, window, cx| {
                this.run_config(RunConfig { name: "doc".into(), kind: RunKind::Doctest, target: "mathx.py".into(), ..Default::default() }, window, cx)
            });
            finished(&this, cx).await;
            let _ = this.update(cx, |this, cx| {
                eprintln!("smoke-testrunners: doctest -> {}", state(this, cx));
                cx.quit();
            });
        })
        .detach();
    }
}
