//! Debug sessions: start, attach, stop, stepping, adapter events, Smart Step Into and Attach to Process.

use super::*;

impl Workspace {
    pub(in crate::workspace) fn debug_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) -> bool {
        use Action as A;
        match action {
            A::ToggleBreakpoint => self.toggle_breakpoint_here(window, cx),
            A::ViewBreakpoints => self.open_bp_dialog(cx),
            A::MuteBreakpoints => self.toggle_mute_breakpoints(cx),
            A::SmartStepInto => self.smart_step_into(cx),
            A::AttachToProcess => self.list_processes(cx),
            A::SetNextStatement => {
                let Some(ed) = self.focused_editor(window, cx).or_else(|| self.active_editor().cloned()) else { return true };
                let line = ed.read(cx).caret_line_col().0;
                self.set_next_statement(ed, line, cx);
            }
            A::Debug => {
                self.reload_runs();
                let cfg = self.runs.file.selected_config().cloned().or_else(|| self.context_config(window, cx));
                match cfg {
                    Some(cfg) => self.start_debug(cfg, window, cx),
                    None => self.status = "Nothing to debug: open a Python file or select a configuration".into(),
                }
            }
            A::DebugContext => {
                let Some(ctx) = self.context_config(window, cx) else {
                    self.status = "Debug: open a Python file first".into();
                    return true;
                };
                self.launch_context(ctx, true, window, cx);
            }
            A::Resume | A::StepOver | A::StepInto | A::StepOut | A::RunToCursor => {
                let Some(thread) = self.debug.paused_thread() else {
                    self.status = "The debugger is not paused".into();
                    return true;
                };
                let command = match action {
                    A::StepOver => "next",
                    A::StepInto => "stepIn",
                    A::StepOut => "stepOut",
                    _ => "continue",
                };
                if action == A::RunToCursor {
                    let Some(ed) = self.active_editor().cloned() else { return true };
                    let (path, line) = (ed.read(cx).path.clone(), ed.read(cx).caret_line_col().0);
                    let Some(path) = path else { return true };
                    if let Some(s) = self.debug.session.as_mut() {
                        s.temp = Some((path.clone(), line));
                    }
                    self.send_breakpoints(&path, cx);
                }
                self.debug_request(command, json!({"threadId": thread}), cx);
                self.set_running(cx);
            }
            A::Stop if self.debug.is_live() => self.stop_debug(cx),
            A::PauseProgram => {
                if self.debug.state() != Some(State::Running) {
                    self.status = "Pause: the program is not running".into();
                    return true;
                }
                self.pause_debug(cx);
            }
            A::AddToWatches => {
                // The selection or the word at the caret; nothing there: type one in.
                let text = self.active_editor().map(|ed| {
                    let e = ed.read(cx);
                    let sel = e.buffer.selected_text();
                    if sel.trim().is_empty() {
                        e.buffer.slice(e.buffer.word_range_at(e.buffer.primary().head))
                    } else {
                        sel
                    }
                });
                let text = text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty() && !t.contains('\n'));
                self.debug.tab = DebugTab::Debugger;
                self.bottom = Some(BottomTool::Debug);
                match text {
                    Some(t) if self.focused_editor(window, cx).is_some() => self.add_watch(t, cx),
                    _ => self.debug.edit = Some(VarEdit { target: EditTarget::NewWatch, text: String::new() }),
                }
                window.focus(&self.debug.focus.clone());
            }
            A::RerunDebug => match (self.debug.last.clone(), self.debug.last_attach.clone()) {
                (Some(cfg), _) => self.start_debug(cfg, window, cx),
                (None, Some((pid, label))) => self.reattach(pid, label, window, cx),
                (None, None) => self.status = "Rerun: nothing has been debugged yet".into(),
            },
            A::EvaluateExpression => {
                if self.debug.paused_thread().is_none() {
                    self.status = "Evaluate: the debugger is not paused".into();
                    return true;
                }
                // The selection or the word at the caret goes into the console's input.
                if let Some(ed) = self.active_editor() {
                    let e = ed.read(cx);
                    let sel = e.buffer.selected_text();
                    let t = if sel.trim().is_empty() {
                        let r = e.buffer.word_range_at(e.buffer.primary().head);
                        e.buffer.slice(r)
                    } else {
                        sel
                    };
                    self.debug.input = t.trim().to_string();
                }
                self.debug.tab = DebugTab::Console;
                self.bottom = Some(BottomTool::Debug);
                window.focus(&self.debug.focus.clone());
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Send a request; an error reply goes to the status bar (a refused step, a stale thread).
    pub(super) fn debug_request(&self, command: &'static str, args: Value, cx: &mut Context<Self>) {
        let Some(s) = &self.debug.session else { return };
        let reply = s.client.request(command, args);
        cx.spawn(async move |this, cx| {
            // A dropped reply means the adapter went away: the session's end says so.
            if let Ok(Err(e)) = reply.await {
                let _ = this.update(cx, |this, cx| {
                    this.status = format!("Debugger: {command} failed: {e}");
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn set_running(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.debug.session.as_mut() {
            if s.state != State::Ended {
                s.state = State::Running;
            }
        }
        self.debug.frames.clear();
        self.debug.threads.clear();
        self.debug.threads_open = false;
        self.debug.idle_watches();
        self.debug.exception = None;
        self.clear_exec_marks(cx);
        self.stale_data_views("Running: rows of the last stop", cx);
    }

    pub(super) fn clear_exec_marks(&mut self, cx: &mut Context<Self>) {
        let marked = std::mem::take(&mut self.debug.marked);
        for e in self.editors().cloned().collect::<Vec<_>>() {
            if e.read(cx).path.as_ref().is_some_and(|p| marked.contains(p)) || e.read(cx).exec_line.is_some() {
                e.update(cx, |e, cx| {
                    e.exec_line = None;
                    e.clear_inline_values();
                    cx.notify();
                });
            }
        }
    }

    /// Pause Program: debugpy pauses all threads; the main one is asked.
    pub(super) fn pause_debug(&mut self, cx: &mut Context<Self>) {
        let Some(s) = &self.debug.session else { return };
        let client = s.client.clone();
        let threads = client.request("threads", json!({}));
        cx.spawn(async move |this, cx| {
            let list = match threads.await {
                Ok(Ok(v)) => v.get("threads").and_then(Value::as_array).cloned().unwrap_or_default(),
                _ => Vec::new(),
            };
            let main = list.iter().find(|t| t.get("name").and_then(Value::as_str) == Some("MainThread")).or(list.first());
            let id = main.and_then(|t| t.get("id")).and_then(Value::as_i64).unwrap_or(0);
            if let Ok(Err(e)) = client.request("pause", json!({"threadId": id})).await {
                let _ = this.update(cx, |this, cx| {
                    this.debug.info(format!("Cannot pause: {e}"));
                    cx.notify();
                });
            }
        })
        .detach();
        self.status = "Pausing…".into();
    }

    /// Debug a configuration (also pytest ones: a file, a folder or `file::test`).
    pub(in crate::workspace) fn start_debug(&mut self, cfg: RunConfig, window: &mut Window, cx: &mut Context<Self>) {
        if cfg.kind == brisk_run::RunKind::Compound {
            // One debug session at a time: debug the members one by one.
            self.show_toast(format!("'{}' is a Compound: run it, or debug its configurations one at a time", cfg.name), true, cx);
            return;
        }
        if self.remote_blocks(&cfg, "Debugging", cx) {
            return;
        }
        if self.before_launch(&cfg, crate::workspace::before_launch_ui::Then::Debug, window, cx) {
            return;
        }
        self.save_all(cx);
        if self.debug.is_live() {
            self.stop_debug(cx);
        }
        self.debug.last = Some(cfg.clone());
        self.debug.last_attach = None;
        let python = match &cfg.interpreter {
            Some(p) => Some(brisk_run::resolve(&self.root, p)),
            None => self.runs.interpreter(&self.root).map(|i| i.path),
        };
        let Some(python) = python else {
            self.show_toast("No Python interpreter configured: create a .venv or install Python".into(), true, cx);
            return;
        };
        // pytest runs as `-m pytest <targets>` (a file, a folder or `file::test`); without the
        // Run window's reporting plugin its output is plain text in the console.
        if !self.env_files_ok(&cfg, cx) {
            return;
        }
        let cmd = brisk_run::build(&self.root, &cfg, &python);
        let launch = launch_for(&cmd);
        self.debug.cwd = launch.cwd.clone();
        self.debug.exception = None;
        let name = cfg.name.clone();
        self.debug.console.clear();
        self.debug.console_changed(false);
        self.debug.frames.clear();
        self.debug.idle_watches();
        self.debug.tab = DebugTab::Console;
        self.debug.info(cmd.display());
        self.bottom = Some(BottomTool::Debug);
        self.status = format!("Debugging '{name}'");
        let root = self.root.clone();
        self.debug.gen += 1;
        self.debug.pending = true;
        self.debug.started = Some(std::time::Instant::now());
        let gen = self.debug.gen;
        cx.spawn_in(window, async move |this, cx| {
            let adapter = cx
                .background_executor()
                .spawn({
                    let python = python.clone();
                    async move { brisk_dap::adapter_command(&python) }
                })
                .await;
            let started = adapter.map(|a| Client::start(&a.program, &a.args, &root, &[]).map_err(|e| format!("cannot start debugpy: {e}")));
            let _ = this.update_in(cx, |this, window, cx| {
                if this.debug.gen != gen {
                    // Stopped (or started again) while debugpy was starting.
                    if let Ok(Ok((client, _))) = &started {
                        client.kill();
                    }
                    return;
                }
                this.debug.pending = false;
                match started {
                    Err(missing) => this.offer_debugpy(missing, python, crate::workspace::install_offer_ui::Then::Debug(cfg), cx),
                    Ok(Err(e)) => {
                        this.debug.info(e.clone());
                        this.show_toast(e, true, cx);
                    }
                    Ok(Ok((client, events))) => this.attach_session(client, events, name, ("launch", brisk_dap::launch_args(&launch)), window, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// A started adapter becomes the session: initialize, then `req` (launch or attach).
    pub(super) fn attach_session(
        &mut self,
        client: Arc<Client>,
        mut events: futures::channel::mpsc::UnboundedReceiver<Event>,
        name: String,
        req: (&'static str, Value),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some(ev) = events.next().await {
                if this.update_in(cx, |this, window, cx| this.debug_event(ev, window, cx)).is_err() {
                    break;
                }
            }
        });
        let attached = req.0 == "attach";
        self.debug.session = Some(Session { client: client.clone(), name, state: State::Starting, _events: task, temp: None, exit_code: None, attached });
        cx.spawn(async move |this, cx| {
            let init = client.request("initialize", brisk_dap::initialize_args()).await;
            let err = match init {
                Ok(Ok(_)) => None,
                Ok(Err(e)) => Some(e),
                Err(_) => Some("no reply".to_string()),
            };
            if let Some(e) = err {
                let _ = this.update(cx, |this, cx| {
                    this.debug.info(format!("debugpy did not start: {e}"));
                    cx.notify();
                });
                return;
            }
            let (command, args) = req;
            let res = client.request(command, args).await;
            if let Ok(Err(e)) = res {
                let _ = this.update(cx, |this, cx| {
                    this.debug.info(format!("Cannot {command}: {e}"));
                    this.end_session(cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn stop_debug(&mut self, cx: &mut Context<Self>) {
        self.debug.gen += 1;
        self.debug.pending = false;
        if let Some(s) = self.debug.session.as_ref().filter(|s| s.state != State::Ended) {
            let rx = s.client.request("disconnect", json!({"terminateDebuggee": !s.attached}));
            let client = s.client.clone();
            cx.spawn(async move |_, cx| {
                let _ = futures::future::select(rx, cx.background_executor().timer(std::time::Duration::from_secs(2))).await;
                client.kill();
            })
            .detach();
        }
        let detached = self.debug.session.as_ref().is_some_and(|s| s.attached && s.state != State::Ended);
        self.debug.info(if detached { "Detached: the process keeps running" } else { "Stopped" });
        self.end_session(cx);
    }

    pub(super) fn end_session(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.debug.session.as_mut() {
            s.state = State::Ended;
        }
        self.debug.frames.clear();
        self.debug.threads.clear();
        self.debug.threads_open = false;
        self.debug.idle_watches();
        self.debug.edit = None;
        self.debug.exception = None;
        self.clear_exec_marks(cx);
        self.stale_data_views("The debug session ended", cx);
        for e in self.editors().cloned().collect::<Vec<_>>() {
            e.update(cx, |e, cx| {
                e.clear_breakpoints_verified();
                cx.notify();
            });
        }
    }

    pub(super) fn debug_event(&mut self, ev: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Event::Dap { event, body } = ev else {
            if self.debug.state().is_some_and(|s| s != State::Ended) {
                let code = self.debug.session.as_ref().and_then(|s| s.exit_code);
                self.debug.info(match code {
                    Some(c) => format!("Process finished with exit code {c}"),
                    None => "Debugger disconnected".into(),
                });
                self.end_session(cx);
            }
            cx.notify();
            return;
        };
        match event.as_str() {
            "initialized" => {
                for (p, _) in self.all_breakpoints(cx) {
                    self.send_breakpoints(&p, cx);
                }
                self.debug_request("setExceptionBreakpoints", self.debug.exception_args(), cx);
                self.debug_request("configurationDone", json!({}), cx);
                if let Some(s) = self.debug.session.as_mut() {
                    s.state = State::Running;
                }
            }
            "output" => {
                let cat = body.get("category").and_then(Value::as_str).unwrap_or("console");
                let text = body.get("output").and_then(Value::as_str).unwrap_or("");
                match cat {
                    "stdout" => self.debug.print(text, Line::Out),
                    "stderr" => self.debug.print(text, Line::Err),
                    "telemetry" | "important" => {}
                    _ => self.debug.print(text, Line::Info),
                }
            }
            "stopped" => {
                let thread = body.get("threadId").and_then(Value::as_i64).unwrap_or(1);
                if let Some(s) = self.debug.session.as_mut() {
                    s.state = State::Paused(thread);
                    // Run to Cursor reached (or another breakpoint): the temporary one goes.
                    if let Some((p, _)) = s.temp.take() {
                        self.send_breakpoints(&p, cx);
                    }
                }
                if body.get("reason").and_then(Value::as_str) == Some("exception") {
                    let d = body.get("text").or_else(|| body.get("description")).and_then(Value::as_str).unwrap_or("exception");
                    self.debug.exception = Some(d.to_string());
                    self.exception_info(thread, cx);
                }
                self.debug.tab = DebugTab::Debugger;
                self.bottom = Some(BottomTool::Debug);
                self.load_frames(thread, window, cx);
                self.load_threads(cx);
            }
            "continued" => self.set_running(cx),
            "exited" => {
                if let Some(s) = self.debug.session.as_mut() {
                    s.exit_code = body.get("exitCode").and_then(Value::as_i64);
                }
            }
            "terminated" => {
                let code = self.debug.session.as_ref().and_then(|s| s.exit_code);
                if self.debug.state() != Some(State::Ended) {
                    self.debug.info(match code {
                        Some(c) => format!("\nProcess finished with exit code {c}"),
                        None => "\nProcess finished".into(),
                    });
                }
                self.end_session(cx);
                self.debug_request("disconnect", json!({}), cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// Stopped on an exception: its type, message and traceback (DAP exceptionInfo).
    pub(super) fn exception_info(&mut self, thread: i64, cx: &mut Context<Self>) {
        let Some(s) = &self.debug.session else { return };
        let rx = s.client.request("exceptionInfo", json!({"threadId": thread}));
        cx.spawn(async move |this, cx| {
            let res = rx.await;
            let _ = this.update(cx, |this, cx| {
                let (head, stack) = match res {
                    Ok(Ok(v)) => {
                        let id = v.get("exceptionId").and_then(Value::as_str).unwrap_or("");
                        let ty = v.pointer("/details/typeName").and_then(Value::as_str).unwrap_or(id);
                        let msg = v.pointer("/details/message").or_else(|| v.get("description")).and_then(Value::as_str).unwrap_or("");
                        let head = if msg.is_empty() { ty.to_string() } else { format!("{ty}: {msg}") };
                        (head, v.pointer("/details/stackTrace").and_then(Value::as_str).unwrap_or("").to_string())
                    }
                    _ => (this.debug.exception.clone().unwrap_or_else(|| "exception".into()), String::new()),
                };
                this.debug.print("Traceback (most recent call last):\n", Line::Err);
                let stack = python_order(&stack);
                if !stack.is_empty() {
                    this.debug.print(&format!("{stack}\n"), Line::Err);
                }
                this.debug.print(&format!("{head}\n"), Line::Err);
                this.debug.exception = Some(head);
                cx.notify();
            });
        })
        .detach();
    }

    /// A console link: `File "x.py", line N` (relative to the session's directory).
    pub(super) fn console_link(&self, path: &str, line: usize) -> Option<(PathBuf, usize)> {
        let p = PathBuf::from(path);
        let p = if p.is_absolute() { p } else { self.debug.cwd.join(p) };
        p.is_file().then_some((p, line))
    }

    pub(in crate::workspace) fn open_console_link(&mut self, path: &Path, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file(path, window, cx);
        if let Some(ed) = self.active_editor().cloned() {
            ed.update(cx, |e, cx| e.go_to_line(line.saturating_sub(1), cx));
        }
        self.focus_editor(window, cx);
        cx.notify();
    }

    // --- Smart Step Into ----------------------------------------------------------------

    /// Shift+F7: the calls on the stop line; one call steps into it right away.
    pub(super) fn smart_step_into(&mut self, cx: &mut Context<Self>) {
        let (Some(thread), Some(frame)) = (self.debug.paused_thread(), self.debug.frames.get(self.debug.frame).map(|f| f.id)) else {
            self.status = "The debugger is not paused".into();
            return;
        };
        let Some(s) = &self.debug.session else { return };
        let rx = s.client.request("stepInTargets", json!({"frameId": frame}));
        // The stop's statement, from its editor (or the file on disk).
        let stmt = self.debug.frames.get(self.debug.frame).and_then(|f| {
            let p = f.path.clone()?;
            let lines: Vec<String> = match self.editors().find(|e| e.read(cx).path.as_ref() == Some(&p)) {
                Some(ed) => {
                    let b = &ed.read(cx).buffer;
                    (f.line..(f.line + 30).min(b.len_lines())).map(|l| b.line(l)).collect()
                }
                None => std::fs::read_to_string(&p).ok()?.lines().skip(f.line).take(30).map(String::from).collect(),
            };
            Some(statement_text(&lines, 0))
        });
        cx.spawn(async move |this, cx| {
            let res = rx.await;
            let _ = this.update(cx, |this, cx| {
                if this.debug.paused_thread() != Some(thread) {
                    return;
                }
                let targets: Vec<(i64, String)> = match &res {
                    Ok(Ok(v)) => v
                        .get("targets")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter().filter_map(|t| Some((t.get("id")?.as_i64()?, t.get("label").and_then(Value::as_str).unwrap_or("?").to_string()))).collect()
                        })
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                let targets = match &stmt {
                    Some(s) => targets_on_statement(s, targets),
                    None => targets,
                };
                match targets.len() {
                    // Nothing to choose (or no support): a plain Step Into.
                    0 => {
                        this.debug_request("stepIn", json!({"threadId": thread}), cx);
                        this.set_running(cx);
                    }
                    1 => {
                        this.debug_request("stepIn", json!({"threadId": thread, "targetId": targets[0].0}), cx);
                        this.set_running(cx);
                    }
                    _ => this.debug.chooser = Some(Chooser { kind: ChooserKind::StepInto(thread), targets, sel: 0 }),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Step into the chosen call / attach to the chosen process.
    pub(in crate::workspace) fn chooser_go(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(st) = self.debug.chooser.take() else { return };
        if let Some((id, label)) = st.targets.get(i) {
            match st.kind {
                ChooserKind::StepInto(thread) => {
                    if self.debug.paused_thread() == Some(thread) {
                        self.debug_request("stepIn", json!({"threadId": thread, "targetId": id}), cx);
                        self.set_running(cx);
                    }
                }
                ChooserKind::Attach => self.attach_to_process(*id as u32, label.clone(), window, cx),
            }
        }
        cx.notify();
    }

    // --- Set Next Statement -----------------------------------------------------------------

    /// Jump to Cursor / a dragged execution arrow: the paused thread continues from `line`
    /// of this editor's file without running the lines between (debugpy gotoTargets + goto).
    pub(in crate::workspace) fn set_next_statement(&mut self, ed: Entity<Editor>, line: usize, cx: &mut Context<Self>) {
        let Some(thread) = self.debug.paused_thread() else {
            self.status = "Jump to Cursor: the debugger is not paused".into();
            return;
        };
        let Some(path) = ed.read(cx).path.clone() else { return };
        let Some(s) = &self.debug.session else { return };
        let client = s.client.clone();
        // debugpy moves the innermost frame, whatever file the target names: another file
        // would mean that line number in the paused one. Other limits (into a loop, out of the
        // function) debugpy reports itself, and the program stays paused where it was.
        if self.debug.frames.first().and_then(|f| f.path.as_ref()) != Some(&path) {
            let msg = format!("Cannot jump to line {}: only in the file where the program is paused", line + 1);
            self.debug.info(msg.clone());
            self.show_toast(msg, true, cx);
            return;
        }
        cx.spawn(async move |this, cx| {
            let targets = client.request("gotoTargets", json!({"source": {"path": path.to_string_lossy()}, "line": line + 1})).await;
            let target = match &targets {
                Ok(Ok(v)) => v.pointer("/targets/0/id").and_then(Value::as_i64),
                _ => None,
            };
            let res = match target {
                Some(id) => match client.request("goto", json!({"threadId": thread, "targetId": id})).await {
                    Ok(Ok(_)) => Ok(()),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err("no reply".into()),
                },
                None => Err(match targets {
                    Ok(Err(e)) => e,
                    _ => "no target on this line".into(),
                }),
            };
            let _ = this.update(cx, |this, cx| {
                match res {
                    // The thread stops again on the new line ("stopped", reason "goto").
                    Ok(()) => this.status = format!("Next statement: line {}", line + 1),
                    Err(e) => {
                        let msg = format!("Cannot jump to line {}: {e}", line + 1);
                        this.debug.info(msg.clone());
                        this.show_toast(msg, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // --- Attach to Process ----------------------------------------------------------------

    /// Ctrl+Alt+F5: running Python processes (not debugpy's own) to choose from.
    pub(super) fn list_processes(&mut self, cx: &mut Context<Self>) {
        self.status = "Looking for Python processes…".into();
        cx.spawn(async move |this, cx| {
            let ps = cx.background_executor().spawn(async move { process_lines() }).await;
            let _ = this.update(cx, |this, cx| {
                let procs = brisk_dap::python_processes(&ps, std::process::id());
                if procs.is_empty() {
                    this.status = "No Python processes to attach to".into();
                } else {
                    this.status.clear();
                    let targets = procs.into_iter().map(|(pid, cmd)| (pid as i64, format!("{pid}  {}    {cmd}", process_target(&cmd)))).collect();
                    this.debug.chooser = Some(Chooser { kind: ChooserKind::Attach, targets, sel: 0 });
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Ctrl+F2 while a debug session is live: does it stop the debugger rather than a run?
    /// The focused window decides (Debug or Run), else the one started last.
    pub(in crate::workspace) fn stop_targets_debug(&self, window: &Window, cx: &gpui::App) -> bool {
        let Some(panel) = self.runs.panel.as_ref().filter(|p| p.read(cx).is_running()) else { return true };
        if self.debug.focus.contains_focused(window, cx) {
            return true;
        }
        if gpui::Focusable::focus_handle(panel.read(cx), cx).contains_focused(window, cx) {
            return false;
        }
        match (self.debug.started, panel.read(cx).last_started_running()) {
            (Some(d), Some(r)) => d >= r,
            _ => true,
        }
    }

    /// No debugpy anywhere: offer to install it into `python`, then start `then` again.
    pub(super) fn offer_debugpy(
        &mut self,
        missing: brisk_dap::DebugpyMissing,
        python: PathBuf,
        then: crate::workspace::install_offer_ui::Then,
        cx: &mut Context<Self>,
    ) {
        self.debug.info(missing.to_string());
        self.status = missing.to_string();
        self.install_offer = Some(crate::workspace::install_offer_ui::InstallOffer { package: "debugpy", python, then });
        cx.notify();
    }

    /// Rerun after an Attach: the same process again if it still runs.
    pub(in crate::workspace) fn reattach(&mut self, pid: u32, label: String, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let ps = cx.background_executor().spawn(async move { process_lines() }).await;
            let alive = brisk_dap::python_processes(&ps, std::process::id()).iter().any(|p| p.0 == pid);
            let _ = this.update_in(cx, |this, window, cx| {
                if alive {
                    this.attach_to_process(pid, label, window, cx);
                } else {
                    let msg = format!("Process {pid} has exited: use Attach to Process (Ctrl+Alt+F5) to choose another");
                    this.debug.info(msg.clone());
                    this.show_toast(msg, false, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// debugpy's adapter (with the project interpreter) injects itself into `pid`.
    pub(in crate::workspace) fn attach_to_process(&mut self, pid: u32, label: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.debug.is_live() {
            self.stop_debug(cx);
        }
        let Some(python) = self.runs.interpreter(&self.root).map(|i| i.path) else {
            self.show_toast("No Python interpreter configured: create a .venv or install Python".into(), true, cx);
            return;
        };
        let name = format!("pid {pid}");
        self.debug.last = None;
        self.debug.last_attach = Some((pid, label.clone()));
        self.debug.exception = None;
        self.debug.console.clear();
        self.debug.console_changed(false);
        self.debug.frames.clear();
        self.debug.idle_watches();
        self.debug.tab = DebugTab::Console;
        self.debug.info(format!("Attaching to {label}"));
        self.bottom = Some(BottomTool::Debug);
        self.status = format!("Attaching to process {pid}");
        let root = self.root.clone();
        self.debug.gen += 1;
        self.debug.pending = true;
        self.debug.started = Some(std::time::Instant::now());
        let gen = self.debug.gen;
        cx.spawn_in(window, async move |this, cx| {
            let adapter = cx
                .background_executor()
                .spawn({
                    let python = python.clone();
                    async move { brisk_dap::adapter_command(&python) }
                })
                .await;
            let started = adapter.map(|a| Client::start(&a.program, &a.args, &root, &[]).map_err(|e| format!("cannot start debugpy: {e}")));
            let _ = this.update_in(cx, |this, window, cx| {
                if this.debug.gen != gen {
                    if let Ok(Ok((client, _))) = &started {
                        client.kill();
                    }
                    return;
                }
                this.debug.pending = false;
                match started {
                    Err(missing) => this.offer_debugpy(missing, python, crate::workspace::install_offer_ui::Then::Attach(pid, label), cx),
                    Ok(Err(e)) => {
                        this.debug.info(e.clone());
                        this.show_toast(e, true, cx);
                    }
                    Ok(Ok((client, events))) => this.attach_session(client, events, name, ("attach", brisk_dap::attach_args(pid, true)), window, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::workspace) fn chooser_key(&mut self, e: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(st) = self.debug.chooser.as_mut() else { return };
        let n = st.targets.len();
        match e.keystroke.key.as_str() {
            "escape" => self.debug.chooser = None,
            "up" => st.sel = (st.sel + n - 1) % n.max(1),
            "down" | "tab" => st.sel = (st.sel + 1) % n.max(1),
            "enter" | "f7" => {
                let i = st.sel;
                self.chooser_go(i, window, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    pub(in crate::workspace) fn render_chooser(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let st = self.debug.chooser.as_ref()?;
        let title = match st.kind {
            ChooserKind::StepInto(_) => "Smart Step Into  (Enter: step into · Esc: cancel)",
            ChooserKind::Attach => "Attach to Process  (Enter: attach · Esc: cancel)",
        };
        let rows = st.targets.iter().enumerate().map(|(i, (_, label))| {
            let selected = i == st.sel;
            div()
                .id(("smart-step", i))
                .h(ui_px(24.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .cursor_pointer()
                .font_family(mono_font())
                .when(selected, |x| x.bg(Theme::selection_row()))
                .when(!selected, |x| x.hover(|s| s.bg(Theme::hover_row())))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(label.clone())
                .on_click(cx.listener(move |this, _, w, cx| this.chooser_go(i, w, cx)))
        });
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(120.))
                .occlude()
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.debug.chooser = None;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .id("smart-step-popup")
                        .min_w(px(260.))
                        .max_w(px(900.))
                        .flex()
                        .flex_col()
                        .p(px(6.))
                        .gap(px(2.))
                        .bg(Theme::panel())
                        .border_1()
                        .border_color(Theme::divider())
                        .rounded(px(8.))
                        .map(crate::ui_kit::floating)
                        .text_size(ui_px(13.))
                        .text_color(Theme::text())
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(div().px(px(10.)).pb(px(4.)).text_size(ui_px(12.)).text_color(Theme::text_dim()).child(title))
                        .children(rows),
                )
                .into_any_element(),
        )
    }
}
