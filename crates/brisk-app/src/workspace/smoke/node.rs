//! `BRISK_SMOKE_NODE=1` (on an empty scratch folder, with Node.js and npm on PATH): npm and
//! Node.js runs. Writes a `package.json` with two scripts, `hello.js` and `greet.ts`, then
//! checks: the ▶ marks on the script lines, a script's ▶ runs it through npm (with the
//! configuration's variable), a `.ts` file runs with node, the configurations are saved, and
//! Debug on an npm configuration says it is not available. Exits with code 1 on a failure.

use crate::editor::run_marks::RunTarget;
use crate::workspace::Workspace;
use brisk_run::{RunConfig, RunKind};
use gpui::{AsyncWindowContext, Context, WeakEntity, Window};
use std::time::Duration;

fn wait(cx: &AsyncWindowContext, ms: u64) -> gpui::Task<()> {
    cx.background_executor().timer(Duration::from_millis(ms))
}

fn check(ok: bool, what: &str, got: impl std::fmt::Debug) {
    if ok {
        eprintln!("smoke-node: ok {what}");
    } else {
        eprintln!("smoke-node: FAIL {what}: {got:?}");
        std::process::exit(1);
    }
}

/// Waits until the active Run tab's text has `expect`; the text at the end.
async fn output(this: &WeakEntity<Workspace>, expect: &str, cx: &mut AsyncWindowContext) -> String {
    let mut last = String::new();
    for _ in 0..300 {
        wait(cx, 100).await;
        last = this.update(cx, |t, cx| t.runs.panel.as_ref().and_then(|p| p.read(cx).active_text()).unwrap_or_default()).unwrap_or_default();
        if last.contains(expect) {
            break;
        }
    }
    last
}

