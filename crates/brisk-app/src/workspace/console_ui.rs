//! Python Console tool window: a REPL in the project's interpreter (`py_console.rs`), a
//! multi-line input (Enter runs a complete statement, Shift+Enter adds a line, ↑/↓ walk the
//! history) and the namespace's variables on the right, refreshed after every command.

use super::{BottomTool, Workspace};
use crate::editor::Editor;
use crate::py_console::{Console, Event, Var};
use crate::theme::{Theme, mono_font};
use brisk_keymap::Action;
use gpui::prelude::*;
use gpui::{div, px, Context, Entity, FocusHandle, Focusable, KeyDownEvent, ScrollHandle, Window};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Kind {
    Out,
    Err,
    Info,
    /// Echo of the code that ran (">>> " / "... " and the line), with Python highlights
    /// relative to the line's text.
    Code(Vec<(std::ops::Range<usize>, brisk_syntax::Highlight)>),
}

/// The ran code as echo lines with their Python highlights.
fn echo_lines(code: &str) -> Vec<(String, Kind)> {
    let mut tree = brisk_syntax::SyntaxTree::new(brisk_syntax::Lang::Python);
    tree.parse(code);
    let spans = tree.highlights(code, 0..code.len());
    let mut out = Vec::new();
    let mut start = 0;
    for (i, l) in code.split('\n').enumerate() {
        let end = start + l.len();
        const PREFIX: usize = 4;
        let hl = spans
            .iter()
            .filter(|s| s.range.start < end && s.range.end > start)
            .map(|s| (s.range.start.max(start) - start + PREFIX..s.range.end.min(end) - start + PREFIX, s.kind))
            .collect();
        out.push((format!("{} {l}\n", if i == 0 { ">>>" } else { "..." }), Kind::Code(hl)));
        start = end + 1;
    }
    out
}

pub struct PyConsole {
    pub input: Entity<Editor>,
    pub focus: FocusHandle,
    console: Option<Console>,
    /// Bumped on every start: the polling task of an older process stops.
    generation: u64,
    pub lines: Vec<(String, Kind)>,
    pub vars: Vec<Var>,
    pub version: String,
    python: Option<PathBuf>,
    /// A command is running (Enter waits).
    pub busy: bool,
    /// The code Enter sent to `check`; the answer runs it or adds a line.
    checking: Option<String>,
    /// The prompt of a pending `input()` call.
    pub waiting_input: Option<String>,
    pub history: Vec<String>,
    /// Commands sent so far: Stop's fallback kill checks it is still the same command.
    runs: u64,
    /// Position while walking the history, and the unsent text it replaced.
    hist_pos: Option<usize>,
    draft: String,
    scroll: ScrollHandle,
}

const MAX_LINES: usize = 5000;

impl PyConsole {
    fn new(cx: &mut Context<Workspace>) -> Self {
        let input = cx.new(|cx| {
            let mut e = Editor::scratch("", cx);
            e.set_language(Some(brisk_syntax::Lang::Python));
            e
        });
        Self {
            input,
            focus: cx.focus_handle(),
            console: None,
            generation: 0,
            lines: Vec::new(),
            vars: Vec::new(),
            version: String::new(),
            python: None,
            busy: false,
            checking: None,
            waiting_input: None,
            history: Vec::new(),
            runs: 0,
            hist_pos: None,
            draft: String::new(),
            scroll: ScrollHandle::new(),
        }
    }

    pub fn running(&self) -> bool {
        self.console.is_some()
    }

    fn print(&mut self, text: &str, kind: Kind) {
        // Output arrives in chunks: continue the last unfinished line of the same kind.
        let mut parts = text.split('\n');
        if let Some(first) = parts.next() {
            match self.lines.last_mut() {
                Some((last, k)) if *k == kind && !last.ends_with('\n') && matches!(kind, Kind::Out | Kind::Err) => last.push_str(first),
                _ => self.lines.push((first.to_string(), kind.clone())),
            }
        }
        for p in parts {
            if let Some((last, _)) = self.lines.last_mut() {
                last.push('\n');
            }
            self.lines.push((p.to_string(), kind.clone()));
        }
        // A trailing newline leaves an empty open line; drop it so the next chunk starts fresh.
        if text.ends_with('\n') {
            if let Some((l, _)) = self.lines.last() {
                if l.is_empty() {
                    self.lines.pop();
                }
            }
        }
        if self.lines.len() > MAX_LINES {
            self.lines.drain(..self.lines.len() - MAX_LINES);
        }
        self.scroll.scroll_to_bottom();
    }

