//! What the Services tool window (Run Dashboard) needs from the Run window: every Run tab
//! with its status, and Stop / select by tab.

use super::TerminalPanel;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct RunInfo {
    pub id: u64,
    pub name: String,
    pub running: bool,
    /// Exit code; `None` while running or when it was killed.
    pub code: Option<i32>,
    /// Run time so far, or until it finished.
    pub elapsed: Duration,
    /// pytest: the summary line ("Tests passed: 3").
    pub tests: Option<String>,
    pub active: bool,
}

impl TerminalPanel {
    /// Every Run tab, in tab order.
    pub fn runs_info(&self) -> Vec<RunInfo> {
        self.tabs
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                let r = t.run.as_ref()?;
                Some(RunInfo {
                    id: t.id,
                    name: r.name.clone(),
                    running: r.running,
                    code: r.code,
                    elapsed: r.ended.unwrap_or_else(std::time::Instant::now).duration_since(r.started),
                    tests: r.tests.as_ref().filter(|t| t.finished).map(|t| t.tree().tree.summary(true)),
                    active: i == self.active,
                })
            })
            .collect()
    }

    /// Show a tab (Services selects it to show its console).
    pub fn select_run(&mut self, id: u64) -> bool {
        match self.tabs.iter().position(|t| t.id == id) {
            Some(i) => {
                self.active = i;
                true
            }
            None => false,
        }
    }

    /// A run of configuration `name` is still running.
    pub fn running_named(&self, name: &str) -> bool {
        self.tabs.iter().any(|t| t.run.as_ref().is_some_and(|r| r.running && r.name == name))
    }

    /// Stop every running run of configuration `name` ("Stop and Rerun").
    pub fn stop_named(&mut self, name: &str) {
        for t in self.tabs.iter_mut().filter(|t| t.run.as_ref().is_some_and(|r| r.running && r.name == name)) {
            t.stop_run();
        }
    }

    /// Stop one tab's process.
    pub fn stop_run(&mut self, id: u64) -> bool {
        match self.tabs.iter_mut().find(|t| t.id == id && t.run.as_ref().is_some_and(|r| r.running)) {
            Some(t) => {
                t.stop_run();
                true
            }
            None => false,
        }
    }
}