impl Workspace {
    pub(in crate::workspace) fn node_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let pj = root.join("package.json");
        let _ =
            std::fs::write(&pj, "{\n  \"name\": \"demo\",\n  \"scripts\": {\n    \"hello\": \"node hello.js\",\n    \"greet\": \"node greet.ts\"\n  }\n}\n");
        let _ = std::fs::write(root.join("hello.js"), "console.log('hi from npm ' + process.env.FOO);\n");
        let _ = std::fs::write(root.join("greet.ts"), "const who: string = 'ts';\nconsole.log(`hi from ${who}`);\n");
        let _ = std::fs::write(
            root.join("repo.ts"),
            "export class UserRepo {\n  load = async () => {};\n  save(u) {}\n}\nexport const makeRepo = () => new UserRepo();\n",
        );
        self.open_file(&pj, window, cx);
        cx.spawn_in(window, async move |this, cx| {
            wait(cx, 500).await;
            let marks = this.update(cx, |t, cx| t.active_editor().cloned().map(|e| e.update(cx, |e, _| e.run_marks())).unwrap_or_default()).unwrap_or_default();
            let got: Vec<(usize, RunTarget)> = marks.into_iter().map(|m| (m.line, m.target)).collect();
            check(got == [(3, RunTarget::Npm("hello".into())), (4, RunTarget::Npm("greet".into()))], "marks on script lines", &got);

            // ▶ on "hello": npm run hello, with the configuration's variable.
            let pj2 = pj.clone();
            let _ = this.update_in(cx, |t, window, cx| {
                let mut c = RunConfig::for_npm(&t.root, &pj2, "hello");
                c.env.insert("FOO".into(), "bar".into());
                t.launch_context(c, false, window, cx);
            });
            let out = output(&this, "hi from npm bar", cx).await;
            check(out.contains("hi from npm bar"), "npm run hello", out.chars().take(400).collect::<String>());

            // A .ts file with node (type stripping).
            let ts = root.join("greet.ts");
            let _ = this.update_in(cx, |t, window, cx| {
                let c = RunConfig::for_node(&t.root, &ts);
                t.launch_context(c, false, window, cx);
            });
            let out = output(&this, "hi from ts", cx).await;
            check(out.contains("hi from ts"), "node greet.ts", out.chars().take(400).collect::<String>());

            let saved = brisk_run::load(&root).0;
            let npm = saved.configurations.iter().any(|c| c.kind == RunKind::Npm && c.script == "hello");
            let node = saved.configurations.iter().any(|c| c.kind == RunKind::Node && c.target == "greet.ts");
            check(npm && node, "configurations saved", saved.configurations.iter().map(|c| c.name.clone()).collect::<Vec<_>>());

            let toast = this
                .update_in(cx, |t, window, cx| {
                    t.toast = None;
                    let c = RunConfig::for_npm(&t.root, &pj, "hello");
                    t.start_debug(c, window, cx);
                    t.toast.as_ref().map(|t| t.0.clone())
                })
                .ok()
                .flatten();
            check(toast.as_deref().is_some_and(|t| t.contains("not available")), "debug blocked", &toast);
            // JS / TS symbols: Go to Class and Go to Symbol find them (the files were written
            // after the startup scan).
            let _ = this.update(cx, |t, cx| t.reindex_files(cx));
            for _ in 0..100 {
                let listed = this.update(cx, |t, _| t.files.as_ref().is_some_and(|f| f.files.iter().any(|p| p.ends_with("repo.ts")))).unwrap_or(false);
                if listed {
                    break;
                }
                wait(cx, 100).await;
            }
            for _ in 0..100 {
                let ready = this.update(cx, |t, cx| {
                    t.ensure_symbols(cx);
                    t.symbols.list.is_some() && !t.symbols.building
                });
                if ready.unwrap_or(false) {
                    break;
                }
                wait(cx, 100).await;
            }
            let found = this
                .update(cx, |t, _| {
                    let class = t.symbol_items("UserRepo", true, 5).into_iter().map(|i| i.label).collect::<Vec<_>>();
                    let func = t.symbol_items("makeRepo", false, 5).into_iter().map(|i| i.label).collect::<Vec<_>>();
                    let method = t.symbol_items("UserRepo.save", false, 5).into_iter().map(|i| i.label).collect::<Vec<_>>();
                    (class, func, method)
                })
                .unwrap_or_default();
            let ok = found.0.first().is_some_and(|l| l.contains("UserRepo")) && found.1.first().is_some_and(|l| l.contains("makeRepo")) && !found.2.is_empty();
            check(ok, "TS symbols indexed", &found);

            // Ctrl+B on an import path and on an imported name; path completion in an import.
            let main = root.join("main.ts");
            let _ = std::fs::write(&main, "import { makeRepo } from './repo';\nmakeRepo();\n");
            let at_file = |t: &Workspace, cx: &gpui::App| {
                t.active_editor().and_then(|e| {
                    let e = e.read(cx);
                    e.path.as_ref().map(|p| (p.file_name().unwrap_or_default().to_string_lossy().into_owned(), e.caret_line_col().0))
                })
            };
            let m = main.clone();
            let got = this
                .update_in(cx, |t, window, cx| {
                    t.open_file(&m, window, cx);
                    let ed = t.active_editor().cloned()?;
                    ed.update(cx, |e, cx| e.select_on_line(0, 29..29, cx));
                    t.js_goto(&ed, None, window, cx);
                    at_file(t, cx)
                })
                .ok()
                .flatten();
            check(got.as_ref().is_some_and(|g| g.0 == "repo.ts"), "Ctrl+B on './repo'", &got);
            let m = main.clone();
            let got = this
                .update_in(cx, |t, window, cx| {
                    t.open_file(&m, window, cx);
                    let ed = t.active_editor().cloned()?;
                    ed.update(cx, |e, cx| e.select_on_line(1, 2..2, cx));
                    t.js_goto(&ed, None, window, cx);
                    at_file(t, cx)
                })
                .ok()
                .flatten();
            check(got == Some(("repo.ts".into(), 4)), "Ctrl+B on an imported name", &got);
            let m = main.clone();
            let _ = this
                .update_in(cx, |t, window, cx| {
                    t.open_file(&m, window, cx);
                    let ed = t.active_editor().cloned()?;
                    ed.update(cx, |e, cx| {
                        e.select_on_line(2, 0..0, cx);
                        e.type_for_smoke("import z from './", cx);
                    });
                    Some(())
                })
                .ok()
                .flatten();
            // The Typed events reach the workspace after this update.
            wait(cx, 300).await;
            let list = this.update(cx, |t, cx| t.active_editor().map(|e| e.read(cx).insight_summary())).ok().flatten().unwrap_or_default();
            check(list.contains("repo") && list.contains("greet") && !list.contains("repo.ts"), "path completion", &list);

            // HTML: `>` closes the tag, the pair follows a rename, Tab expands Emmet.
            let page = root.join("index.html");
            let _ = std::fs::write(&page, "\n");
            let got = this
                .update_in(cx, |t, window, cx| {
                    t.open_file(&page, window, cx);
                    let ed = t.active_editor().cloned()?;
                    Some(ed.update(cx, |e, cx| {
                        e.select_on_line(0, 0..0, cx);
                        e.type_for_smoke("<section class=\"a\">", cx);
                        let closed = e.text_string();
                        // Rename: the caret at the end of `section` in the opening tag.
                        e.select_on_line(0, 8..8, cx);
                        e.type_for_smoke("s", cx);
                        let renamed = e.text_string();
                        e.perform(brisk_keymap::Action::EditorBackspace, window, cx);
                        let back = e.text_string();
                        e.select_on_line(1, 0..0, cx);
                        e.type_for_smoke("ul>li.item$*2", cx);
                        e.perform(brisk_keymap::Action::EditorTab, window, cx);
                        (closed, renamed, back, e.text_string())
                    }))
                })
                .ok()
                .flatten()
                .unwrap_or_default();
            check(got.0.starts_with("<section class=\"a\"></section>"), "tag closed on >", &got.0);
            check(got.1.starts_with("<sections class=\"a\"></sections>"), "paired tag renamed", &got.1);
            check(got.2.starts_with("<section class=\"a\"></section>"), "rename undone by Backspace", &got.2);
            check(got.3.contains("<ul>\n    <li class=\"item1\"></li>\n    <li class=\"item2\"></li>\n</ul>"), "Emmet on Tab", &got.3);
            eprintln!("smoke-node: PASS");
            std::process::exit(0);
        })
        .detach();
    }
}
