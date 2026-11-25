//! BRISK_SMOKE_RUNTPL=1|dialog on an empty folder: Edit Configuration Templates — the Python
//! template gets a working directory and a variable by typing, is saved to run.json, and both a
//! new configuration from "+" and the temporary one of Ctrl+Shift+F10 start from it; pytest's
//! template stays empty.

use crate::workspace::Workspace;
use gpui::{Context, Window};

impl Workspace {
    pub(in crate::workspace) fn run_tpl_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let _ = std::fs::create_dir_all(root.join("sub"));
        let _ = std::fs::write(root.join("show.py"), "import os\nprint('cwd', os.path.basename(os.getcwd()), 'TPL', os.environ.get('TPL'))\n");
        self.dispatch(brisk_keymap::Action::EditRunConfigurations, window, cx);
        if let Some(d) = self.runs.dialog.as_mut() {
            d.toggle_templates();
        }
        let shown =
            |this: &Workspace| this.runs.dialog.as_ref().map(|d| (d.templates, d.drafts.iter().map(|x| x.cfg.name.clone()).collect::<Vec<_>>(), d.field));
        eprintln!("smoke-runtpl: templates -> {:?}", shown(self));
        self.smoke_press("tab", window, cx); // Working directory
        self.rc_type("sub", window, cx);
        self.smoke_press("tab", window, cx); // Environment
        self.rc_type("TPL=1", window, cx);
        if std::env::var("BRISK_SMOKE_RUNTPL").as_deref() == Ok("dialog") {
            return;
        }
        self.smoke_press("enter", window, cx);
        let file = brisk_run::load(&root).0;
        eprintln!(
            "smoke-runtpl: saved templates {:?}",
            file.templates.iter().map(|t| (t.name.clone(), t.working_directory.clone(), t.env.clone())).collect::<Vec<_>>()
        );
        // "+" (Alt+Insert, Enter: Python script) and a pytest one.
        self.dispatch(brisk_keymap::Action::EditRunConfigurations, window, cx);
        self.smoke_press("alt-insert enter", window, cx);
        let new = self.runs.dialog.as_ref().and_then(|d| d.drafts.get(d.current)).map(|x| (x.cfg.working_directory.clone(), x.env.clone()));
        self.smoke_press("alt-insert down down enter", window, cx);
        let pytest = self.runs.dialog.as_ref().and_then(|d| d.drafts.get(d.current)).map(|x| (x.cfg.kind, x.cfg.working_directory.clone(), x.env.clone()));
        eprintln!("smoke-runtpl: new Python {new:?}, new pytest {pytest:?}");
        self.smoke_press("escape", window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.runs.interpreter(&t.root).is_some()).unwrap_or(false) {
                    break;
                }
            }
            // Ctrl+Shift+F10 on show.py: a temporary configuration from the template.
            let _ = this.update_in(cx, |this, window, cx| {
                this.open_file(&this.root.join("show.py"), window, cx);
                this.dispatch(brisk_keymap::Action::RunContext, window, cx);
            });
            let mut out = String::new();
            for _ in 0..100 {
                pause(100).await;
                out = this.update(cx, |t, cx| t.runs.panel.as_ref().and_then(|p| p.read(cx).active_text()).unwrap_or_default()).unwrap_or_default();
                if out.contains("Process finished") {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| {
                let tmp = this.runs.file.get("show").map(|c| (c.temporary, c.working_directory.clone(), c.env.clone()));
                eprintln!("smoke-runtpl: temporary {tmp:?}, output {:?}", out.lines().find(|l| l.starts_with("cwd")));
                cx.quit();
            });
        })
        .detach();
    }
}
