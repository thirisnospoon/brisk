//! `BRISK_SMOKE_TS=1` with `BRISK_TSC` pointing at TypeScript 7's `tsc` (on an empty scratch
//! folder): the TypeScript language server. Writes `user.ts` and `main.ts`, then checks: the
//! server starts, a type error shows in the editor, completion after `u.` lists the class's
//! methods, Ctrl+B goes to the other file, and Quick Documentation shows the type. Exits
//! with code 1 on a failure.

use crate::workspace::lsp_ui::TyState;
use crate::workspace::Workspace;
use brisk_keymap::Action;
use gpui::{AsyncWindowContext, Context, Window};
use std::time::Duration;

fn wait(cx: &AsyncWindowContext, ms: u64) -> gpui::Task<()> {
    cx.background_executor().timer(Duration::from_millis(ms))
}

fn check(ok: bool, what: &str, got: impl std::fmt::Debug) {
    if ok {
        eprintln!("smoke-ts: ok {what}");
    } else {
        eprintln!("smoke-ts: FAIL {what}: {got:?}");
        std::process::exit(1);
    }
}

impl Workspace {
    pub(in crate::workspace) fn ts_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let _ = std::fs::write(root.join("tsconfig.json"), "{ \"compilerOptions\": { \"strict\": true } }\n");
        let _ = std::fs::write(root.join("user.ts"), "export class User {\n  greet(n: number): string { return 'hi ' + n; }\n}\n");
        let main = root.join("main.ts");
        let _ = std::fs::write(&main, "import { User } from './user';\nconst u = new User();\nu.greet('x');\n");
        self.open_file(&main, window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let t0 = std::time::Instant::now();
            for _ in 0..200 {
                let s = this.update(cx, |t, _| t.lsp.ts.state.clone()).unwrap_or(TyState::Unknown);
                if !matches!(s, TyState::Unknown | TyState::Starting) {
                    break;
                }
                wait(cx, 50).await;
            }
            let state = this.update(cx, |t, _| t.lsp.ts.state.clone()).ok();
            check(state == Some(TyState::Ready), "server started", (&state, t0.elapsed()));

            let mut diags = Vec::new();
            for _ in 0..100 {
                diags = this
                    .update(cx, |t, cx| t.active_editor().map(|e| e.read(cx).diagnostics.iter().map(|d| (d.start.line, d.message.clone())).collect::<Vec<_>>()))
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                if !diags.is_empty() {
                    break;
                }
                wait(cx, 100).await;
            }
            check(diags.iter().any(|(l, m)| *l == 2 && m.contains("not assignable")), "type error shown", &diags);

            // Completion after `u.`.
            let _ = this.update(cx, |t, cx| {
                if let Some(ed) = t.active_editor().cloned() {
                    ed.update(cx, |e, cx| {
                        e.select_on_line(3, 0..0, cx);
                        e.type_for_smoke("u.", cx);
                    });
                }
            });
            let mut list = String::new();
            for _ in 0..50 {
                wait(cx, 100).await;
                list = this.update(cx, |t, cx| t.active_editor().map(|e| e.read(cx).insight_summary())).ok().flatten().unwrap_or_default();
                if list.contains("greet") {
                    break;
                }
            }
            check(list.contains("greet"), "completion after u.", &list);

            // Ctrl+B on `User` in `new User()`: user.ts.
            let _ = this.update_in(cx, |t, window, cx| {
                if let Some(ed) = t.active_editor().cloned() {
                    ed.update(cx, |e, cx| {
                        e.close_completion_for_smoke();
                        e.select_on_line(1, 15..15, cx);
                    });
                }
                t.lsp_action(Action::GotoDeclaration, window, cx);
            });
            let mut at = None;
            for _ in 0..50 {
                wait(cx, 100).await;
                at = this
                    .update(cx, |t, cx| {
                        t.active_editor().and_then(|e| {
                            let e = e.read(cx);
                            e.path.as_ref().map(|p| (p.file_name().unwrap_or_default().to_string_lossy().into_owned(), e.caret_line_col().0))
                        })
                    })
                    .ok()
                    .flatten();
                if at.as_ref().is_some_and(|a| a.0 == "user.ts") {
                    break;
                }
            }
            check(at == Some(("user.ts".into(), 0)), "Ctrl+B to user.ts", &at);

            // Ctrl+Q on `greet`.
            let _ = this.update_in(cx, |t, window, cx| {
                if let Some(ed) = t.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.select_on_line(1, 3..3, cx));
                }
                t.lsp_action(Action::QuickDocumentation, window, cx);
            });
            let mut doc = None;
            for _ in 0..50 {
                wait(cx, 100).await;
                doc = this.update(cx, |t, cx| t.active_editor().and_then(|e| e.read(cx).doc_shown_text().0)).ok().flatten();
                if doc.is_some() {
                    break;
                }
            }
            check(doc.as_deref().is_some_and(|d| d.contains("greet") && d.contains("number")), "quick documentation", &doc);
            eprintln!("smoke-ts: PASS");
            std::process::exit(0);
        })
        .detach();
    }
}
