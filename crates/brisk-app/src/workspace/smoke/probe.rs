//! `BRISK_SMOKE_PROBE=ts` (with `BRISK_TSC`) or `BRISK_SMOKE_PROBE=stage1` (without it), on an
//! empty scratch folder: a broad probe of the JS / TS / HTML / CSS features. Every scenario
//! logs `smoke-probe: ok …` or `smoke-probe: FAIL …` and the probe goes on; it exits 0 at the
//! end with a count of failures (exit 1 when any failed).

use crate::editor::Editor;
use crate::workspace::lsp_ui::TyState;
use crate::workspace::Workspace;
use brisk_keymap::Action;
use gpui::{AsyncWindowContext, Context, WeakEntity, Window};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static FAILS: AtomicUsize = AtomicUsize::new(0);

fn wait(cx: &AsyncWindowContext, ms: u64) -> gpui::Task<()> {
    cx.background_executor().timer(Duration::from_millis(ms))
}

fn check(ok: bool, what: &str, got: impl std::fmt::Debug) {
    let g: String = format!("{got:?}").chars().take(500).collect();
    if ok {
        eprintln!("smoke-probe: ok {what}: {g}");
    } else {
        FAILS.fetch_add(1, Ordering::SeqCst);
        eprintln!("smoke-probe: FAIL {what}: {g}");
    }
}

fn note(what: &str, got: impl std::fmt::Debug) {
    let g: String = format!("{got:?}").chars().take(500).collect();
    eprintln!("smoke-probe: note {what}: {g}");
}

/// Polls `f` every 100 ms for up to `ms`; its first `Some`.
async fn until<T>(
    this: &WeakEntity<Workspace>,
    cx: &mut AsyncWindowContext,
    ms: u64,
    mut f: impl FnMut(&mut Workspace, &mut Context<Workspace>) -> Option<T>,
) -> Option<T> {
    for _ in 0..=ms / 100 {
        if let Ok(Some(v)) = this.update(cx, |t, cx| f(t, cx)) {
            return Some(v);
        }
        wait(cx, 100).await;
    }
    None
}

fn summary(t: &Workspace, cx: &gpui::App) -> String {
    t.active_editor().map(|e| e.read(cx).insight_summary()).unwrap_or_default()
}

/// Only the completion list part of the summary.
fn comp(t: &Workspace, cx: &gpui::App) -> String {
    summary(t, cx).split(", preview").next().unwrap_or_default().to_string()
}

fn at_file(t: &Workspace, cx: &gpui::App) -> Option<(String, usize, usize)> {
    t.active_editor().and_then(|e| {
        let e = e.read(cx);
        let (l, c) = e.caret_line_col();
        e.path.as_ref().map(|p| (p.file_name().unwrap_or_default().to_string_lossy().into_owned(), l, c))
    })
}

fn text(t: &Workspace, cx: &gpui::App) -> String {
    t.active_editor().map(|e| e.read(cx).text_string()).unwrap_or_default()
}

fn diags(t: &Workspace, cx: &gpui::App) -> Vec<(usize, usize, String)> {
    t.active_editor().map(|e| e.read(cx).diagnostics.iter().map(|d| (d.start.line, d.start.col, d.message.clone())).collect()).unwrap_or_default()
}

/// Run `f` on the active editor, with the window.
fn on_ed<R>(
    t: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
    f: impl FnOnce(&mut Editor, &mut Window, &mut Context<Editor>) -> R,
) -> Option<R> {
    let e = t.active_editor().cloned()?;
    Some(e.update(cx, |e, cx| f(e, window, cx)))
}

/// Char column of `needle` on `line` of `s`.
fn col_of(s: &str, line: usize, needle: &str) -> usize {
    let l = s.lines().nth(line).unwrap_or_default();
    let b = l.find(needle).unwrap_or(0);
    l[..b].chars().count()
}

const USER: &str = "export class User {\n  greet(n: number): string { return 'hi ' + n; }\n  grow(by: number): number { return by; }\n  name = 'x';\n}\nexport interface Point { x: number; y: number }\nexport function makePoint(x: number, y: number): Point { return { x, y }; }\n";
const MAIN: &str = "import { User, makePoint } from './user';\nconst u = new User();\nu.greet('x');\nconst p = makePoint(1, 2);\n\n";
const UNI: &str = "import { User } from './user';\nconst w = new User();\nconst s = 'Привет 🎉🎉 мир'; w.nope;\nconst z = '🎉'; \n";

