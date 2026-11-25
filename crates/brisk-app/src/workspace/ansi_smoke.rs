//! BRISK_SMOKE_ANSI=1 on an empty folder: Run output with 256-color and truecolor escapes,
//! OSC 8 hyperlinks (a `file://` one opens the file at its line, an `https://` one the
//! browser) and `x.py:N:M` links that put the caret on the column.

use super::Workspace;
use brisk_run::RunConfig;
use gpui::{Context, Window};

const SCRIPT: &str = r#"import os, sys
target = os.path.abspath('target.py').replace('\\', '/')
print('\x1b[38;5;196mRED256\x1b[0m \x1b[38;5;46mGREEN256\x1b[0m \x1b[38;5;244mGRAY256\x1b[0m')
print('\x1b[38;2;10;200;30mTRUECOLOR\x1b[0m \x1b[48;2;200;10;30mONBG\x1b[0m')
print(f'\x1b]8;;file:///{target.lstrip("/")}#L3\x1b\\open target\x1b]8;;\x1b\\ and \x1b]8;;https://example.com/docs\x1b\\the docs\x1b]8;;\x1b\\')
print('target.py:4:9: error: something')
"#;

impl Workspace {
    pub(super) fn ansi_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let _ = std::fs::write(root.join("ansi.py"), SCRIPT);
        let _ = std::fs::write(root.join("target.py"), "a = 1\nb = 2\nc = 3\nvalue = compute(1)\n");
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.runs.interpreter(&t.root).is_some()).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.run_config(RunConfig { name: "ansi".into(), target: "ansi.py".into(), ..Default::default() }, window, cx)
            });
            for _ in 0..100 {
                pause(100).await;
                if this
                    .update(cx, |t, cx| t.runs.panel.as_ref().and_then(|p| p.read(cx).active_text()).unwrap_or_default().contains("Process finished"))
                    .unwrap_or(false)
                {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(p) = this.runs.panel.clone() else { return };
                let pr = p.read(cx);
                let colors: Vec<_> = ["RED256", "GREEN256", "GRAY256", "TRUECOLOR"].iter().map(|n| (*n, pr.fg_of(n))).collect();
                eprintln!("smoke-ansi: colors {colors:?}");
                eprintln!("smoke-ansi: hyperlinks {:?}", pr.hyperlinks().iter().map(|u| u.rsplit('/').next().unwrap_or("").to_string()).collect::<Vec<_>>());
                eprintln!("smoke-ansi: text {:?}", pr.active_text().unwrap_or_default().lines().skip(1).take(4).collect::<Vec<_>>());
                // OSC 8 file link: opens target.py at line 3.
                p.update(cx, |p, cx| p.click_first(Some("file:"), cx));
                let _ = window;
            });
            let at = |this: &Workspace, cx: &gpui::App| {
                this.active_editor().map(|e| {
                    let e = e.read(cx);
                    (e.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()), e.caret_line_col())
                })
            };
            pause(200).await;
            let _ = this.update(cx, |this, cx| {
                eprintln!("smoke-ansi: osc8 file click -> {:?}", at(this, cx));
                // `target.py:4:9`: line 4, column 9.
                if let Some(p) = this.runs.panel.clone() {
                    p.update(cx, |p, cx| p.click_first(None, cx));
                }
            });
            pause(200).await;
            let _ = this.update(cx, |this, cx| eprintln!("smoke-ansi: path:line:col click -> {:?}", at(this, cx)));
            // The https one goes to the browser (recorded, not opened, in smokes).
            let _ = this.update(cx, |this, cx| {
                if let Some(p) = this.runs.panel.clone() {
                    p.update(cx, |p, cx| p.click_first(Some("https:"), cx));
                }
            });
            pause(100).await;
            let _ = this.update(cx, |this, cx| {
                eprintln!("smoke-ansi: https link -> {:?}", this.last_url);
                cx.quit();
            });
        })
        .detach();
    }
}
