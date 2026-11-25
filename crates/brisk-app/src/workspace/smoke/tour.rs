//! BRISK_SMOKE_TOUR=1 on a project with `app/main.py` (examples/demo_project under git):
//! walks through the main windows and popups one by one, printing
//! "tour: <n> <name>" as each is shown, so a screenshot can be taken of each (see
//! BRISK_TOUR_SYNC below). For reviewing the UI, not a pass/fail check.

use crate::workspace::{PopupKind, SeTab, Workspace};
use brisk_keymap::Action;
use gpui::{Context, Window};

const STEPS: &[&str] = &[
    "editor",
    "search_everywhere",
    "find_action",
    "recent_files",
    "settings",
    "commit",
    "git_log",
    "run",
    "terminal",
    "problems",
    "python_packages",
    "python_console",
    "database",
    "run_configurations",
    "interpreters",
    "vcs_operations",
    "tree_menu",
    "find_in_files",
    "find_in_dir",
    "tests_menu",
    "status_history",
    "debug",
    "light_theme",
];

impl Workspace {
    fn tour_reset(&mut self) {
        self.popup = None;
        self.menu = None;
        self.settings_dialog = None;
        self.runs.dialog = None;
        if self.find.open {
            self.find.open = false;
        }
    }

    fn tour_step(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.tour_reset();
        let query = |this: &mut Workspace, kind: PopupKind, q: &str| {
            this.open_popup(kind);
            if let Some(p) = this.popup.as_mut() {
                p.query = q.into();
            }
            this.refresh_popup();
        };
        match name {
            "editor" => {
                let p = self.root.join("app/main.py");
                self.open_file(&p, window, cx);
            }
            "search_everywhere" => query(self, PopupKind::SearchEverywhere(SeTab::All), "user"),
            "find_action" => query(self, PopupKind::FindAction, "run"),
            "recent_files" => self.dispatch(Action::RecentFiles, window, cx),
            "settings" => self.dispatch(Action::Settings, window, cx),
            "commit" => self.dispatch(Action::ToggleCommitWindow, window, cx),
            "git_log" => self.dispatch(Action::ToggleGitWindow, window, cx),
            "run" => {
                let p = self.root.join("app/main.py");
                self.open_file(&p, window, cx);
                self.dispatch(Action::RunContext, window, cx);
            }
            "terminal" => self.dispatch(Action::ToggleTerminalWindow, window, cx),
            "problems" => self.dispatch(Action::ToggleProblemsWindow, window, cx),
            "python_packages" => self.dispatch(Action::TogglePythonPackages, window, cx),
            "python_console" => self.dispatch(Action::TogglePythonConsole, window, cx),
            "database" => self.dispatch(Action::ToggleDatabaseWindow, window, cx),
            "run_configurations" => self.dispatch(Action::EditRunConfigurations, window, cx),
            "interpreters" => self.open_popup(PopupKind::Interpreters),
            "vcs_operations" => self.dispatch(Action::VcsQuickList, window, cx),
            "tree_menu" => {
                self.dispatch(Action::ToggleProjectWindow, window, cx);
                if let Some(id) = self.tree.find(&self.root.join("app")) {
                    self.tree_menu(id, gpui::point(gpui::px(140.), gpui::px(135.)), window, cx);
                }
            }
            "find_in_files" => self.dispatch(Action::FindInFiles, window, cx),
            "find_in_dir" => {
                let d = self.root.join("app");
                self.find_in_dir(&d, window, cx);
            }
            "tests_menu" => {
                if let Some(id) = self.tree.find(&self.root.join("tests")) {
                    self.tree_menu(id, gpui::point(gpui::px(140.), gpui::px(185.)), window, cx);
                }
            }
            "status_history" => self.open_popup(PopupKind::StatusHistory),
            "debug" => {
                let p = self.root.join("app/main.py");
                self.open_file(&p, window, cx);
                self.dispatch(Action::ToggleBreakpoint, window, cx);
                self.dispatch(Action::DebugContext, window, cx);
            }
            "light_theme" => self.dispatch(Action::QuickSwitchTheme, window, cx),
            _ => {}
        }
        cx.notify();
    }

    pub(in crate::workspace) fn tour_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            bg.timer(std::time::Duration::from_millis(1500)).await;
            for (i, name) in STEPS.iter().enumerate() {
                let _ = this.update_in(cx, |this, window, cx| {
                    eprintln!("tour: {i:02} {name}");
                    this.tour_step(name, window, cx);
                });
                bg.timer(std::time::Duration::from_millis(1800)).await;
                // With BRISK_TOUR_SYNC=<dir>: "<n>.ready" asks for the screenshot, and the
                // next step waits for "<n>.done" (or 15 s).
                if let Some(dir) = std::env::var_os("BRISK_TOUR_SYNC").map(std::path::PathBuf::from) {
                    let _ = std::fs::write(dir.join(format!("{i:02}.ready")), name);
                    for _ in 0..150 {
                        if dir.join(format!("{i:02}.done")).exists() {
                            break;
                        }
                        bg.timer(std::time::Duration::from_millis(100)).await;
                    }
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.dispatch(Action::QuickSwitchTheme, window, cx);
                cx.quit();
            });
        })
        .detach();
    }
}