impl Workspace {
    pub(in crate::workspace) fn probe_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = std::env::var("BRISK_SMOKE_PROBE").unwrap_or_default();
        if mode == "stage1" {
            return self.probe_stage1(window, cx);
        }
        let root = self.root.clone();
        let w = |n: &str, s: &str| {
            let _ = std::fs::write(root.join(n), s);
        };
        w(
            "tsconfig.json",
            "{ \"compilerOptions\": { \"strict\": true, \"allowJs\": true, \"checkJs\": true, \"jsx\": \"preserve\", \"target\": \"es2022\", \"module\": \"esnext\", \"moduleResolution\": \"bundler\" } }\n",
        );
        w("user.ts", USER);
        w("util.ts", "export function helperFunc(a: number): number { return a * 2; }\n");
        w("main.ts", MAIN);
        w("uni.ts", UNI);
        w("app.js", "class Dog {\n  bark() { return 1; }\n  wag() {}\n}\nconst d = new Dog();\nd.nope();\n\n");
        w("comp.tsx", "type Props = { label: string; count: number };\nexport function Btn(props: Props) {\n  return <div>{props}</div>;\n}\n\n");
        w("lib.ts", "export function compute(alpha: number, beta: string): boolean { return alpha > 0 && beta !== ''; }\n");
        w("a.py", "def pyfunc():\n    return 1\n\npyfunc()\n");
        let main = root.join("main.ts");
        self.open_file(&main, window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let t0 = std::time::Instant::now();
            let ready = until(&this, cx, 20000, |t, _| (!matches!(t.lsp.ts.state, TyState::Unknown | TyState::Starting)).then(|| t.lsp.ts.state.clone())).await;
            check(ready == Some(TyState::Ready), "TS server started", (&ready, t0.elapsed()));
            let mainp = this.update(cx, |t, _| t.root.join("main.ts")).unwrap();
            let open = |p: &str| {
                let p = p.to_string();
                move |t: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>| {
                    let f = t.root.join(&p);
                    t.open_file(&f, window, cx);
                }
            };

            // 1. Diagnostics, then fixing the error clears it.
            let d = until(&this, cx, 10000, |t, cx| Some(diags(t, cx)).filter(|d| !d.is_empty())).await.unwrap_or_default();
            check(d.iter().any(|(l, _, m)| *l == 2 && m.contains("not assignable")), "ts: type error shown", &d);
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.select_on_line(2, 8..11, cx);
                    e.type_for_smoke("1", cx);
                })
            });
            let fixed = until(&this, cx, 6000, |t, cx| {
                let d = diags(t, cx);
                (!d.iter().any(|(l, _, _)| *l == 2)).then_some(d)
            })
            .await;
            let line2 = this.update(cx, |t, cx| text(t, cx).lines().nth(2).map(str::to_string)).ok().flatten();
            check(fixed.is_some(), "ts: diagnostics cleared after the fix (no save)", (&line2, this.update(cx, |t, cx| diags(t, cx)).ok()));
            // A new error introduced by typing.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.select_on_line(4, 0..0, cx);
                    e.type_for_smoke("const bad: number = 'str';", cx);
                    e.close_completion_for_smoke();
                })
            });
            let newd = until(&this, cx, 6000, |t, cx| {
                let d = diags(t, cx);
                d.iter().any(|(l, _, _)| *l == 4).then_some(d)
            })
            .await;
            check(newd.is_some(), "ts: new error appears after typing it (no save)", this.update(cx, |t, cx| diags(t, cx)).ok());
            // Saving: does the check then catch up?
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, _| {
                    let _ = e.save();
                })
            });
            let saved = until(&this, cx, 4000, |t, cx| {
                let d = diags(t, cx);
                (d.iter().any(|(l, _, _)| *l == 4) && !d.iter().any(|(l, _, _)| *l == 2)).then_some(d)
            })
            .await;
            note("ts: diagnostics after save", (saved.is_some(), this.update(cx, |t, cx| diags(t, cx)).ok()));
            // Reset the line.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    let n = e.buffer.line(4).chars().count();
                    e.select_on_line(4, 0..n, cx);
                    e.type_for_smoke(" ", cx);
                    e.close_completion_for_smoke();
                })
            });

            // 2. Completion with a prefix.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.select_on_line(4, 0..1, cx);
                    e.type_for_smoke("u.gr", cx);
                })
            });
            let list = until(&this, cx, 5000, |t, cx| Some(comp(t, cx)).filter(|s| s.contains("grow"))).await;
            let list = list.unwrap_or_else(|| this.update(cx, |t, cx| comp(t, cx)).unwrap_or_default());
            check(list.contains("greet") && list.contains("grow") && !list.contains("\"name\""), "ts: completion u.gr", &list);
            // Accept it: `u.greet()` / `u.grow()`.
            let line = this
                .update(cx, |t, cx| {
                    let e = t.active_editor().cloned()?;
                    e.update(cx, |e, cx| e.accept_completion(false, cx));
                    Some(e.read(cx).buffer.line(4).to_string())
                })
                .ok()
                .flatten();
            check(line.as_deref().is_some_and(|l| l.starts_with("u.gr") && l.contains("()")), "ts: accept completion u.gr", &line);

            // 3. Completion in a fresh object literal type.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.close_completion_for_smoke();
                    let n = e.buffer.line(4).chars().count();
                    e.select_on_line(4, 0..n, cx);
                    e.type_for_smoke("const o = { alpha: 1, beta: 'b' }; o.", cx);
                })
            });
            let list = until(&this, cx, 5000, |t, cx| Some(comp(t, cx)).filter(|s| s.contains("alpha"))).await;
            check(
                list.as_deref().is_some_and(|l| l.contains("beta")),
                "ts: completion on object literal",
                list.or_else(|| this.update(cx, |t, cx| comp(t, cx)).ok()),
            );

            // 4. A string with `//` before the caret on the line.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.close_completion_for_smoke();
                    let n = e.buffer.line(4).chars().count();
                    e.select_on_line(4, 0..n, cx);
                    e.show_signature(None, cx);
                    e.type_for_smoke("const url = 'http://x'; u.", cx);
                })
            });
            let list = until(&this, cx, 3000, |t, cx| Some(comp(t, cx)).filter(|s| s.contains("greet"))).await;
            check(list.is_some(), "ts: completion after a 'http://' string on the line", this.update(cx, |t, cx| comp(t, cx)).ok());

            // 5. Auto-import completion of another file's export.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.close_completion_for_smoke();
                    let n = e.buffer.line(4).chars().count();
                    e.select_on_line(4, 0..n, cx);
                    e.show_signature(None, cx);
                })
            });
            // One key at a time, as a person types.
            for ch in "helperFu".chars() {
                let _ = this.update(cx, |t, cx| {
                    if let Some(e) = t.active_editor().cloned() {
                        e.update(cx, |e, cx| e.type_for_smoke(&ch.to_string(), cx));
                    }
                });
                wait(cx, 150).await;
            }
            let list = until(&this, cx, 6000, |t, cx| Some(comp(t, cx)).filter(|s| s.contains("helperFunc"))).await;
            check(list.is_some(), "ts: auto-import completion listed", this.update(cx, |t, cx| comp(t, cx)).ok());
            let _ = this.update(cx, |t, cx| {
                let e = t.active_editor().cloned()?;
                e.update(cx, |e, cx| e.accept_completion(false, cx));
                Some(())
            });
            // The import line comes from `completionItem/resolve`, a moment later.
            let txt = until(&this, cx, 4000, |t, cx| Some(text(t, cx)).filter(|s| s.contains("import { helperFunc }")))
                .await
                .unwrap_or_else(|| this.update(cx, |t, cx| text(t, cx)).unwrap_or_default());
            check(
                txt.contains("import { helperFunc } from './util'") || txt.contains("import { helperFunc } from \"./util\""),
                "ts: auto-import adds the import",
                &txt,
            );

            // Back to the original text.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.close_completion_for_smoke();
                    e.buffer.select_all();
                    e.type_for_smoke(MAIN, cx);
                    e.close_completion_for_smoke();
                    let _ = e.save();
                })
            });
            wait(cx, 300).await;
            let cur = this.update(cx, |t, cx| text(t, cx)).unwrap_or_default();
            note("ts: main.ts reset", &cur);

            // 6. Ctrl+B on a method through the server (not an imported name).
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.close_completion_for_smoke();
                    e.select_on_line(2, 4..4, cx);
                });
                t.lsp_action(Action::GotoDeclaration, window, cx);
            });
            let at = until(&this, cx, 4000, |t, cx| at_file(t, cx).filter(|a| a.0 == "user.ts")).await;
            check(at.as_ref().is_some_and(|a| a.1 == 1), "ts: Ctrl+B on u.greet -> user.ts:2", (&at, this.update(cx, |t, _| t.status.clone()).ok()));
            let _ = this.update_in(cx, open("main.ts"));
            // 7. Ctrl+B on an imported function.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| e.select_on_line(3, 12..12, cx));
                t.lsp_action(Action::GotoDeclaration, window, cx);
            });
            let at = until(&this, cx, 4000, |t, cx| at_file(t, cx).filter(|a| a.0 == "user.ts")).await;
            check(at.as_ref().is_some_and(|a| a.1 == 6), "ts: Ctrl+B on makePoint -> user.ts:7", &at);
            let _ = this.update_in(cx, open("main.ts"));
            // 8. Go to Type Declaration on `p`.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| e.select_on_line(3, 6..6, cx));
                t.lsp_action(Action::GotoTypeDeclaration, window, cx);
            });
            let at = until(&this, cx, 4000, |t, cx| at_file(t, cx).filter(|a| a.0 == "user.ts")).await;
            check(at.as_ref().is_some_and(|a| a.1 == 5), "ts: Go to Type Declaration p -> Point", (&at, this.update(cx, |t, _| t.status.clone()).ok()));

            // 9. Find Usages of `greet` from its declaration in user.ts.
            let _ = this.update_in(cx, |t, window, cx| {
                t.open_file(&t.root.join("user.ts"), window, cx);
                on_ed(t, window, cx, |e, _, cx| e.select_on_line(1, 3..3, cx));
                t.find.window = None;
                t.lsp_action(Action::FindUsages, window, cx);
            });
            let rows = until(&this, cx, 5000, |t, _| Some(t.find_window_rows()).filter(|r| !r.is_empty())).await.unwrap_or_default();
            check(rows.iter().any(|r| r.contains("main.ts")), "ts: Find Usages greet across files", (&rows, this.update(cx, |t, _| t.status.clone()).ok()));
            // Ctrl+B on the declaration itself shows usages too.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| e.select_on_line(6, 18..18, cx));
                t.find.window = None;
                t.lsp_action(Action::GotoDeclaration, window, cx);
            });
            let rows = until(&this, cx, 5000, |t, _| Some(t.find_window_rows()).filter(|r| !r.is_empty())).await.unwrap_or_default();
            check(
                rows.iter().any(|r| r.contains("main.ts")),
                "ts: Ctrl+B on declaration makePoint -> usages",
                (&rows, this.update(cx, |t, cx| (t.status.clone(), at_file(t, cx))).ok()),
            );

            // 10. Parameter Info after `(`.
            let _ = this.update_in(cx, |t, window, cx| {
                t.open_file(&t.root.join("main.ts"), window, cx);
                on_ed(t, window, cx, |e, _, cx| {
                    e.select_on_line(4, 0..0, cx);
                    e.type_for_smoke("makePoint(", cx);
                })
            });
            let sig = until(&this, cx, 4000, |t, cx| Some(summary(t, cx)).filter(|s| s.contains("signature Some"))).await;
            check(
                sig.as_deref().is_some_and(|s| s.contains("x: number")),
                "ts: Parameter Info after (",
                sig.or_else(|| this.update(cx, |t, cx| comp(t, cx)).ok()),
            );
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| e.type_for_smoke("1,", cx));
            });
            wait(cx, 800).await;
            let s2 = this.update(cx, |t, cx| t.active_editor().map(|e| e.read(cx).insight_for_smoke())).ok().flatten();
            check(s2.as_ref().is_some_and(|s| s.1.iter().any(|l| l.contains("**y: number**"))), "ts: Parameter Info active param after ,", &s2);

            // 11. Hover / Quick Doc on a library method.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.close_completion_for_smoke();
                    let n = e.buffer.line(4).chars().count();
                    e.select_on_line(4, 0..n, cx);
                    e.type_for_smoke("const arr = [1, 2].map(x => x * 2);", cx);
                    e.close_completion_for_smoke();
                    e.select_on_line(4, 20..20, cx);
                });
                t.lsp_action(Action::QuickDocumentation, window, cx);
            });
            let doc = until(&this, cx, 4000, |t, cx| t.active_editor().and_then(|e| e.read(cx).doc_shown_text().0)).await;
            check(doc.as_deref().is_some_and(|d| d.contains("map") && d.contains("callbackfn")), "ts: Quick Doc on Array.map", &doc);
            let src = this.update(cx, |t, cx| t.active_editor().map(|e| e.read(cx).insight_for_smoke())).ok().flatten();
            note("ts: Quick Doc on Array.map, source/sig", &src);
            // Type Info on `u`.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| e.select_on_line(1, 6..6, cx));
                t.status.clear();
                t.lsp_action(Action::TypeInfo, window, cx);
            });
            let st = until(&this, cx, 4000, |t, _| Some(t.status.clone()).filter(|s| !s.is_empty())).await;
            let shown = this.update(cx, |t, cx| t.active_editor().and_then(|e| e.read(cx).doc_shown_text().0)).ok().flatten();
            check(st.as_deref().is_some_and(|s| s.contains("User")), "ts: Type Info on u", (&st, &shown));
            // Quick Doc on user's class: the source link label.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| e.select_on_line(1, 16..16, cx));
                t.lsp_action(Action::QuickDocumentation, window, cx);
            });
            wait(cx, 1500).await;
            let qd = this.update(cx, |t, cx| t.active_editor().map(|e| (e.read(cx).doc_shown_text().0, e.read(cx).insight_for_smoke()))).ok().flatten();
            note("ts: Quick Doc on User (text, source)", &qd);
            // Quick Definition.
            let _ = this.update_in(cx, |t, window, cx| {
                t.status.clear();
                t.lsp_action(Action::QuickDefinition, window, cx);
            });
            wait(cx, 1500).await;
            let qd = this.update(cx, |t, cx| (t.status.clone(), summary(t, cx))).ok();
            note("ts: Quick Definition on User (status, insight)", &qd);

            // 12. Rename through the popup: makePoint -> makePt in both files.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    let n = e.buffer.line(4).chars().count();
                    e.select_on_line(4, 0..n, cx);
                    e.type_for_smoke(" ", cx);
                    e.close_completion_for_smoke();
                    let _ = e.save();
                    e.select_on_line(3, 12..12, cx);
                });
                t.popup = None;
                t.dispatch(Action::Rename, window, cx);
            });
            let q = until(&this, cx, 4000, |t, _| t.popup.as_ref().filter(|p| p.kind == crate::workspace::PopupKind::Rename).map(|p| p.query.clone())).await;
            check(q.as_deref() == Some("makePoint"), "ts: Rename popup opens with the name", (&q, this.update(cx, |t, _| t.status.clone()).ok()));
            if q.is_some() {
                let items = this.update(cx, |t, _| t.rename_items("makePt").into_iter().map(|i| i.label).collect::<Vec<_>>()).unwrap_or_default();
                note("ts: rename items", &items);
                let _ = this.update_in(cx, |t, window, cx| {
                    t.popup = None;
                    t.apply_rename("makePt".into(), window, cx);
                });
                let done = until(&this, cx, 5000, |t, cx| {
                    let m = text(t, cx);
                    let u = t
                        .editors()
                        .find(|e| e.read(cx).path.as_deref().is_some_and(|p| p.ends_with("user.ts")))
                        .map(|e| e.read(cx).text_string())
                        .or_else(|| std::fs::read_to_string(t.root.join("user.ts")).ok())
                        .unwrap_or_default();
                    (m.contains("makePt(1, 2)") && m.contains("{ User, makePt }") && u.contains("function makePt(")).then_some(())
                })
                .await;
                let snap = this.update(cx, |t, cx| (t.status.clone(), text(t, cx))).ok();
                check(done.is_some(), "ts: Rename makePoint -> makePt in both files", &snap);
            }
            // Rename of a class member from its usage.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| e.select_on_line(2, 3..3, cx));
                t.popup = None;
                t.dispatch(Action::Rename, window, cx);
            });
            let q = until(&this, cx, 4000, |t, _| t.popup.as_ref().filter(|p| p.kind == crate::workspace::PopupKind::Rename).map(|p| p.query.clone())).await;
            check(q.as_deref() == Some("greet"), "ts: Rename popup on a method", (&q, this.update(cx, |t, _| t.status.clone()).ok()));
            let _ = this.update(cx, |t, _| t.popup = None);
            let _ = this.update(cx, |t, _| t.nav.rename = None);

            // 13. Close the tab and reopen it: the server still answers.
            let _ = this.update_in(cx, |t, window, cx| {
                if let Some(e) = t.active_editor().cloned() {
                    let _ = e.update(cx, |e, _| e.save());
                    t.close_editor(&e, window, cx);
                }
                t.open_file(&mainp, window, cx);
                on_ed(t, window, cx, |e, _, cx| {
                    e.select_on_line(4, 0..0, cx);
                    e.type_for_smoke("u.", cx);
                })
            });
            let list = until(&this, cx, 5000, |t, cx| Some(comp(t, cx)).filter(|s| s.contains("greet"))).await;
            check(list.is_some(), "ts: completion after close + reopen", this.update(cx, |t, cx| comp(t, cx)).ok());
            let d = until(&this, cx, 5000, |t, cx| Some(diags(t, cx)).filter(|d| !d.is_empty())).await;
            note("ts: diagnostics after reopen (expected none on 0..3)", &d);
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.close_completion_for_smoke();
                    let n = e.buffer.line(4).chars().count();
                    e.select_on_line(4, 0..n, cx);
                    e.type_for_smoke(" ", cx);
                    e.close_completion_for_smoke();
                    let _ = e.save();
                })
            });

            // 14. Non-ASCII text before the caret.
            let _ = this.update_in(cx, open("uni.ts"));
            let d = until(&this, cx, 8000, |t, cx| Some(diags(t, cx)).filter(|d| d.iter().any(|x| x.0 == 2))).await.unwrap_or_default();
            let want = col_of(UNI, 2, "nope");
            check(d.iter().any(|x| x.0 == 2 && x.1 == want), &format!("ts: diagnostic column after Cyrillic + emoji (want col {want})"), &d);
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    let n = e.buffer.line(3).chars().count();
                    e.select_on_line(3, n..n, cx);
                    e.type_for_smoke("w.", cx);
                })
            });
            let list = until(&this, cx, 5000, |t, cx| Some(comp(t, cx)).filter(|s| s.contains("greet"))).await;
            check(list.is_some(), "ts: completion after emoji on the line", this.update(cx, |t, cx| comp(t, cx)).ok());
            let line = this
                .update(cx, |t, cx| {
                    let e = t.active_editor().cloned()?;
                    e.update(cx, |e, cx| {
                        e.select_completion_for_smoke("greet");
                        e.accept_completion(false, cx)
                    });
                    Some(e.read(cx).buffer.line(3).to_string())
                })
                .ok()
                .flatten();
            check(line.as_deref().is_some_and(|l| l.trim_end().ends_with("w.greet()")), "ts: accepted completion lands after the emoji", &line);
            // Ctrl+B on `greet` after the emoji.
            let c = line.as_deref().map(|l| l.chars().count().saturating_sub(4)).unwrap_or(0);
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.close_completion_for_smoke();
                    e.select_on_line(3, c..c, cx);
                });
                t.lsp_action(Action::GotoDeclaration, window, cx);
            });
            let at = until(&this, cx, 4000, |t, cx| at_file(t, cx).filter(|a| a.0 == "user.ts")).await;
            check(at.as_ref().is_some_and(|a| a.1 == 1 && a.2 == 2), "ts: Ctrl+B after emoji -> user.ts:2:3", &at);
            // Hover on `w` after Cyrillic + emoji (line 2).
            let _ = this.update_in(cx, open("uni.ts"));
            let wc = col_of(UNI, 2, "w.nope");
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| e.select_on_line(2, wc..wc, cx));
                t.lsp_action(Action::QuickDocumentation, window, cx);
            });
            let doc = until(&this, cx, 4000, |t, cx| t.active_editor().and_then(|e| e.read(cx).doc_shown_text().0)).await;
            check(doc.as_deref().is_some_and(|d| d.contains("const w: User")), "ts: Quick Doc on w after emoji", &doc);

            // 15. A .js file: checkJs diagnostics, completion, Ctrl+B.
            let _ = this.update_in(cx, open("app.js"));
            let d = until(&this, cx, 8000, |t, cx| Some(diags(t, cx)).filter(|d| !d.is_empty())).await.unwrap_or_default();
            check(d.iter().any(|x| x.0 == 5), "js: checkJs error on d.nope()", &d);
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.select_on_line(6, 0..0, cx);
                    e.type_for_smoke("d.", cx);
                })
            });
            let list = until(&this, cx, 5000, |t, cx| Some(comp(t, cx)).filter(|s| s.contains("bark"))).await;
            check(list.as_deref().is_some_and(|l| l.contains("wag")), "js: completion d.", list.or_else(|| this.update(cx, |t, cx| comp(t, cx)).ok()));
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.close_completion_for_smoke();
                    e.select_on_line(4, 15..15, cx);
                });
                t.lsp_action(Action::GotoDeclaration, window, cx);
            });
            wait(cx, 800).await;
            let at = this.update(cx, |t, cx| at_file(t, cx)).ok().flatten();
            check(at.as_ref().is_some_and(|a| a.0 == "app.js" && a.1 == 0), "js: Ctrl+B on Dog", &at);
            // Find Usages in JS (server).
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| e.select_on_line(1, 3..3, cx));
                t.find.window = None;
                t.lsp_action(Action::FindUsages, window, cx);
            });
            wait(cx, 1500).await;
            let rows = this.update(cx, |t, _| (t.find_window_rows(), t.status.clone())).ok();
            note("js: Find Usages bark (no uses) ", &rows);

            // 16. TSX: completion in JSX, Ctrl+B, tag auto-close.
            let _ = this.update_in(cx, open("comp.tsx"));
            wait(cx, 500).await;
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.select_on_line(2, 20..20, cx);
                    e.type_for_smoke(".", cx);
                })
            });
            let list = until(&this, cx, 5000, |t, cx| Some(comp(t, cx)).filter(|s| s.contains("label"))).await;
            check(
                list.as_deref().is_some_and(|l| l.contains("count")),
                "tsx: completion props. inside JSX",
                list.or_else(|| this.update(cx, |t, cx| comp(t, cx)).ok()),
            );
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.close_completion_for_smoke();
                    e.select_on_line(1, 29..29, cx);
                });
                t.lsp_action(Action::GotoDeclaration, window, cx);
            });
            wait(cx, 1000).await;
            let at = this.update(cx, |t, cx| at_file(t, cx)).ok().flatten();
            check(at.as_ref().is_some_and(|a| a.0 == "comp.tsx" && a.1 == 0), "tsx: Ctrl+B on Props", &at);
            let d = this.update(cx, |t, cx| diags(t, cx)).unwrap_or_default();
            note("tsx: diagnostics", &d);

            // 17. A second TS file opened later: hover, Parameter Info on a local function.
            let _ = this.update_in(cx, |t, window, cx| {
                let f = t.root.join("lib.ts");
                t.open_file(&f, window, cx);
                on_ed(t, window, cx, |e, _, cx| {
                    e.select_on_line(1, 0..0, cx);
                    e.type_for_smoke("compute(", cx);
                })
            });
            let sig = until(&this, cx, 4000, |t, cx| Some(summary(t, cx)).filter(|s| s.contains("signature Some"))).await;
            check(sig.as_deref().is_some_and(|s| s.contains("alpha: number")), "ts: Parameter Info in a file opened later", &sig);
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.close_completion_for_smoke();
                    e.select_on_line(0, 18..18, cx);
                });
                t.lsp_action(Action::QuickDocumentation, window, cx);
            });
            let doc = until(&this, cx, 4000, |t, cx| t.active_editor().and_then(|e| e.read(cx).doc_shown_text().0)).await;
            check(doc.as_deref().is_some_and(|d| d.contains("compute")), "ts: Quick Doc in a file opened later", &doc);
            // Code completion (Ctrl+Space) explicit, on an empty line.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    let n = e.buffer.line(1).chars().count();
                    e.select_on_line(1, 0..n, cx);
                    e.type_for_smoke(" ", cx);
                    e.close_completion_for_smoke();
                    e.select_on_line(1, 0..0, cx);
                });
                window.focus(&gpui::Focusable::focus_handle(t.active_editor().unwrap().read(cx), cx));
                t.lsp_action(Action::CodeCompletion, window, cx);
            });
            let list = until(&this, cx, 4000, |t, cx| Some(comp(t, cx)).filter(|s| s.contains("completion Some"))).await;
            note("ts: Ctrl+Space on empty line", &list);

            // 17b. Explicit Ctrl+Space auto-import in a file with no imports.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.close_completion_for_smoke();
                    e.show_signature(None, cx);
                    let n = e.buffer.line(1).chars().count();
                    e.select_on_line(1, 0..n, cx);
                    e.type_for_smoke("helperFu", cx);
                    e.close_completion_for_smoke();
                });
            });
            wait(cx, 1500).await;
            let _ = this.update_in(cx, |t, window, cx| {
                window.focus(&gpui::Focusable::focus_handle(t.active_editor().unwrap().read(cx), cx));
                t.lsp_action(Action::CodeCompletion, window, cx);
            });
            let list = until(&this, cx, 5000, |t, cx| Some(comp(t, cx)).filter(|s| s.contains("helperFunc"))).await;
            check(list.is_some(), "ts: Ctrl+Space auto-import helperFunc (other file export)", this.update(cx, |t, cx| (comp(t, cx), t.status.clone())).ok());
            // Same for an export of user.ts.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.close_completion_for_smoke();
                    let n = e.buffer.line(1).chars().count();
                    e.select_on_line(1, 0..n, cx);
                });
            });
            for ch in "makeP".chars() {
                let _ = this.update(cx, |t, cx| {
                    if let Some(e) = t.active_editor().cloned() {
                        e.update(cx, |e, cx| e.type_for_smoke(&ch.to_string(), cx));
                    }
                });
                wait(cx, 150).await;
            }
            let list = until(&this, cx, 5000, |t, cx| Some(comp(t, cx)).filter(|s| s.contains("makeP"))).await;
            check(list.is_some(), "ts: auto-import completion makeP (as you type)", this.update(cx, |t, cx| comp(t, cx)).ok());
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.close_completion_for_smoke();
                    let n = e.buffer.line(1).chars().count();
                    e.select_on_line(1, 0..n, cx);
                    e.type_for_smoke(" ", cx);
                    let _ = e.save();
                });
            });
            // 17c. Rename from the declaration: every file follows.
            let _ = this.update_in(cx, |t, window, cx| {
                let f = t.root.join("user.ts");
                t.open_file(&f, window, cx);
                on_ed(t, window, cx, |e, _, cx| e.select_on_line(1, 3..3, cx));
                t.popup = None;
                t.dispatch(Action::Rename, window, cx);
            });
            let q = until(&this, cx, 4000, |t, _| t.popup.as_ref().filter(|p| p.kind == crate::workspace::PopupKind::Rename).map(|p| p.query.clone())).await;
            if q.is_some() {
                let _ = this.update_in(cx, |t, window, cx| {
                    t.popup = None;
                    t.apply_rename("salute".into(), window, cx);
                });
                let done = until(&this, cx, 5000, |t, cx| {
                    let m = t
                        .editors()
                        .find(|e| e.read(cx).path.as_deref().is_some_and(|p| p.ends_with("main.ts")))
                        .map(|e| e.read(cx).text_string())
                        .or_else(|| std::fs::read_to_string(t.root.join("main.ts")).ok())
                        .unwrap_or_default();
                    (m.contains("u.salute(") && text(t, cx).contains("salute(n")).then_some(())
                })
                .await;
                check(done.is_some(), "ts: Rename method greet -> salute from declaration, across files", this.update(cx, |t, _| t.status.clone()).ok());
            } else {
                check(false, "ts: Rename popup from declaration", this.update(cx, |t, _| t.status.clone()).ok());
            }
            // 18. Python alongside.
            let _ = this.update_in(cx, open("a.py"));
            wait(cx, 3000).await;
            let st = this.update(cx, |t, _| t.lsp.state.clone()).ok();
            note("py: ty state", &st);
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| e.select_on_line(3, 2..2, cx));
                t.lsp_action(Action::GotoDeclaration, window, cx);
            });
            wait(cx, 1500).await;
            let at = this.update(cx, |t, cx| (at_file(t, cx), t.status.clone())).ok();
            check(at.as_ref().is_some_and(|a| a.0.as_ref().is_some_and(|f| f.0 == "a.py" && f.1 == 0)), "py: Ctrl+B on pyfunc", &at);
            let ts_alive = this.update(cx, |t, _| (t.lsp.ts.state.clone(), t.lsp.ts.server.as_ref().map(|s| s.is_alive()))).ok();
            check(ts_alive.as_ref().is_some_and(|s| s.0 == TyState::Ready && s.1 == Some(true)), "ts: server still alive at the end", &ts_alive);

            let n = FAILS.load(Ordering::SeqCst);
            eprintln!("smoke-probe: DONE with {n} failure(s)");
            std::process::exit(if n == 0 { 0 } else { 1 });
        })
        .detach();
    }

    /// Without TypeScript: the offer, then Brisk's own JS / TS / HTML / CSS support.
    fn probe_stage1(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let w = |n: &str, s: &str| {
            let _ = std::fs::write(root.join(n), s);
        };
        w("user.ts", USER);
        w("main.ts", MAIN);
        w("loc.js", "function outer(count) {\n  const total = count + 1;\n  return total * count;\n}\nouter(2);\n");
        w("page.vue", "<template>\n\n</template>\n");
        w("view.jsx", "export const V = () => (\n\n);\n");
        w("gen.tsx", "\n");
        w("index.html", "<div><p>\n\n");
        w("e.html", "\n\n\n");
        w("s.scss", "$c: red;\na {\n  \n}\n");
        w("c.css", "a {\n  color: rgb(0, 128, 0);\n  background: hsl(120, 100%, 50%);\n  border-color: red;\n  outline-color: rgba(0, 0, 255, 0.5);\n  fill: #0f08;\n  color: var(--x);\n  color: RED;\n  border: 1px solid transparent;\n}\n");
        let main = root.join("main.ts");
        self.open_file(&main, window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let open = |p: &str| {
                let p = p.to_string();
                move |t: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>| {
                    let f = t.root.join(&p);
                    t.open_file(&f, window, cx);
                }
            };
            let offer = until(&this, cx, 5000, |t, _| t.lsp.ts.offer.then_some(())).await;
            check(offer.is_some(), "stage1: download offer shown", this.update(cx, |t, _| (t.lsp.ts.offer, t.lsp.ts.state.clone())).ok());
            // Path Ctrl+B and imported-name Ctrl+B without the server.
            let got = this
                .update_in(cx, |t, window, cx| {
                    on_ed(t, window, cx, |e, _, cx| e.select_on_line(0, 35..35, cx));
                    t.lsp_action(Action::GotoDeclaration, window, cx);
                    at_file(t, cx)
                })
                .ok()
                .flatten();
            check(got.as_ref().is_some_and(|g| g.0 == "user.ts"), "stage1: Ctrl+B on './user'", &got);
            let _ = this.update_in(cx, open("main.ts"));
            let got = this
                .update_in(cx, |t, window, cx| {
                    on_ed(t, window, cx, |e, _, cx| e.select_on_line(3, 12..12, cx));
                    t.lsp_action(Action::GotoDeclaration, window, cx);
                    (at_file(t, cx), t.status.clone())
                })
                .ok();
            check(got.as_ref().is_some_and(|g| g.0.as_ref().is_some_and(|a| a.0 == "user.ts" && a.1 == 6)), "stage1: Ctrl+B on imported makePoint", &got);
            let _ = this.update_in(cx, open("main.ts"));
            // Ctrl+B on a local const.
            let got = this
                .update_in(cx, |t, window, cx| {
                    on_ed(t, window, cx, |e, _, cx| e.select_on_line(2, 0..0, cx));
                    t.lsp_action(Action::GotoDeclaration, window, cx);
                    (at_file(t, cx), t.status.clone())
                })
                .ok();
            check(got.as_ref().is_some_and(|g| g.0.as_ref().is_some_and(|a| a.0 == "main.ts" && a.1 == 1)), "stage1: Ctrl+B on local u", &got);
            // Path completion.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.select_on_line(4, 0..0, cx);
                    e.type_for_smoke("import x from './u", cx);
                })
            });
            wait(cx, 300).await;
            let list = this.update(cx, |t, cx| comp(t, cx)).unwrap_or_default();
            check(list.contains("user") && !list.contains("user.ts"), "stage1: path completion ./u", &list);
            // Completion without the server: words.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.close_completion_for_smoke();
                    let n = e.buffer.line(4).chars().count();
                    e.select_on_line(4, 0..n, cx);
                    e.type_for_smoke("make", cx);
                });
                t.lsp_action(Action::CodeCompletion, window, cx);
            });
            wait(cx, 600).await;
            let list = this.update(cx, |t, cx| (comp(t, cx), t.status.clone())).ok();
            check(list.as_ref().is_some_and(|l| l.0.contains("makePoint")), "stage1: word completion without server", &list);
            // Quick Doc without the server: says it needs TypeScript.
            let st = this
                .update_in(cx, |t, window, cx| {
                    on_ed(t, window, cx, |e, _, cx| {
                        e.close_completion_for_smoke();
                        e.select_on_line(1, 16..16, cx);
                    });
                    t.lsp_action(Action::QuickDocumentation, window, cx);
                    t.status.clone()
                })
                .ok();
            check(st.as_ref().is_some_and(|s| s.contains("TypeScript")), "stage1: Quick Doc says it needs TypeScript", &st);
            // Find Usages and Rename of a local without the server (JS).
            let _ = this.update_in(cx, open("loc.js"));
            let got = this
                .update_in(cx, |t, window, cx| {
                    on_ed(t, window, cx, |e, _, cx| e.select_on_line(1, 9..9, cx));
                    t.find.window = None;
                    t.lsp_action(Action::FindUsages, window, cx);
                    (t.find_window_rows(), t.status.clone())
                })
                .ok();
            check(got.as_ref().is_some_and(|g| g.1.contains("2 usage") || g.1.contains("3 usage")), "stage1: Find Usages local in JS", &got);
            let got = this
                .update_in(cx, |t, window, cx| {
                    on_ed(t, window, cx, |e, _, cx| e.select_on_line(1, 9..9, cx));
                    t.dispatch(Action::Rename, window, cx);
                    let n = t.active_editor().map(|e| e.read(cx).buffer.selections.len());
                    (n, t.status.clone())
                })
                .ok();
            check(got.as_ref().is_some_and(|g| g.0 == Some(2)), "stage1: Rename local in JS (2 carets)", &got);
            let got = this
                .update_in(cx, |t, window, cx| {
                    on_ed(t, window, cx, |e, _, cx| e.select_on_line(0, 15..15, cx));
                    t.dispatch(Action::Rename, window, cx);
                    let n = t.active_editor().map(|e| e.read(cx).buffer.selections.len());
                    (n, t.status.clone())
                })
                .ok();
            check(got.as_ref().is_some_and(|g| g.0 == Some(3)), "stage1: Rename parameter in JS (3 carets)", &got);
            // Ctrl+B on a function call in the file.
            let got = this
                .update_in(cx, |t, window, cx| {
                    on_ed(t, window, cx, |e, _, cx| {
                        e.buffer.selections.truncate(1);
                        e.select_on_line(4, 1..1, cx)
                    });
                    t.lsp_action(Action::GotoDeclaration, window, cx);
                    at_file(t, cx)
                })
                .ok()
                .flatten();
            check(got.as_ref().is_some_and(|g| g.1 == 0), "stage1: Ctrl+B on outer()", &got);

            // HTML in Vue and JSX: `>` closes the tag.
            let tag = |file: &'static str, line: usize, typed: &'static str| {
                move |t: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>| {
                    let f = t.root.join(file);
                    t.open_file(&f, window, cx);
                    on_ed(t, window, cx, |e, _, cx| {
                        e.select_on_line(line, 0..0, cx);
                        e.type_for_smoke(typed, cx);
                        e.close_completion_for_smoke();
                        e.buffer.line(line).to_string()
                    })
                }
            };
            let got = this.update_in(cx, tag("page.vue", 1, "  <div class=\"x\">")).ok().flatten();
            check(got.as_deref() == Some("  <div class=\"x\"></div>"), "stage1: vue tag closed on >", &got);
            let got = this.update_in(cx, tag("view.jsx", 1, "  <span>")).ok().flatten();
            check(got.as_deref() == Some("  <span></span>"), "stage1: jsx tag closed on >", &got);
            let got = this.update_in(cx, tag("view.jsx", 1, "<Foo.Bar>")).ok().flatten();
            check(got.as_deref().is_some_and(|l| l.starts_with("<Foo.Bar></Foo.Bar>")), "stage1: jsx member tag closed", &got);
            let got = this.update_in(cx, tag("gen.tsx", 0, "const a: Array<string>")).ok().flatten();
            check(got.as_deref() == Some("const a: Array<string>"), "stage1: tsx no close for Array<string>", &got);
            let got = this.update_in(cx, tag("gen.tsx", 0, "const f = <T,>(x: T) => <b>")).ok().flatten();
            note("stage1: tsx generic arrow then <b>", &got);
            let got = this.update_in(cx, tag("view.jsx", 1, "<br>")).ok().flatten();
            check(got.as_deref().is_some_and(|l| !l.contains("</br>")), "stage1: jsx void br", &got);
            let got = this.update_in(cx, tag("view.jsx", 1, "<>")).ok().flatten();
            note("stage1: jsx fragment <>", &got);
            // `</` completes the innermost open tag.
            let got = this
                .update_in(cx, |t, window, cx| {
                    let f = t.root.join("index.html");
                    t.open_file(&f, window, cx);
                    on_ed(t, window, cx, |e, _, cx| {
                        e.select_on_line(0, 8..8, cx);
                        e.type_for_smoke("hi</", cx);
                        let a = e.buffer.line(0).to_string();
                        e.type_for_smoke("</", cx);
                        (a, e.buffer.line(0).to_string())
                    })
                })
                .ok()
                .flatten();
            check(got.as_ref().is_some_and(|g| g.0 == "<div><p>hi</p>" && g.1 == "<div><p>hi</p></div>"), "stage1: </ completes p then div", &got);
            // Emmet edge cases.
            let emmet = |file: &'static str, abbr: &'static str| {
                move |t: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>| {
                    let f = t.root.join(file);
                    t.open_file(&f, window, cx);
                    on_ed(t, window, cx, |e, window, cx| {
                        e.buffer.select_all();
                        e.type_for_smoke("\n", cx);
                        e.select_on_line(0, 0..0, cx);
                        e.type_for_smoke(abbr, cx);
                        e.close_completion_for_smoke();
                        e.perform(Action::EditorTab, window, cx);
                        (e.text_string(), e.caret_line_col())
                    })
                }
            };
            let got = this.update_in(cx, emmet("e.html", "div#a.b.c>p*2+span{x}")).ok().flatten();
            check(
                got.as_ref()
                    .is_some_and(|g| g.0.contains("<div id=\"a\" class=\"b c\">") && g.0.matches("<p></p>").count() == 2 && g.0.contains("<span>x</span>")),
                "stage1: Emmet div#a.b.c>p*2+span{x}",
                &got,
            );
            let got = this.update_in(cx, emmet("e.html", "ul>li*3>a")).ok().flatten();
            check(got.as_ref().is_some_and(|g| g.0.matches("<a href=\"\"></a>").count() == 3), "stage1: Emmet ul>li*3>a", &got);
            let got = this.update_in(cx, emmet("e.html", "div>p^span")).ok().flatten();
            note("stage1: Emmet div>p^span", &got);
            let got = this.update_in(cx, emmet("e.html", "input[type=checkbox name=x]")).ok().flatten();
            note("stage1: Emmet input[type=checkbox name=x]", &got);
            let got = this.update_in(cx, emmet("e.html", "  .card")).ok().flatten();
            check(got.as_ref().is_some_and(|g| g.0.contains("  <div class=\"card\"></div>")), "stage1: Emmet indented .card", &got);
            let got = this.update_in(cx, emmet("gen.tsx", "div.card>span")).ok().flatten();
            check(got.as_ref().is_some_and(|g| g.0.contains("className=\"card\"")), "stage1: Emmet className in tsx", &got);
            let got = this.update_in(cx, emmet("page.vue", "nav>a*2")).ok().flatten();
            note("stage1: Emmet in vue", &got);
            // CSS: swatches.
            let got = this
                .update_in(cx, |t, window, cx| {
                    let f = t.root.join("c.css");
                    t.open_file(&f, window, cx);
                    on_ed(t, window, cx, |e, _, _| (1..10).map(|l| (l, e.swatch(l).map(|c| (c.r, c.g, c.b, c.a)))).collect::<Vec<_>>())
                })
                .ok()
                .flatten()
                .unwrap_or_default();
            let s = |l: usize| got.iter().find(|x| x.0 == l).and_then(|x| x.1);
            let near = |a: f32, b: f32| (a - b).abs() < 0.02;
            check(s(1).is_some_and(|c| near(c.0, 0.) && near(c.1, 128. / 255.) && near(c.2, 0.)), "stage1: swatch rgb()", s(1));
            check(s(2).is_some_and(|c| near(c.0, 0.) && near(c.1, 1.) && near(c.2, 0.)), "stage1: swatch hsl()", s(2));
            check(s(3).is_some_and(|c| near(c.0, 1.) && near(c.1, 0.)), "stage1: swatch named red", s(3));
            check(s(4).is_some_and(|c| near(c.2, 1.) && near(c.3, 0.5)), "stage1: swatch rgba() alpha", s(4));
            check(s(5).is_some_and(|c| near(c.1, 1.) && near(c.3, 136. / 255.)), "stage1: swatch #0f08", s(5));
            check(s(6).is_none(), "stage1: no swatch for var(--x)", s(6));
            check(s(7).is_some(), "stage1: swatch for RED (case)", s(7));
            note("stage1: swatch transparent", s(8));
            // SCSS: property and value completion.
            let _ = this.update_in(cx, |t, window, cx| {
                let f = t.root.join("s.scss");
                t.open_file(&f, window, cx);
                on_ed(t, window, cx, |e, _, cx| {
                    e.select_on_line(2, 2..2, cx);
                    e.type_for_smoke("colo", cx);
                })
            });
            wait(cx, 300).await;
            let list = this.update(cx, |t, cx| comp(t, cx)).unwrap_or_default();
            check(list.contains("\"color\""), "stage1: SCSS property completion", &list);
            let line = this
                .update(cx, |t, cx| {
                    let e = t.active_editor().cloned()?;
                    e.update(cx, |e, cx| {
                        e.select_completion_for_smoke("color");
                        e.accept_completion(false, cx)
                    });
                    Some(e.read(cx).buffer.line(2).to_string())
                })
                .ok()
                .flatten();
            check(line.as_deref() == Some("  color: "), "stage1: SCSS accept adds ': '", &line);
            let _ = this.update(cx, |t, cx| {
                if let Some(e) = t.active_editor().cloned() {
                    e.update(cx, |e, cx| e.type_for_smoke("rebe", cx));
                }
            });
            wait(cx, 300).await;
            let list = this.update(cx, |t, cx| comp(t, cx)).unwrap_or_default();
            check(list.contains("rebeccapurple"), "stage1: SCSS value completion", &list);
            // Nested SCSS rule.
            let _ = this.update_in(cx, |t, window, cx| {
                on_ed(t, window, cx, |e, _, cx| {
                    e.close_completion_for_smoke();
                    e.buffer.select_all();
                    e.type_for_smoke("a {\n&:hover {\nmarg", cx);
                })
            });
            wait(cx, 300).await;
            let list = this.update(cx, |t, cx| comp(t, cx)).unwrap_or_default();
            check(list.contains("margin"), "stage1: SCSS completion in nested rule", &list);
            // Swatch in SCSS for a $var line.
            let sw = this.update(cx, |t, cx| t.active_editor().map(|e| e.read(cx).is_stylesheet())).ok().flatten();
            note("stage1: scss is stylesheet", sw);
            // A .ts file while Missing: still no server, offer state.
            let st = this.update(cx, |t, _| (t.lsp.ts.state.clone(), t.lsp.ts.offer)).ok();
            note("stage1: final ts state", &st);

            let n = FAILS.load(Ordering::SeqCst);
            eprintln!("smoke-probe: DONE with {n} failure(s)");
            std::process::exit(if n == 0 { 0 } else { 1 });
        })
        .detach();
    }
}
