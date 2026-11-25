//! BRISK_SMOKE_RUNTERM=1 on an empty folder: the Run toolbar's "Copy command line" puts the
//! run's command on the clipboard, and "Open in Terminal" runs the same command (its working
//! directory, variables and arguments) in a project terminal tab.

use super::Workspace;
use brisk_run::RunConfig;
use gpui::{Context, Window};

impl Workspace {
    pub(super) fn run_term_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let _ = std::fs::create_dir_all(root.join("sub dir"));
        let _ = std::fs::write(
            root.join("argv.py"),
            "import os, sys\nprint('ARGV', sys.argv[1:], 'CWD', os.path.basename(os.getcwd()), 'V', os.environ.get('RT_VAR'))\n",
        );
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.runs.interpreter(&t.root).is_some()).unwrap_or(false) {
                    break;
                }
            }
            let cfg = RunConfig {
                name: "argv".into(),
                target: "argv.py".into(),
                parameters: "one \"two words\" it's".into(),
                working_directory: "sub dir".into(),
                env: [("RT_VAR".to_string(), "o'k".to_string())].into_iter().collect(),
                ..Default::default()
            };
            let _ = this.update_in(cx, |this, window, cx| this.run_config(cfg, window, cx));
            let mut run_out = String::new();
            for _ in 0..100 {
                pause(100).await;
                run_out = this.update(cx, |t, cx| t.runs.panel.as_ref().and_then(|p| p.read(cx).active_text()).unwrap_or_default()).unwrap_or_default();
                if run_out.contains("Process finished") {
                    break;
                }
            }
            let run_line = run_out.lines().find(|l| l.starts_with("ARGV")).unwrap_or("").to_string();
            // The two toolbar buttons.
            let _ = this.update_in(cx, |this, _, cx| {
                let Some(p) = this.runs.panel.clone() else { return };
                p.update(cx, |p, cx| {
                    p.copy_command_line(cx);
                    p.open_in_terminal(cx);
                });
                let clip = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
                let header = run_out.lines().next().unwrap_or("").to_string();
                eprintln!("smoke-runterm: clipboard is the header {}, {clip:?}", clip == header);
            });
            let mut term = String::new();
            for _ in 0..150 {
                pause(100).await;
                term = this.update(cx, |t, cx| t.terminal.as_ref().and_then(|p| p.read(cx).active_text()).unwrap_or_default()).unwrap_or_default();
                if term.lines().any(|l| l.starts_with("ARGV")) {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| {
                let term_line = term.lines().find(|l| l.starts_with("ARGV")).unwrap_or("");
                eprintln!(
                    "smoke-runterm: run {run_line:?}, terminal {term_line:?}, same {}, bottom {:?}",
                    !run_line.is_empty() && run_line == term_line.trim_end(),
                    this.bottom
                );
                cx.quit();
            });
        })
        .detach();
    }
}
