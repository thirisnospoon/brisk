//! The DEBUG smoke (`BRISK_SMOKE_DEBUG`, docs/testing.md).

use super::*;

impl Workspace {
    /// BRISK_SMOKE_DEBUG=<file.py>: breakpoint, debug, frames, variables, evaluate, steps,
    /// run to cursor, conditional breakpoint, finish.
    #[cfg(feature = "smoke")]
    pub(in crate::workspace) fn debug_smoke(&mut self, file: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file(&file, window, cx);
        let Some(ed) = self.active_editor().cloned() else { return };
        let line_of = |needle: &str, cx: &mut Context<Self>| ed.read(cx).buffer.text().lines().position(|l| l.contains(needle)).unwrap_or(0);
        let bp_line = line_of("total = add(", cx);
        ed.update(cx, |e, cx| {
            e.set_breakpoints(&[]);
            e.go_to_line(bp_line, cx);
        });
        self.dispatch(Action::ToggleBreakpoint, window, cx);
        eprintln!("smoke-debug: ctrl-f8 on line {} -> {:?}", bp_line + 1, ed.read(cx).breakpoint_list().iter().map(|(l, _)| l + 1).collect::<Vec<_>>());
        let ret_line = line_of("return a + b", cx);
        let loop_line = line_of("acc +=", cx);
        let print_line = line_of("print(\"total\"", cx);
        let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let wait = |ms| bg.timer(std::time::Duration::from_millis(ms));
            let state = |this: &gpui::WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext| this.update(cx, |t, cx| t.debug.read(cx).state()).ok().flatten();
            // Wait until paused / ended.
            macro_rules! until {
                ($pred:expr) => {{
                    let mut ok = false;
                    for _ in 0..300 {
                        wait(100).await;
                        let s = state(&this, cx);
                        if $pred(s) {
                            ok = true;
                            break;
                        }
                    }
                    ok
                }};
            }
            let paused = |s: Option<State>| matches!(s, Some(State::Paused(_)));
            // Interpreters are discovered in the background at start.
            for _ in 0..100 {
                if this.update(cx, |t, _| t.runs.all.is_some()).unwrap_or(true) {
                    break;
                }
                wait(100).await;
            }
            let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::DebugContext, w, cx));
            let ok = until!(paused);
            wait(400).await;
            let _ = this.update_in(cx, |t, _, cx| {
                let f: Vec<String> = t.debug.read(cx).frames.iter().map(|f| format!("{}:{}", f.name, f.line + 1)).collect();
                let v: Vec<String> = t.debug.read(cx).vars.iter().map(|r| format!("{}{}={}", "  ".repeat(r.depth), r.var.name, r.var.value)).collect();
                let exec = t.active_editor().and_then(|e| e.read(cx).exec_line).map(|l| l + 1);
                eprintln!("smoke-debug: paused={ok} frames={f:?} exec_line={exec:?}");
                eprintln!("smoke-debug: variables {v:?}");
                let inl: Vec<String> = t.active_editor().map(|e| (0..12).filter_map(|l| e.read(cx).inline_value(l).map(|v| format!("{}: {v}", l + 1))).collect()).unwrap_or_default();
                eprintln!("smoke-debug: inline values {inl:?}");
            });
            if std::env::var_os("BRISK_SMOKE_DEBUG_HOLD").is_some() {
                // Screenshots: stay paused at the first breakpoint.
                let _ = this.update_in(cx, |t, _, cx| {
                    if let Some(i) = t.debug.read(cx).vars.iter().position(|r| r.var.name == "items") {
                        t.toggle_var(i, cx);
                    }
                    t.add_watch("len(items)".into(), cx);
                });
                // Print what changes, for a driver that clicks the window (mouse checks).
                let mut last = String::new();
                loop {
                    wait(250).await;
                    let Ok(now) = this.update(cx, |t, cx| {
                        let bps: Vec<String> = t
                            .active_editor()
                            .map(|e| e.read(cx).breakpoint_list().into_iter().map(|(l, b)| format!("{}{}", l + 1, b.condition.map(|c| format!(" if {c}")).unwrap_or_default())).collect())
                            .unwrap_or_default();
                        format!(
                            "state={:?} top={:?} bps={bps:?} muted={} popup={} bpdlg={} tab={:?} sel={:?} edit={:?} watches={:?}",
                            t.debug.read(cx).state(),
                            t.debug.read(cx).frames.first().map(|f| f.line + 1),
                            t.debug.read(cx).muted,
                            t.popup.is_some(),
                            t.debug.read(cx).bp_dialog.as_ref().map_or("closed".to_string(), |d| format!("open(sel {} editing {})", d.sel, d.editing)),
                            t.debug.read(cx).tab,
                            t.debug.read(cx).sel.and_then(|s| t.debug.read(cx).vars.get(s)).map(|r| r.var.name.clone()),
                            t.debug.read(cx).edit.as_ref().map(|e| e.target),
                            t.debug.read(cx).watches,
                        )
                    }) else {
                        return;
                    };
                    if now != last {
                        eprintln!("smoke-hold: {now}");
                        last = now;
                    }
                }
            }
            // Expand a list variable.
            let _ = this.update_in(cx, |t, _, cx| {
                if let Some(i) = t.debug.read(cx).vars.iter().position(|r| r.var.name == "items") {
                    t.toggle_var(i, cx);
                }
            });
            wait(600).await;
            let _ = this.update_in(cx, |t, _, cx| {
                let v: Vec<String> = t.debug.read(cx).vars.iter().filter(|r| r.depth >= 2).map(|r| format!("{}={}", r.var.name, r.var.value)).collect();
                eprintln!("smoke-debug: expanded items -> {v:?}");
            });
            // Set Value of items[0], Copy Value of items.
            let _ = this.update_in(cx, |t, _, cx| {
                if let Some(i) = t.debug.read(cx).vars.iter().position(|r| r.depth >= 2 && r.var.name == "0") {
                    t.set_value(i, "10".into(), cx);
                }
            });
            wait(800).await;
            let _ = this.update_in(cx, |t, _, cx| {
                let v = t.debug.read(cx).vars.iter().find(|r| r.depth >= 2 && r.var.name == "0").map(|r| r.var.value.clone());
                if let Some(i) = t.debug.read(cx).vars.iter().position(|r| r.var.name == "items") {
                    t.copy_value(i, cx);
                }
                let clip = cx.read_from_clipboard().and_then(|c| c.text());
                eprintln!("smoke-debug: set value items[0] -> {v:?}; copy value of items -> {clip:?}");
                t.debug.as_mut(cx).watches.clear();
                for w in ["len(items)", "total", "nope_x"] {
                    t.add_watch(w.into(), cx);
                }
            });
            wait(1000).await;
            let watch_rows = |t: &Workspace, cx: &gpui::App| -> Vec<String> {
                t.debug.read(cx).vars.iter().filter(|r| matches!(r.kind, RowKind::Watch(_))).map(|r| format!("{}={}", r.var.name, r.var.value)).collect()
            };
            let _ = this.update_in(cx, |t, _, cx| {
                eprintln!("smoke-debug: watches -> {:?}", watch_rows(t, cx));
                // Edit the first watch (as F2 + typing + Enter), remove the failing one (Delete).
                if let Some(i) = t.debug.read(cx).vars.iter().position(|r| r.kind == RowKind::Watch(0)) {
                    t.debug.as_mut(cx).begin_edit(i);
                    if let Some(e) = t.debug.as_mut(cx).edit.as_mut() {
                        e.text = "len(items) + 1".into();
                    }
                    t.commit_edit(cx);
                }
                t.remove_watch(2, cx);
            });
            wait(1000).await;
            let _ = this.update_in(cx, |t, _, cx| {
                let saved = std::fs::read_to_string(watches_path()).unwrap_or_default();
                eprintln!("smoke-debug: edited watches -> {:?}; saved={}", watch_rows(t, cx), saved.contains("len(items) + 1") && !saved.contains("nope_x"));
            });
            // Evaluate in the console.
            let _ = this.update_in(cx, |t, w, cx| {
                t.dispatch(Action::EvaluateExpression, w, cx);
                t.debug.as_mut(cx).input = "sum(items) * 10".into();
                t.evaluate_input(cx);
            });
            wait(800).await;
            let _ = this.update_in(cx, |t, _, cx| {
                let tail: Vec<String> = t.debug.read(cx).console.iter().rev().take(2).map(|(l, _)| l.trim_end().to_string()).collect();
                eprintln!("smoke-debug: evaluate -> {tail:?}");
                // Tab completion: one match is inserted, several open the list.
                t.debug.as_mut(cx).input = "ite".into();
                t.complete_input(cx);
            });
            wait(800).await;
            let _ = this.update(cx, |t, cx| {
                eprintln!("smoke-debug: tab after 'ite' -> {:?} list {:?}", t.debug.read(cx).input, t.debug.read(cx).completion.as_ref().map(|c| c.shown(&t.debug.read(cx).input).iter().map(|i| i.0.clone()).collect::<Vec<_>>()));
                t.debug.as_mut(cx).completion = None;
                t.debug.as_mut(cx).input = "items.".into();
                t.complete_input(cx);
            });
            wait(800).await;
            let _ = this.update(cx, |t, cx| {
                let shown: Vec<String> = t.debug.read(cx).completion.as_ref().map(|c| c.shown(&t.debug.read(cx).input).iter().take(4).map(|i| i.0.clone()).collect()).unwrap_or_default();
                eprintln!("smoke-debug: tab after 'items.' -> list {shown:?}");
                for ch in ["c", "o"] {
                    t.debug.as_mut(cx).input.push_str(ch);
                    t.debug.as_mut(cx).refilter_completion();
                }
                let shown: Vec<String> = t.debug.read(cx).completion.as_ref().map(|c| c.shown(&t.debug.read(cx).input).iter().map(|i| i.0.clone()).collect()).unwrap_or_default();
                t.debug.as_mut(cx).completion_key("down");
                t.debug.as_mut(cx).completion_key("enter");
                eprintln!("smoke-debug: typed 'co' -> {shown:?}, down+enter -> {:?}; history file has {}", t.debug.read(cx).input, load_history().iter().any(|h| h == "sum(items) * 10"));
                t.debug.as_mut(cx).input.clear();
                cx.notify();
            });
            // F7 into add(), Shift+F8 out, F8 over.
            let mut seen = Vec::new();
            for a in [Action::StepInto, Action::StepOut, Action::StepOver] {
                let _ = this.update_in(cx, |t, w, cx| t.dispatch(a, w, cx));
                until!(paused);
                wait(500).await;
                if let Ok(s) = this.update(cx, |t, cx| t.debug.read(cx).frames.first().map(|f| format!("{}:{}", f.name, f.line + 1))) {
                    seen.push(s.unwrap_or_default());
                }
                if a == Action::StepOut {
                    let _ = this.update(cx, |t, cx| {
                        let r: Vec<String> = t.debug.read(cx).vars.iter().filter_map(|r| r.return_label().map(|l| format!("{l} = {}", r.var.value))).collect();
                        let top = t.debug.read(cx).vars.iter().find(|r| r.depth == 1).map(|r| r.var.name.clone());
                        eprintln!("smoke-debug: return values after shift-f8 -> {r:?} first local {top:?}");
                    });
                }
            }
            eprintln!("smoke-debug: f7, shift-f8, f8 -> {seen:?} (return line {})", ret_line + 1);
            wait(500).await;
            let _ = this.update_in(cx, |t, _, cx| eprintln!("smoke-debug: watches after f8 -> {:?}", watch_rows(t, cx)));
            // Conditional breakpoint inside the loop, then F9.
            let _ = this.update_in(cx, |t, _, cx| {
                if let Some(ed) = t.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.set_breakpoint_condition(loop_line, Some("i == 3".into()), cx));
                }
            });
            wait(300).await;
            let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::Resume, w, cx));
            until!(paused);
            wait(500).await;
            let _ = this.update_in(cx, |t, _, cx| {
                let i = t.debug.read(cx).vars.iter().find(|r| r.var.name == "i").map(|r| r.var.value.clone());
                let at = t.debug.read(cx).frames.first().map(|f| f.line + 1);
                let marks: Vec<(usize, Option<String>, Option<bool>)> =
                    t.active_editor().map(|e| e.read(cx).breakpoint_list().into_iter().map(|(l, b)| (l + 1, b.condition, b.verified)).collect()).unwrap_or_default();
                eprintln!("smoke-debug: conditional bp -> line {at:?} i={i:?} breakpoints {marks:?}");
            });
            // Run to cursor at the print line (the loop breakpoint removed first).
            let _ = this.update_in(cx, |t, w, cx| {
                if let Some(ed) = t.active_editor().cloned() {
                    ed.update(cx, |e, cx| {
                        e.toggle_breakpoint(loop_line, cx);
                        e.go_to_line(print_line, cx);
                    });
                }
                t.dispatch(Action::RunToCursor, w, cx);
            });
            until!(paused);
            wait(500).await;
            let _ = this.update_in(cx, |t, _, cx| eprintln!("smoke-debug: run to cursor -> {:?}", t.debug.read(cx).frames.first().map(|f| f.line + 1)));
            let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::Resume, w, cx));
            let ended = until!(|s: Option<State>| s == Some(State::Ended));
            wait(300).await;
            let _ = this.update_in(cx, |t, _, cx| {
                let out: Vec<String> = t.debug.read(cx).console.iter().filter(|(_, k)| matches!(k, Line::Out | Line::Info)).map(|(l, _)| l.trim_end().to_string()).collect();
                let exec = t.active_editor().and_then(|e| e.read(cx).exec_line);
                let inl = t.active_editor().map(|e| (0..12).any(|l| e.read(cx).inline_value(l).is_some()));
                eprintln!("smoke-debug: inline values left after the end -> {inl:?}");
                eprintln!("smoke-debug: finished={ended} exec_line={exec:?} console tail {:?}", out.iter().rev().take(3).collect::<Vec<_>>());
                eprintln!("smoke-debug: stored -> {}", std::fs::read_to_string(store_path()).map(|s| s.contains("dbg_main.py")).unwrap_or(false));
            });
            // Pause a busy program, rerun it, stop it while it runs.
            let spin = dir.join("dbg_spin.py");
            let _ = std::fs::write(&spin, "import time\n\nn = 0\nwhile True:\n    n += 1\n    time.sleep(0.01)\n");
            let _ = this.update_in(cx, |t, w, cx| {
                t.open_file(&spin, w, cx);
                t.dispatch(Action::DebugContext, w, cx);
            });
            let running = until!(|s: Option<State>| s == Some(State::Running));
            wait(1500).await;
            let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::PauseProgram, w, cx));
            let p = until!(paused);
            wait(500).await;
            let _ = this.update_in(cx, |t, _, cx| {
                let n = t.debug.read(cx).vars.iter().find(|r| r.var.name == "n").map(|r| r.var.value.clone());
                eprintln!("smoke-debug: pause running={running} paused={p} at {:?} n={n:?}", t.debug.read(cx).frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)));
            });
            let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::RerunDebug, w, cx));
            let rerun = until!(|s: Option<State>| s == Some(State::Running));
            wait(800).await;
            let _ = this.update_in(cx, |t, w, cx| {
                eprintln!("smoke-debug: rerun -> running={rerun} name={:?}", t.debug.read(cx).session.as_ref().map(|s| s.name.clone()));
                t.dispatch(Action::Stop, w, cx);
            });
            let stopped = until!(|s: Option<State>| s == Some(State::Ended));
            // Stop while debugpy is still starting: no session may appear afterwards.
            let _ = this.update_in(cx, |t, w, cx| {
                t.dispatch(Action::RerunDebug, w, cx);
                let pending = t.debug.read(cx).pending;
                t.dispatch(Action::Stop, w, cx);
                eprintln!("smoke-debug: stop -> {stopped}; stop while starting: pending={pending} after={}", t.debug.read(cx).pending);
            });
            wait(3000).await;
            let _ = this.update_in(cx, |t, _, cx| eprintln!("smoke-debug: late start dropped -> state={:?}", t.debug.read(cx).state()));
            // An uncaught exception: red header, traceback links in the console.
            let exc = dir.join("dbg_exc.py");
            let _ = std::fs::write(&exc, "def f(x):\n    return 1 / x\n\n\nf(0)\n");
            let _ = this.update_in(cx, |t, w, cx| {
                t.open_file(&exc, w, cx);
                t.dispatch(Action::DebugContext, w, cx);
            });
            let p = until!(paused);
            wait(1000).await;
            let _ = this.update_in(cx, |t, w, cx| {
                let links: Vec<(PathBuf, usize)> = t
                    .debug
                    .read(cx)
                    .console
                    .iter()
                    .filter(|(_, k)| matches!(k, Line::Err | Line::Out))
                    .flat_map(|(l, _)| brisk_run::find_links(l))
                    .filter_map(|l| t.debug.read(cx).console_link(&l.path, l.line))
                    .collect();
                eprintln!("smoke-debug: exception paused={p} header={:?} frame={:?} links={:?}", t.debug.read(cx).exception, t.debug.read(cx).frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)), links.iter().map(|(p, l)| format!("{}:{l}", p.file_name().unwrap_or_default().to_string_lossy())).collect::<Vec<_>>());
                let err: Vec<&str> = t.debug.read(cx).console.iter().filter(|(_, k)| *k == Line::Err).map(|(l, _)| l.trim_end()).collect();
                eprintln!("smoke-debug: exception console {err:?}");
                // Click on the first link: as the console's on_click does.
                if let Some((p, l)) = links.first().cloned() {
                    t.open_file(&spin, w, cx);
                    t.open_console_link(&p, l, w, cx);
                    let at = t.active_editor().map(|e| (e.read(cx).path.clone().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string())), e.read(cx).caret_line_col().0 + 1));
                    eprintln!("smoke-debug: link opens -> {at:?}");
                }
                t.dispatch(Action::Stop, w, cx);
            });
            until!(|s: Option<State>| s == Some(State::Ended));
            // A new session reached a stop or its end (the previous one is Ended too).
            macro_rules! settled {
                () => {{
                    let mut st = None;
                    for _ in 0..300 {
                        wait(100).await;
                        let (pending, s) = this.update(cx, |t, cx| (t.debug.read(cx).pending, t.debug.read(cx).state())).unwrap_or((false, None));
                        if !pending && matches!(s, Some(State::Paused(_) | State::Ended)) {
                            st = s;
                            break;
                        }
                    }
                    st
                }};
            }
            // View Breakpoints: a disabled breakpoint is skipped; "on raised" stops on a handled exception.
            let raise = dir.join("dbg_raise.py");
            let _ = std::fs::write(&raise, "def f():\n    try:\n        raise ValueError(\"handled\")\n    except ValueError:\n        pass\n    x = 1\n    return x\n\n\nf()\n");
            let _ = this.update_in(cx, |t, w, cx| {
                eprintln!("smoke-debug: after stop exception={:?}", t.debug.read(cx).exception);
                t.open_file(&raise, w, cx);
                if let Some(ed) = t.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.toggle_breakpoint(5, cx));
                }
                t.dispatch(Action::ViewBreakpoints, w, cx);
                let rows: Vec<String> = t
                    .bp_rows(cx)
                    .iter()
                    .map(|r| match r {
                        BpRow::Raised => format!("raised={}", t.debug.read(cx).exc_raised),
                        BpRow::Uncaught => format!("uncaught={}", t.debug.read(cx).exc_uncaught),
                        BpRow::Class(_, c) => format!("class {}", c.name),
                        BpRow::Line(p, l, b) => format!("{}:{} enabled={}", p.file_name().unwrap_or_default().to_string_lossy(), l + 1, b.enabled),
                    })
                    .collect();
                eprintln!("smoke-debug: view breakpoints open={} rows {rows:?}", t.debug.read(cx).bp_dialog.is_some());
                t.bp_set_enabled(&raise, 5, false, cx);
                t.set_exception_breakpoints(false, true, cx);
                let sent = t.source_breakpoints(&raise, cx).len();
                let shown = t.active_editor().map(|e| e.read(cx).breakpoint_list().into_iter().map(|(l, b)| (l + 1, b.enabled)).collect::<Vec<_>>());
                eprintln!("smoke-debug: disabled -> editor {shown:?}, sent to debugpy {sent}");
                t.debug.as_mut(cx).bp_dialog = None;
                t.dispatch(Action::DebugContext, w, cx);
            });
            let s1 = settled!();
            let _ = this.update_in(cx, |t, w, cx| {
                eprintln!("smoke-debug: disabled bp + raised off -> {s1:?}");
                t.set_exception_breakpoints(true, true, cx);
                t.dispatch(Action::DebugContext, w, cx);
            });
            let s2 = settled!();
            wait(800).await;
            let _ = this.update_in(cx, |t, w, cx| {
                let saved = std::fs::read_to_string(exc_path()).unwrap_or_default().replace(char::is_whitespace, "");
                eprintln!(
                    "smoke-debug: raised on -> {s2:?} at {:?} header={:?} saved={}",
                    t.debug.read(cx).frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)),
                    t.debug.read(cx).exception,
                    saved.contains("\"raised\":true")
                );
                t.dispatch(Action::Stop, w, cx);
                t.set_exception_breakpoints(false, true, cx);
                t.bp_set_condition(&raise, 5, Some("x > 0".into()), cx);
                let c = t.bp_rows(cx).into_iter().find_map(|r| match r {
                    BpRow::Line(_, 5, b) => Some((b.condition, b.enabled)),
                    _ => None,
                });
                t.bp_remove(&raise, 5, cx);
                eprintln!("smoke-debug: condition {c:?}; after remove rows={}", t.bp_rows(cx).len());
            });
            until!(|s: Option<State>| s == Some(State::Ended));
            // pytest: one test (caret inside it), then the whole file; a breakpoint in the test.
            let test = dir.join("test_dbg_calc.py");
            let _ = std::fs::write(&test, "def double(x):\n    return x * 2\n\n\ndef test_double():\n    value = double(21)\n    assert value == 42\n\n\ndef test_other():\n    assert double(1) == 2\n");
            for (caret, what) in [(5usize, "one test"), (0, "file")] {
                let _ = this.update_in(cx, |t, w, cx| {
                    t.open_file(&test, w, cx);
                    if let Some(ed) = t.active_editor().cloned() {
                        ed.update(cx, |e, cx| {
                            if !e.breakpoint_list().iter().any(|(l, _)| *l == 6) {
                                e.toggle_breakpoint(6, cx);
                            }
                            e.go_to_line(caret, cx);
                        });
                    }
                    t.dispatch(Action::DebugContext, w, cx);
                });
                let s = settled!();
                wait(800).await;
                let _ = this.update_in(cx, |t, w, cx| {
                    let cmd = t.debug.read(cx).console.first().map(|(l, _)| l.trim_end().rsplit(' ').take(2).collect::<Vec<_>>().join(" ")).unwrap_or_default();
                    let value = t.debug.read(cx).vars.iter().find(|r| r.var.name == "value").map(|r| r.var.value.clone());
                    eprintln!("smoke-debug: pytest {what}: {s:?} at {:?} value={value:?} cmd tail {cmd:?}", t.debug.read(cx).frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)));
                    t.dispatch(Action::Resume, w, cx);
                });
                until!(|s: Option<State>| s == Some(State::Ended));
                wait(300).await;
                let _ = this.update_in(cx, |t, _, cx| {
                    let summary = t.debug.read(cx).console.iter().rev().map(|(l, _)| l.trim()).find(|l| l.contains("passed") || l.contains("failed"));
                    eprintln!("smoke-debug: pytest {what} result {summary:?}");
                });
            }
            let thr = dir.join("dbg_threads.py");
            let _ = std::fs::write(&thr, "import threading\nev = threading.Event()\ndef work(n):\n    x = n * 2\n    ev.set()\n    return x\ndef main():\n    t = threading.Thread(target=work, args=(5,), name=\"Worker\")\n    t.start()\n    t.join()\nmain()\n");
            let _ = this.update_in(cx, |t, w, cx| {
                t.open_file(&thr, w, cx);
                if let Some(ed) = t.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.toggle_breakpoint(4, cx));
                }
                t.dispatch(Action::DebugContext, w, cx);
            });
            let s = settled!();
            let mut threads = Vec::new();
            for _ in 0..50 {
                wait(100).await;
                threads = this.update(cx, |t, cx| t.debug.read(cx).threads.clone()).unwrap_or_default();
                if !threads.is_empty() {
                    break;
                }
            }
            let main_id = threads.iter().find(|t| t.1 == "MainThread").map(|t| t.0);
            let _ = this.update_in(cx, |t, w, cx| {
                let shown = t.render_threads(cx).is_some();
                eprintln!(
                    "smoke-debug: threads {s:?} at {:?} threads={:?} chooser={shown}",
                    t.debug.read(cx).frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)),
                    threads.iter().map(|t| t.1.as_str()).collect::<Vec<_>>()
                );
                if let Some(id) = main_id {
                    t.select_thread(id, w, cx);
                }
            });
            wait(1500).await;
            let _ = this.update_in(cx, |t, w, cx| {
                let frames: Vec<String> = t.debug.read(cx).frames.iter().map(|f| format!("{}:{}", f.name, f.line + 1)).collect();
                eprintln!("smoke-debug: main thread -> paused={:?} frames has main:10 {}", t.debug.read(cx).paused_thread() == main_id, frames.iter().any(|f| f == "main:10"));
                t.dispatch(Action::Resume, w, cx);
            });
            until!(|s: Option<State>| s == Some(State::Ended));
            let _ = this.update_in(cx, |t, _, cx| {
                eprintln!("smoke-debug: threads after end {:?} {:?}", t.debug.read(cx).threads, t.debug.read(cx).console.iter().rev().take(3).map(|l| l.0.trim().to_string()).collect::<Vec<_>>());
                let _ = std::fs::remove_file(&thr);
                let _ = cx;
            });
            let smart = dir.join("dbg_smart.py");
            let _ = std::fs::write(&smart, "def a(x):\n    return x + 1\n\n\ndef b(x):\n    return x * 2\n\n\nr = a(b(3))\nprint(r)\n");
            let _ = this.update_in(cx, |t, w, cx| {
                t.open_file(&smart, w, cx);
                if let Some(ed) = t.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.toggle_breakpoint(8, cx));
                }
                t.dispatch(Action::DebugContext, w, cx);
            });
            let s = settled!();
            wait(800).await;
            let _ = this.update_in(cx, |t, w, cx| {
                eprintln!("smoke-debug: smart step {s:?} at {:?}", t.debug.read(cx).frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)));
                t.dispatch(Action::SmartStepInto, w, cx);
            });
            let mut labels = Vec::new();
            for _ in 0..50 {
                wait(100).await;
                labels = this.update(cx, |t, cx| t.debug.read(cx).chooser.as_ref().map(|s| s.targets.iter().map(|t| t.1.clone()).collect::<Vec<_>>())).ok().flatten().unwrap_or_default();
                if !labels.is_empty() {
                    break;
                }
            }
            let _ = this.update_in(cx, |t, w, cx| {
                let i = labels.iter().position(|l| l.starts_with("a")).unwrap_or(0);
                eprintln!("smoke-debug: smart step targets {labels:?} -> choose {:?}", labels.get(i));
                t.chooser_go(i, w, cx);
            });
            let s = settled!();
            wait(800).await;
            let _ = this.update_in(cx, |t, w, cx| {
                let x = t.debug.read(cx).vars.iter().find(|r| r.var.name == "x").map(|r| r.var.value.clone());
                eprintln!("smoke-debug: smart step into a -> {s:?} at {:?} x={x:?}", t.debug.read(cx).frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)));
                t.dispatch(Action::Resume, w, cx);
            });
            until!(|s: Option<State>| s == Some(State::Ended));
            // Exception breakpoint for one class: a handled KeyError passes, a handled ValueError stops.
            let excf = dir.join("dbg_exc_class.py");
            let _ = std::fs::write(&excf, "def f():\n    try:\n        raise KeyError(\"k\")\n    except KeyError:\n        pass\n    try:\n        raise ValueError(\"v\")\n    except ValueError:\n        pass\n    return 1\n\n\nf()\n");
            let _ = this.update_in(cx, |t, w, cx| {
                t.open_file(&excf, w, cx);
                t.dispatch(Action::ViewBreakpoints, w, cx);
                t.bp_add_class(cx);
                for ch in ["V", "a", "l", "u", "e", "E", "r", "r", "o", "r"] {
                    let ks = gpui::Keystroke { modifiers: gpui::Modifiers::default(), key: ch.to_lowercase(), key_char: Some(ch.into()) };
                    t.bp_dialog_key(&gpui::KeyDownEvent { keystroke: ks, is_held: false }, w, cx);
                }
                let ks = gpui::Keystroke { modifiers: gpui::Modifiers::default(), key: "enter".into(), key_char: None };
                t.bp_dialog_key(&gpui::KeyDownEvent { keystroke: ks, is_held: false }, w, cx);
                let saved = std::fs::read_to_string(exc_path()).unwrap_or_default().replace(char::is_whitespace, "");
                eprintln!(
                    "smoke-debug: exception class {:?} args {} saved={}",
                    t.debug.read(cx).exc_classes.iter().map(|c| (c.name.clone(), c.raised, c.uncaught)).collect::<Vec<_>>(),
                    t.debug.read(cx).exception_args(),
                    saved.contains("\"name\":\"ValueError\"")
                );
                t.debug.as_mut(cx).bp_dialog = None;
                t.dispatch(Action::DebugContext, w, cx);
            });
            let s = settled!();
            wait(800).await;
            let _ = this.update_in(cx, |t, w, cx| {
                eprintln!("smoke-debug: class ValueError -> {s:?} at {:?} header={:?}", t.debug.read(cx).frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)), t.debug.read(cx).exception);
                if let Some(i) = t.debug.read(cx).exc_classes.iter().position(|c| c.name == "ValueError") {
                    t.bp_remove_class(i, cx);
                }
                t.dispatch(Action::Resume, w, cx);
            });
            until!(|s: Option<State>| s == Some(State::Ended));
            let _ = std::fs::remove_file(&excf);
            // Jump to Cursor: back to `x = 1` (x stays 2 until stepped), then into another frame (refused).
            let jump = dir.join("dbg_jump.py");
            let _ = std::fs::write(&jump, "def f():\n    x = 1\n    x = 2\n    x = 3\n    return x\n\n\nr = f()\nprint(\"r\", r)\n");
            let _ = this.update_in(cx, |t, w, cx| {
                t.open_file(&jump, w, cx);
                if let Some(ed) = t.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.toggle_breakpoint(3, cx));
                }
                t.dispatch(Action::DebugContext, w, cx);
            });
            let s = settled!();
            wait(800).await;
            let var = |t: &Workspace, cx: &gpui::App, n: &str| t.debug.read(cx).vars.iter().find(|r| r.var.name == n).map(|r| r.var.value.clone());
            let top = |t: &Workspace, cx: &gpui::App| t.debug.read(cx).frames.first().map(|f| format!("{}:{}", f.name, f.line + 1));
            let _ = this.update_in(cx, |t, w, cx| {
                eprintln!("smoke-debug: jump {s:?} at {:?} x={:?}", top(t, cx), var(t, cx, "x"));
                if let Some(ed) = t.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.go_to_line(1, cx));
                }
                t.dispatch(Action::SetNextStatement, w, cx);
            });
            let mut at = None;
            for _ in 0..50 {
                wait(100).await;
                at = this.update(cx, |t, cx| top(t, cx)).ok().flatten();
                if at.as_deref() == Some("f:2") {
                    break;
                }
            }
            wait(600).await;
            let _ = this.update_in(cx, |t, w, cx| {
                eprintln!("smoke-debug: jumped -> {at:?} x={:?}", var(t, cx, "x"));
                t.dispatch(Action::StepOver, w, cx);
            });
            let s = settled!();
            wait(800).await;
            let _ = this.update_in(cx, |t, _, cx| {
                eprintln!("smoke-debug: after step {s:?} at {:?} x={:?}", top(t, cx), var(t, cx, "x"));
                if let Some(ed) = t.active_editor().cloned() {
                    t.set_next_statement(ed, 8, cx);
                }
            });
            wait(2000).await;
            let _ = this.update_in(cx, |t, w, cx| {
                eprintln!("smoke-debug: jump out of f -> at {:?} console {:?}", top(t, cx), t.debug.read(cx).console.last().map(|l| l.0.trim().to_string()));
                if let Some(ed) = t.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.toggle_breakpoint(3, cx));
                }
                let _ = w;
            });
            // The removed breakpoint reaches debugpy first.
            wait(500).await;
            let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::Resume, w, cx));
            let mut states = Vec::new();
            for _ in 0..60 {
                wait(100).await;
                let st = this.update(cx, |t, cx| t.debug.read(cx).state()).ok().flatten();
                if states.last() != Some(&st) {
                    states.push(st);
                }
                if st == Some(State::Ended) {
                    break;
                }
            }
            wait(1000).await;
            let _ = this.update(cx, |t, cx| {
                let _ = cx;
                eprintln!("smoke-debug: jump states {states:?} output {:?}", t.debug.read(cx).console.iter().rev().map(|l| l.0.trim()).find(|l| l.starts_with("r ")));
            });
            let _ = std::fs::remove_file(&jump);
            let spin2 = dir.join("dbg_attach_target.py");
            let _ = std::fs::write(&spin2, "import time\n\nn = 0\nwhile True:\n    n += 1\n    time.sleep(0.05)\n");
            let python = this.update(cx, |t, _| t.runs.interpreter(&t.root).map(|i| i.path)).ok().flatten();
            let child = python.and_then(|p| std::process::Command::new(p).arg(&spin2).spawn().ok());
            wait(1500).await;
            if let Some(mut child) = child {
                let pid = child.id();
                let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::AttachToProcess, w, cx));
                let mut listed = Vec::new();
                for _ in 0..100 {
                    wait(100).await;
                    listed = this.update(cx, |t, cx| t.debug.read(cx).chooser.as_ref().map(|c| c.targets.iter().map(|x| x.0).collect::<Vec<_>>())).ok().flatten().unwrap_or_default();
                    if !listed.is_empty() {
                        break;
                    }
                }
                let _ = this.update_in(cx, |t, w, cx| {
                    eprintln!("smoke-debug: attach list has target {} (of {})", listed.contains(&(pid as i64)), listed.len());
                    t.open_file(&spin2, w, cx);
                    if let Some(ed) = t.active_editor().cloned() {
                        ed.update(cx, |e, cx| e.toggle_breakpoint(4, cx));
                    }
                    if let Some(i) = listed.iter().position(|x| *x == pid as i64) {
                        t.chooser_go(i, w, cx);
                    } else {
                        t.debug.as_mut(cx).chooser = None;
                    }
                });
                let s = settled!();
                wait(800).await;
                let _ = this.update_in(cx, |t, w, cx| {
                    let n = t.debug.read(cx).vars.iter().find(|r| r.var.name == "n").map(|r| r.var.value.clone());
                    eprintln!("smoke-debug: attached -> {s:?} at {:?} n={n:?}", t.debug.read(cx).frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)));
                    if let Some(ed) = t.active_editor().cloned() {
                        ed.update(cx, |e, cx| e.toggle_breakpoint(4, cx));
                    }
                    t.dispatch(Action::Stop, w, cx);
                });
                wait(2500).await;
                let alive = matches!(child.try_wait(), Ok(None));
                let _ = this.update(cx, |t, cx| eprintln!("smoke-debug: detach -> state={:?} process alive={alive} console {:?}", t.debug.read(cx).state(), t.debug.read(cx).console.last().map(|l| l.0.trim().to_string())));
                // Rerun attaches to the same process; once it has exited, a message.
                let _ = this.update_in(cx, |t, w, cx| {
                    if let Some(ed) = t.active_editor().cloned() {
                        ed.update(cx, |e, cx| e.toggle_breakpoint(4, cx));
                    }
                    t.dispatch(Action::RerunDebug, w, cx);
                });
                let mut again = None;
                for _ in 0..150 {
                    wait(100).await;
                    again = this.update(cx, |t, cx| t.debug.read(cx).session.as_ref().filter(|s| s.attached && matches!(s.state, State::Paused(_))).map(|s| s.name.clone())).ok().flatten();
                    if again.is_some() {
                        break;
                    }
                }
                let _ = this.update_in(cx, |t, w, cx| {
                    eprintln!("smoke-debug: rerun after attach -> {again:?} (pid {pid}) at {:?}", t.debug.read(cx).frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)));
                    if let Some(ed) = t.active_editor().cloned() {
                        ed.update(cx, |e, cx| e.toggle_breakpoint(4, cx));
                    }
                    t.dispatch(Action::Stop, w, cx);
                });
                wait(1500).await;
                let _ = child.kill();
                let _ = child.wait();
                wait(500).await;
                let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::RerunDebug, w, cx));
                wait(4000).await;
                let _ = this.update(cx, |t, cx| {
                    eprintln!("smoke-debug: rerun after exit -> live={} console {:?}", t.debug.read(cx).is_live(), t.debug.read(cx).console.last().map(|l| l.0.trim().to_string()));
                });
            }
            let _ = this.update_in(cx, |_, _, cx| {
                let _ = std::fs::remove_file(&spin2);
                let _ = std::fs::remove_file(&smart);
                let _ = std::fs::remove_file(&raise);
                let _ = std::fs::remove_file(&test);
                let _ = std::fs::remove_file(&exc);
                let _ = std::fs::remove_file(&spin);
                eprintln!("smoke-debug: done");
                cx.quit();
            });
        })
        .detach();
    }
}
