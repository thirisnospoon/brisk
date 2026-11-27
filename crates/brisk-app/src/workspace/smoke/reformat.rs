//! `BRISK_SMOKE_REFORMAT=1` (on an empty scratch folder): Reformat Code (`Ctrl+Alt+L`) on
//! files that are not Python. Writes minified JSON, CSS, HTML, SQL and TOML, reformats each
//! in its editor and checks the text, that formatting again changes nothing, and that a
//! broken file stays as it is. With `BRISK_TSC` set, also a TypeScript file through
//! TypeScript's server. Exits with code 1 on a failure.

use crate::workspace::Workspace;
use brisk_keymap::Action;
use gpui::{AsyncWindowContext, Context, Window};
use std::time::Duration;

fn wait(cx: &AsyncWindowContext, ms: u64) -> gpui::Task<()> {
    cx.background_executor().timer(Duration::from_millis(ms))
}

fn check(ok: bool, what: &str, got: impl std::fmt::Debug) {
    if ok {
        eprintln!("smoke-reformat: ok {what}");
    } else {
        eprintln!("smoke-reformat: FAIL {what}: {got:?}");
        std::process::exit(1);
    }
}

const CASES: &[(&str, &str, &str)] = &[
    ("a.json", "{\"a\":1,\"b\":[1,2]}", "{\n    \"a\": 1,\n    \"b\": [\n        1,\n        2\n    ]\n}\n"),
    ("s.css", "a{color:red}", "a {\n    color: red;\n}\n"),
    ("p.html", "<div><p>Hi</p></div>", "<div>\n    <p>Hi</p>\n</div>\n"),
    ("q.sql", "select a, b from t where x = 1 and y = 2;", "select a,\n    b\nfrom t\nwhere x = 1\n    and y = 2;\n"),
    ("c.toml", "[ a ]\nx=1\n", "[a]\nx = 1\n"),
    ("bad.json", "{\"a\": [1, 2}", "{\"a\": [1, 2}"),
];

impl Workspace {
    pub(in crate::workspace) fn reformat_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let mut cases: Vec<(String, String, String)> = CASES.iter().map(|(f, t, w)| (f.to_string(), t.to_string(), w.to_string())).collect();
        if std::env::var_os("BRISK_TSC").is_some() {
            cases.push(("m.ts".into(), "const  x={a:1,b:[1,2]}\n".into(), "const x = { a: 1, b: [1, 2] }\n".into()));
        }
        for (f, t, _) in &cases {
            let _ = std::fs::write(root.join(f), t);
        }
        cx.spawn_in(window, async move |this, cx| {
            for (f, _, want) in cases {
                let path = root.join(&f);
                let _ = this.update_in(cx, |t, window, cx| t.open_file(&path, window, cx));
                // The TypeScript server takes a moment to start.
                for round in 0..2 {
                    let mut got = String::new();
                    for _ in 0..100 {
                        let _ = this.update_in(cx, |t, window, cx| {
                            t.status = Default::default();
                            t.format_action(Action::ReformatCode, window, cx);
                        });
                        let mut status = String::new();
                        for _ in 0..30 {
                            wait(cx, 50).await;
                            status = this.update(cx, |t, _| t.status.to_string()).unwrap_or_default();
                            if !status.is_empty() && status != "Reformatting…" {
                                break;
                            }
                        }
                        got = this.update(cx, |t, cx| t.active_editor().map(|e| e.read(cx).text_string())).ok().flatten().unwrap_or_default();
                        if !status.contains("not running yet") {
                            eprintln!("smoke-reformat: {f}: {status}");
                            break;
                        }
                        wait(cx, 100).await;
                    }
                    check(got == want, &format!("{f} round {}", round + 1), &got);
                }
            }
            eprintln!("smoke-reformat: PASS");
            std::process::exit(0);
        })
        .detach();
    }
}