    fn info(&mut self, text: impl Into<String>) {
        self.lines.push((format!("{}\n", text.into()), Kind::Info));
        self.scroll.scroll_to_bottom();
    }
}

impl Workspace {
    pub(super) fn toggle_python_console(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pyc.is_none() {
            self.pyc = Some(PyConsole::new(cx));
        }
        self.toggle_bottom(BottomTool::PythonConsole, window, cx);
        if self.bottom == Some(BottomTool::PythonConsole) {
            if !self.pyc.as_ref().is_some_and(PyConsole::running) {
                self.start_python_console(cx);
            }
            if let Some(p) = &self.pyc {
                window.focus(&p.input.read(cx).focus_handle(cx));
            }
        }
        cx.notify();
    }

    fn start_python_console(&mut self, cx: &mut Context<Self>) {
        let python = self.runs.interpreter(&self.root).map(|i| i.path).unwrap_or_else(|| PathBuf::from("python"));
        let root = self.root.clone();
        let Some(p) = self.pyc.as_mut() else { return };
        p.console = None;
        p.busy = true;
        p.checking = None;
        p.waiting_input = None;
        p.vars.clear();
        p.generation += 1;
        let gen = p.generation;
        match Console::start(&python, &root) {
            Ok(c) => {
                p.console = Some(c);
                p.python = Some(python);
            }
            Err(e) => {
                p.busy = false;
                p.info(format!("Cannot start {}: {e}", python.display()));
                return;
            }
        }
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(40)).await;
            let alive = this
                .update(cx, |this, cx| {
                    let Some(p) = this.pyc.as_mut().filter(|p| p.generation == gen) else { return false };
                    let Some(c) = p.console.as_mut() else { return false };
                    let evs = c.events();
                    if evs.is_empty() {
                        return true;
                    }
                    this.python_console_events(evs, cx);
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !alive {
                break;
            }
        })
        .detach();
    }

    fn python_console_events(&mut self, evs: Vec<Event>, cx: &mut Context<Self>) {
        for e in evs {
            let Some(p) = self.pyc.as_mut() else { return };
            match e {
                Event::Ready(v) => {
                    p.version = v.clone();
                    p.busy = false;
                    let exe = p.python.as_ref().map(|x| x.display().to_string()).unwrap_or_default();
                    p.info(format!("Python {v} ({exe})"));
                }
                Event::Out(s) => p.print(&s, Kind::Out),
                Event::Err(s) => p.print(&s, Kind::Err),
                Event::Check(complete) => {
                    let Some(code) = p.checking.take() else { continue };
                    if complete {
                        self.python_console_run(code, cx);
                        continue;
                    }
                    let input = p.input.clone();
                    input.update(cx, |e, cx| {
                        e.buffer.newline();
                        cx.notify();
                    });
                }
                Event::Input(prompt) => {
                    p.print(&prompt, Kind::Out);
                    p.waiting_input = Some(prompt);
                }
                Event::Done { vars, exit } => {
                    p.vars = vars;
                    p.busy = false;
                    p.waiting_input = None;
                    if exit {
                        p.info("The console process has finished. Rerun to start a new one.");
                    }
                }
                Event::Exited(code) => {
                    if p.busy || p.version.is_empty() {
                        p.info(format!("Process finished with exit code {}", code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())));
                    }
                    p.console = None;
                    p.busy = false;
                    p.waiting_input = None;
                }
            }
        }
    }

    fn python_console_run(&mut self, code: String, cx: &mut Context<Self>) {
        let Some(p) = self.pyc.as_mut() else { return };
        p.lines.extend(echo_lines(&code));
        p.runs += 1;
        p.scroll.scroll_to_bottom();
        if p.history.last() != Some(&code) {
            p.history.push(code.clone());
        }
        p.hist_pos = None;
        p.draft.clear();
        let input = p.input.clone();
        input.update(cx, |e, cx| {
            e.buffer.set_text("");
            cx.notify();
        });
        let Some(c) = p.console.as_mut() else {
            p.info("The console is not running. Rerun to start it.");
            return;
        };
        p.busy = true;
        if let Err(e) = c.run(&code) {
            p.busy = false;
            p.info(format!("Cannot send the command: {e}"));
        }
    }

    /// Enter in the input: answer `input()`, run a complete statement, or add a line.
    fn python_console_enter(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.pyc.as_mut() else { return };
        let text = p.input.read(cx).text_string();
        if p.waiting_input.is_some() {
            if let Some(c) = p.console.as_mut() {
                let _ = c.input(&text);
            }
            p.waiting_input = None;
            // The answer continues the prompt's line.
            p.print(&format!("{text}\n"), Kind::Out);
            let input = p.input.clone();
            input.update(cx, |e, cx| {
                e.buffer.set_text("");
                cx.notify();
            });
            return;
        }
        if p.busy || p.checking.is_some() {
            return;
        }
        if text.trim().is_empty() {
            p.lines.push((">>> \n".into(), Kind::Code(Vec::new())));
            p.scroll.scroll_to_bottom();
            return;
        }
        // A block ends with an empty line, as in the Python REPL.
        let ends_block = text.contains('\n') && text.rsplit('\n').next().is_some_and(|l| l.trim().is_empty());
        if ends_block {
            let code = text.trim_end().to_string();
            return self.python_console_run(code, cx);
        }
        match p.console.as_mut() {
            Some(c) => {
                if c.check(&text).is_ok() {
                    p.checking = Some(text);
                }
            }
            None => self.python_console_run(text, cx),
        }
    }

    fn python_console_history(&mut self, back: bool, cx: &mut Context<Self>) -> bool {
        let Some(p) = self.pyc.as_mut() else { return false };
        if p.history.is_empty() {
            return false;
        }
        let next = match (p.hist_pos, back) {
            (None, true) => {
                p.draft = p.input.read(cx).text_string();
                Some(p.history.len() - 1)
            }
            (None, false) => return false,
            (Some(0), true) => Some(0),
            (Some(i), true) => Some(i - 1),
            (Some(i), false) if i + 1 < p.history.len() => Some(i + 1),
            (Some(_), false) => None,
        };
        p.hist_pos = next;
        let text = match next {
            Some(i) => p.history[i].clone(),
            None => std::mem::take(&mut p.draft),
        };
        let input = p.input.clone();
        input.update(cx, |e, cx| {
            e.buffer.set_text(&text);
            let end = e.buffer.rope().len_chars();
            e.buffer.set_caret(end, false);
            cx.notify();
        });
        true
    }

    /// Keys typed in the console input; true when handled.
    pub(super) fn python_console_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(p) = &self.pyc else { return false };
        if !p.input.read(cx).focus_handle(cx).is_focused(window) {
            return false;
        }
        let k = &e.keystroke;
        let m = &k.modifiers;
        let plain = !(m.control || m.alt || m.shift || m.platform);
        let ed = p.input.read(cx);
        let (line, _) = ed.caret_line_col();
        let last_line = ed.buffer.len_lines().saturating_sub(1);
        let completion = ed.completion_open();
        match k.key.as_str() {
            "enter" if plain && !completion => self.python_console_enter(cx),
            "enter" if m.shift && !m.control && !m.alt => {
                let input = p.input.clone();
                input.update(cx, |e, cx| {
                    e.buffer.newline();
                    cx.notify();
                });
            }
            "up" if plain && !completion && line == 0 => return self.python_console_history(true, cx),
            "down" if plain && !completion && line == last_line => return self.python_console_history(false, cx),
            "escape" if plain && !completion => self.focus_editor(window, cx),
            "c" if m.control && !m.alt && !m.shift && p.busy => self.python_console_interrupt(cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Ctrl+C / Stop while a command runs: KeyboardInterrupt in it, the session and its
    /// variables stay. If the command is still running 1.5 s later, the process is killed.
    fn python_console_interrupt(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.pyc.as_mut() else { return };
        let Some(c) = p.console.as_mut() else { return };
        if c.interrupt().is_err() {
            return;
        }
        let (gen, runs) = (p.generation, p.runs);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            let _ = this.update(cx, |this, cx| {
                let Some(p) = this.pyc.as_mut().filter(|p| p.generation == gen && p.runs == runs && p.busy) else { return };
                if let Some(mut c) = p.console.take() {
                    c.kill();
                }
                p.busy = false;
                p.waiting_input = None;
                p.info("The command did not stop on KeyboardInterrupt; the console process was killed. Rerun to start a new one.");
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn python_console_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            Action::RerunPythonConsole => {
                if let Some(p) = self.pyc.as_mut() {
                    if let Some(mut c) = p.console.take() {
                        c.kill();
                    }
                    p.info("Restarting the console…");
                }
                self.start_python_console(cx);
            }
            Action::StopPythonConsole if self.pyc.as_ref().is_some_and(|p| p.busy && p.running()) => self.python_console_interrupt(cx),
            Action::StopPythonConsole => {
                if let Some(p) = self.pyc.as_mut() {
                    if let Some(mut c) = p.console.take() {
                        c.kill();
                        p.busy = false;
                        p.waiting_input = None;
                        p.checking = None;
                        p.info("The console process was stopped.");
                    }
                }
            }
            Action::ClearPythonConsole => {
                if let Some(p) = self.pyc.as_mut() {
                    p.lines.clear();
                }
            }
            _ => {}
        }
        if let Some(p) = &self.pyc {
            window.focus(&p.input.read(cx).focus_handle(cx));
        }
        cx.notify();
    }

    pub(super) fn render_python_console(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(p) = &self.pyc else { return div().into_any_element() };
        let running = p.running();
        let tool = |id: &'static str, glyph: &'static str, tip: &'static str, on: bool, color: gpui::Hsla, action: Action, cx: &mut Context<Self>| {
            div()
                .id(id)
                .w(px(26.))
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .text_color(if on { color } else { Theme::text_dim() })
                .when(on, |x| x.cursor_pointer().hover(|s| s.bg(Theme::hover_row())).active(|s| s.bg(Theme::press())))
                .child(glyph)
                .tooltip(move |_, cx| cx.new(|_| super::Tip(tip.into())).into())
                .on_click(cx.listener(move |this, _, w, cx| {
                    if on {
                        this.dispatch(action, w, cx);
                    }
                }))
        };
        let green: gpui::Hsla = gpui::rgb(0x5fb865).into();
        let state = if !running {
            "not running".to_string()
        } else if p.waiting_input.is_some() {
            "waiting for input()".into()
        } else if p.busy && p.version.is_empty() {
            "starting…".into()
        } else if p.busy {
            "running…".into()
        } else {
            format!("Python {}", p.version)
        };
        let toolbar = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(6.))
            .h(px(30.))
            .flex_shrink_0()
            .border_b_1()
            .border_color(Theme::divider())
            .child(tool("pyc-rerun", "↻", "Rerun (restart the console)", true, green, Action::RerunPythonConsole, cx))
            .child(tool("pyc-stop", "■", "Stop the console", running, Theme::error(), Action::StopPythonConsole, cx))
            .child(tool("pyc-clear", "⌫", "Clear All", !p.lines.is_empty(), Theme::text_muted(), Action::ClearPythonConsole, cx))
            .child(div().pl(px(10.)).text_size(px(12.)).text_color(Theme::text_muted()).child(state));
        let lines = div()
            .id("pyc-output")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&p.scroll)
            .px(px(8.))
            .py(px(4.))
            .font_family(mono_font())
            .text_size(px(12.5))
            .children(p.lines.iter().map(|(t, k)| {
                let text = t.trim_end_matches('\n').to_string();
                let color = match k {
                    Kind::Out => Theme::text(),
                    Kind::Err => Theme::error(),
                    Kind::Info => Theme::text_muted(),
                    Kind::Code(hl) => {
                        let mut styles: Vec<(std::ops::Range<usize>, gpui::HighlightStyle)> = vec![(0..3.min(text.len()), gpui::HighlightStyle { color: Some(Theme::accent()), ..Default::default() })];
                        styles.extend(hl.iter().filter(|(r, _)| r.end <= text.len()).map(|(r, h)| (r.clone(), gpui::HighlightStyle { color: Some(Theme::syntax(*h)), ..Default::default() })));
                        return div().text_color(Theme::text()).child(gpui::StyledText::new(text).with_highlights(styles)).into_any_element();
                    }
                };
                div().text_color(color).child(text).into_any_element()
            }));
        let focused = p.input.read(cx).focus_handle(cx).is_focused(window);
        let n_lines = p.input.read(cx).buffer.len_lines().clamp(1, 6);
        let empty = p.input.read(cx).buffer.len_chars() == 0;
        let prompt = if p.waiting_input.is_some() { "?" } else { ">>>" };
        let hint = if p.waiting_input.is_some() {
            "Type a line for input() and press Enter"
        } else {
            "Enter runs a complete statement, Shift+Enter adds a line, ↑/↓ — history"
        };
        let input = div()
            .flex()
            .flex_shrink_0()
            .gap(px(6.))
            .px(px(8.))
            .py(px(3.))
            .border_t_1()
            .border_color(Theme::divider())
            .font_family(mono_font())
            .text_size(px(12.5))
            .child(div().pt(px(1.)).text_color(Theme::accent()).child(prompt))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h(px(19. * n_lines as f32))
                    .overflow_hidden()
                    .child(p.input.clone())
                    .when(empty && !focused, |d| {
                        d.child(div().absolute().top_0().left_0().text_color(Theme::text_dim()).child(hint))
                    }),
            );
        let vars = div()
            .id("pyc-vars")
            .w(px(320.))
            .h_full()
            .flex_shrink_0()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(Theme::divider())
            .font_family(mono_font())
            .text_size(px(12.5))
            .when(p.vars.is_empty(), |x| x.child(div().p(px(8.)).text_color(Theme::text_dim()).font_family(crate::theme::ui_font()).child("Variables appear after a command defines them")))
            .children(p.vars.iter().enumerate().map(|(i, v)| {
                let ty = match v.size {
                    Some(n) => format!("{{{}: {n}}}", v.ty),
                    None => format!("{{{}}}", v.ty),
                };
                div()
                    .id(("pyc-var", i))
                    .w_full()
                    .h(px(21.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .hover(|s| s.bg(Theme::hover_row()))
                    .child(div().text_color(Theme::syntax(brisk_syntax::Highlight::Property)).child(v.name.clone()))
                    .child(div().text_color(Theme::text_dim()).child("="))
                    .child(div().text_color(Theme::text_dim()).child(ty))
                    .child(div().text_color(Theme::text()).child(v.value.clone()))
            }));
        let left = div().flex_1().min_w_0().h_full().flex().flex_col().child(lines).child(input);
        div()
            .id("pyc-window")
            .track_focus(&p.focus)
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(13.))
            .child(toolbar)
            .child(div().flex_1().min_h_0().flex().child(left).child(vars))
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, w, cx| {
                if let Some(p) = &this.pyc {
                    w.focus(&p.input.read(cx).focus_handle(cx));
                }
                cx.notify();
            }))
            .into_any_element()
    }

    // --- Smoke --------------------------------------------------------------------------

    /// BRISK_SMOKE_PYCONSOLE=1: opens the console, types statements (a block over several
    /// lines, an error, `input()`), walks the history and restarts.
    pub(super) fn python_console_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(Action::TogglePythonConsole, window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(Duration::from_millis(ms));
            let idle = |this: &gpui::WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext| {
                this.update(cx, |t, _| t.pyc.as_ref().is_some_and(|p| (!p.busy || p.waiting_input.is_some()) && p.checking.is_none())).unwrap_or(false)
            };
            macro_rules! until_idle {
                () => {{
                    for _ in 0..200 {
                        pause(50).await;
                        if idle(&this, cx) {
                            break;
                        }
                    }
                }};
            }
            until_idle!();
            let steps: &[&[&str]] = &[
                &["x = 41", "enter"],
                &["x + 1", "enter"],
                &["def square(n):", "enter", "return n * n", "enter", "enter"],
                &["nums = [square(i) for i in range(5)]", "enter"],
                &["print('sum', sum(nums))", "enter"],
                &["for i in range(2):", "shift-enter", "print('line', i)", "enter", "enter"],
                &["1 / 0", "enter"],
                &["name = input('who? ')", "enter"],
                &["Bob", "enter"],
                &["greeting = f'hi {name}'", "enter"],
            ];
            for step in steps {
                for s in step.iter() {
                    let _ = this.update_in(cx, |this, window, cx| {
                        if matches!(*s, "enter" | "shift-enter") {
                            this.smoke_press(s, window, cx);
                        } else {
                            this.smoke_type(s, window, cx);
                        }
                    });
                    pause(60).await;
                    until_idle!();
                }
            }
            // An endless loop, then Ctrl+C: KeyboardInterrupt, the session and variables stay.
            for s in ["import time", "enter", "while True: time.sleep(0.05)", "enter"] {
                let _ = this.update_in(cx, |this, window, cx| {
                    if s == "enter" {
                        this.smoke_press(s, window, cx)
                    } else {
                        this.smoke_type(s, window, cx)
                    }
                });
                pause(60).await;
                until_idle!();
            }
            let _ = this.update_in(cx, |this, window, cx| this.smoke_press("enter", window, cx));
            pause(800).await;
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-pyconsole: loop running busy {}", this.pyc.as_ref().unwrap().busy);
                this.smoke_press("ctrl-c", window, cx);
            });
            until_idle!();
            let _ = this.update_in(cx, |this, window, cx| {
                let p = this.pyc.as_ref().unwrap();
                eprintln!("smoke-pyconsole: after ctrl-c busy {} running {} vars {}", p.busy, p.running(), p.vars.len());
                eprintln!("smoke-pyconsole: version {:?}", p.version);
                for (l, k) in &p.lines {
                    let k = if let Kind::Code(hl) = k { format!("Code[{} spans]", hl.len()) } else { format!("{k:?}") };
                    eprintln!("smoke-pyconsole: {k} {}", l.trim_end());
                }
                eprintln!("smoke-pyconsole: vars {:?}", p.vars.iter().map(|v| format!("{}:{}={}", v.name, v.ty, v.value)).collect::<Vec<_>>());
                this.smoke_press("up up", window, cx);
                let t = this.pyc.as_ref().unwrap().input.read(cx).text_string();
                eprintln!("smoke-pyconsole: up up -> {t:?}");
                this.smoke_press("down down", window, cx);
                let t = this.pyc.as_ref().unwrap().input.read(cx).text_string();
                eprintln!("smoke-pyconsole: down down -> {t:?}");
            });
            if std::env::var("BRISK_SMOKE_PYCONSOLE").as_deref() == Ok("hold") {
                let _ = this.update_in(cx, |this, window, cx| this.smoke_type("[n for n in nums if n > 2]", window, cx));
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| this.dispatch(Action::RerunPythonConsole, window, cx));
            until_idle!();
            pause(300).await;
            let _ = this.update_in(cx, |this, window, cx| {
                let p = this.pyc.as_ref().unwrap();
                eprintln!("smoke-pyconsole: after restart running {} vars {:?} last {:?}", p.running(), p.vars.len(), p.lines.last().map(|l| l.0.trim_end().to_string()));
                this.dispatch(Action::StopPythonConsole, window, cx);
                eprintln!("smoke-pyconsole: after stop running {}", this.pyc.as_ref().unwrap().running());
                cx.quit();
            });
        })
        .detach();
    }
}
