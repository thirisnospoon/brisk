//! BRISK_SMOKE_SQUIGGLE=1 on an empty folder (writes `squiggle.py`; needs ruff): ruff warns about
//! a tuple written on several lines (RUF023); the smoke adds a deprecated call the way ty reports
//! it and prints where each wave goes (only over code, never the indentation). `=hold`
//! stays open for screenshots (with the Light theme when the settings say so).

use super::Workspace;
use brisk_lint::{Diagnostic, Pos, Severity};
use gpui::{Context, Window};
use std::time::Duration;

const SAMPLE: &str = "import os\nimport warnings\n\n\nclass Field:\n    __slots__ = ('name',\n                 'type',\n                 'default',\n                 )\n\n    def old(self):\n        return os.getcwd()\n\n\ndef main():\n    f = Field()\n    f.old()\n    value = undefined_name + 1\n    return value\n";

impl Workspace {
    pub(super) fn squiggle_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let file = self.root.join("squiggle.py");
        let _ = std::fs::write(&file, SAMPLE);
        self.open_file(&file, window, cx);
        let hold = std::env::var("BRISK_SMOKE_SQUIGGLE").as_deref() == Ok("hold");
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            for _ in 0..200 {
                bg.timer(Duration::from_millis(50)).await;
                if this.update(cx, |t, cx| t.active_editor().is_some_and(|e| e.read(cx).diag_version == e.read(cx).buffer.version())).unwrap_or(true) {
                    break;
                }
            }
            let Ok(Some(ed)) = this.update(cx, |t, _| t.active_editor().cloned()) else { return };
            let _ = ed.update(cx, |e, cx| {
                let d = |code: &str, message: &str, severity, start: (usize, usize), end: (usize, usize)| Diagnostic {
                    file: file.clone(),
                    code: code.into(),
                    message: message.into(),
                    severity,
                    start: Pos { line: start.0, col: start.1 },
                    end: Pos { line: end.0, col: end.1 },
                    fix: None,
                    noqa_line: None,
                };
                e.diagnostics.push(d("deprecated", "The function `old` is deprecated", Severity::Warning, (16, 6), (16, 9)));
                cx.notify();
            });
            bg.timer(Duration::from_millis(300)).await;
            let waves = ed.read_with(cx, |e, _| e.waves_for_smoke()).unwrap_or_default();
            for (code, rows) in &waves {
                eprintln!("smoke-squiggle: {code} -> {rows:?} (line, from column, to column)");
            }
            let codes = ed.read_with(cx, |e, _| e.diagnostics.iter().map(|d| format!("{:?} {}", d.severity, d.code)).collect::<Vec<_>>()).unwrap_or_default();
            eprintln!("smoke-squiggle: problems {codes:?}");
            if hold {
                eprintln!("smoke-squiggle: holding");
                return;
            }
            eprintln!("smoke-squiggle: done");
            let _ = this.update(cx, |_, cx| cx.quit());
        })
        .detach();
    }
}
