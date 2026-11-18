//! Run/Debug Configurations dialog (Run › Edit Configurations…, the toolbar configuration
//! chip, Alt+Shift+F10 › Edit): configurations on the left, the selected one's form on the
//! right. OK writes `.brisk/run.json`.

use super::Workspace;
use crate::theme::Theme;
use brisk_run::{RunConfig, RunKind};
use gpui::prelude::*;
use gpui::{div, px, Context, KeyDownEvent, Window};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    Kind,
    Target,
    Keywords,
    Parameters,
    InterpreterOptions,
    WorkingDirectory,
    Env,
    Interpreter,
    Temporary,
}

impl Field {
    fn is_text(self) -> bool {
        !matches!(self, Field::Kind | Field::Temporary)
    }
}

/// The three kinds the dialog offers: PyCharm's Python (script / module) and pytest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Script,
    Module,
    Pytest,
}

impl Kind {
    const ALL: [Kind; 3] = [Kind::Script, Kind::Module, Kind::Pytest];
    fn of(c: &RunConfig) -> Kind {
        match (c.kind, c.module) {
            (RunKind::Pytest, _) => Kind::Pytest,
            (_, true) => Kind::Module,
            _ => Kind::Script,
        }
    }
    fn title(self) -> &'static str {
        match self {
            Kind::Script => "Script path",
            Kind::Module => "Module name",
            Kind::Pytest => "pytest",
        }
    }
    fn apply(self, c: &mut RunConfig) {
        c.kind = if self == Kind::Pytest { RunKind::Pytest } else { RunKind::Python };
        c.module = self == Kind::Module;
    }
}

/// One configuration being edited; environment variables as PyCharm shows them in one
/// line: `KEY=VALUE;KEY2=VALUE2`.
#[derive(Clone, Debug)]
struct Draft {
    cfg: RunConfig,
    env: String,
    /// The toolbar's selected configuration (the selection follows renames).
    selected: bool,
}

impl Draft {
    fn new(cfg: RunConfig) -> Self {
        let env = env_to_text(&cfg.env);
        Draft { cfg, env, selected: false }
    }
    fn commit(&self) -> RunConfig {
        let mut c = self.cfg.clone();
        c.env = env_from_text(&self.env);
        c.name = c.name.trim().to_string();
        c.interpreter = c.interpreter.filter(|i| !i.trim().is_empty());
        c
    }
}

pub struct RunDialog {
    drafts: Vec<Draft>,
    /// Index of the configuration shown in the form.
    current: usize,
    field: Field,
    /// Caret in the active text field, in chars.
    caret: usize,
    select_all: bool,
    error: Option<String>,
}

pub fn env_to_text(env: &BTreeMap<String, String>) -> String {
    env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(";")
}

/// `A=1;B=x y` (`;` or newlines separate, `\;` is a literal semicolon).
pub fn env_from_text(s: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut cur = String::new();
    let mut parts = Vec::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\\' if it.peek() == Some(&';') => {
                cur.push(';');
                it.next();
            }
            ';' | '\n' => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    for p in parts {
        if let Some((k, v)) = p.split_once('=') {
            let k = k.trim();
            if !k.is_empty() {
                out.insert(k.to_string(), v.to_string());
            }
        }
    }
    out
}

/// Fields of the form for a kind of configuration, in Tab order.
fn fields(kind: Kind, temporary: bool) -> Vec<Field> {
    use Field as F;
    let mut v = vec![F::Name, F::Kind, F::Target];
    if kind == Kind::Pytest {
        v.push(F::Keywords);
    }
    v.extend([F::Parameters, F::InterpreterOptions, F::WorkingDirectory, F::Env, F::Interpreter]);
    if temporary {
        v.push(F::Temporary);
    }
    v
}

fn label(f: Field, kind: Kind) -> &'static str {
    match f {
        Field::Name => "Name:",
        Field::Kind => "Run:",
        Field::Target => match kind {
            Kind::Script => "Script:",
            Kind::Module => "Module:",
            Kind::Pytest => "Target:",
        },
        Field::Keywords => "Keywords (-k):",
        Field::Parameters => "Parameters:",
        Field::InterpreterOptions => "Interpreter options:",
        Field::WorkingDirectory => "Working directory:",
        Field::Env => "Environment:",
        Field::Interpreter => "Interpreter:",
        Field::Temporary => "Temporary:",
    }
}

/// A name not used by `taken`: "base", "base (2)", "base (3)"…
fn unique_name(base: &str, taken: &[&str]) -> String {
    let mut name = base.to_string();
    let mut i = 1;
    while taken.contains(&name.as_str()) {
        i += 1;
        name = format!("{base} ({i})");
    }
    name
}

impl RunDialog {
    fn draft(&mut self) -> Option<&mut Draft> {
        self.drafts.get_mut(self.current)
    }

    fn kind(&self) -> Kind {
        self.drafts.get(self.current).map(|d| Kind::of(&d.cfg)).unwrap_or(Kind::Script)
    }

    fn fields(&self) -> Vec<Field> {
        match self.drafts.get(self.current) {
            Some(d) => fields(Kind::of(&d.cfg), d.cfg.temporary),
            None => Vec::new(),
        }
    }

    fn text(&mut self, f: Field) -> Option<&mut String> {
        let d = self.drafts.get_mut(self.current)?;
        Some(match f {
            Field::Name => &mut d.cfg.name,
            Field::Target => &mut d.cfg.target,
            Field::Keywords => &mut d.cfg.keywords,
            Field::Parameters => &mut d.cfg.parameters,
            Field::InterpreterOptions => &mut d.cfg.interpreter_options,
            Field::WorkingDirectory => &mut d.cfg.working_directory,
            Field::Env => &mut d.env,
            Field::Interpreter => d.cfg.interpreter.get_or_insert_with(String::new),
            Field::Kind | Field::Temporary => return None,
        })
    }

    fn text_len(&self, f: Field) -> usize {
        let Some(d) = self.drafts.get(self.current) else { return 0 };
        let s = match f {
            Field::Name => &d.cfg.name,
            Field::Target => &d.cfg.target,
            Field::Keywords => &d.cfg.keywords,
            Field::Parameters => &d.cfg.parameters,
            Field::InterpreterOptions => &d.cfg.interpreter_options,
            Field::WorkingDirectory => &d.cfg.working_directory,
            Field::Env => &d.env,
            Field::Interpreter => return d.cfg.interpreter.as_deref().map_or(0, |s| s.chars().count()),
            Field::Kind | Field::Temporary => return 0,
        };
        s.chars().count()
    }

    /// Focus a field with the caret at its end (`select` selects the whole text).
    fn focus(&mut self, f: Field, select: bool) {
        self.field = f;
        self.caret = self.text_len(f);
        self.select_all = select && f.is_text() && self.caret > 0;
    }

    fn select_config(&mut self, i: usize) {
        if i < self.drafts.len() {
            self.current = i;
            self.focus(Field::Name, false);
            self.error = None;
        }
    }

    /// Type or paste `s` at the caret (replacing everything when it is all selected).
    fn insert(&mut self, s: &str) {
        let f = self.field;
        let (all, caret) = (self.select_all, self.caret);
        self.select_all = false;
        let Some(t) = self.text(f) else { return };
        if all {
            t.clear();
        }
        let at = if all { 0 } else { caret.min(t.chars().count()) };
        let byte = t.char_indices().nth(at).map_or(t.len(), |(b, _)| b);
        t.insert_str(byte, s);
        self.caret = at + s.chars().count();
        self.error = None;
    }

    /// Backspace (`forward`: Delete); `word` deletes to the start/end of the field.
    fn delete(&mut self, forward: bool, word: bool) {
        let f = self.field;
        let (all, caret) = (self.select_all, self.caret);
        self.select_all = false;
        let Some(t) = self.text(f) else { return };
        let n = t.chars().count();
        let caret = caret.min(n);
        let (a, b) = if all {
            (0, n)
        } else if forward {
            (caret, if word { n } else { (caret + 1).min(n) })
        } else {
            (if word { 0 } else { caret.saturating_sub(1) }, caret)
        };
        let ba = t.char_indices().nth(a).map_or(t.len(), |(i, _)| i);
        let bb = t.char_indices().nth(b).map_or(t.len(), |(i, _)| i);
        t.replace_range(ba..bb, "");
        self.caret = a;
        self.error = None;
    }

    fn toggle(&mut self, back: bool) {
        let f = self.field;
        let Some(d) = self.draft() else { return };
        match f {
            Field::Kind => {
                let k = Kind::of(&d.cfg);
                let i = Kind::ALL.iter().position(|x| *x == k).unwrap_or(0);
                let n = Kind::ALL.len();
                Kind::ALL[if back { (i + n - 1) % n } else { (i + 1) % n }].apply(&mut d.cfg);
            }
            Field::Temporary => d.cfg.temporary = !d.cfg.temporary,
            _ => {}
        }
    }

    fn add(&mut self, kind: Kind) {
        let taken: Vec<&str> = self.drafts.iter().map(|d| d.cfg.name.as_str()).collect();
        let base = match kind {
            Kind::Pytest => "pytest",
            _ => "Unnamed",
        };
        let mut cfg = RunConfig { name: unique_name(base, &taken), ..Default::default() };
        kind.apply(&mut cfg);
        self.drafts.push(Draft::new(cfg));
        self.current = self.drafts.len() - 1;
        self.focus(Field::Name, true);
    }

    fn copy(&mut self) {
        let Some(d) = self.drafts.get(self.current).cloned() else { return };
        let taken: Vec<&str> = self.drafts.iter().map(|d| d.cfg.name.as_str()).collect();
        let mut d = d;
        d.cfg.name = unique_name(&format!("{} copy", d.cfg.name.trim()), &taken);
        d.cfg.temporary = false;
        d.selected = false;
        self.drafts.insert(self.current + 1, d);
        self.current += 1;
        self.focus(Field::Name, true);
    }

    fn remove(&mut self) {
        if self.current >= self.drafts.len() {
            return;
        }
        self.drafts.remove(self.current);
        self.current = self.current.min(self.drafts.len().saturating_sub(1));
        self.focus(Field::Name, false);
    }

    fn move_by(&mut self, up: bool) {
        let i = self.current;
        let j = if up { i.checked_sub(1) } else { (i + 1 < self.drafts.len()).then_some(i + 1) };
        if let Some(j) = j {
            self.drafts.swap(i, j);
            self.current = j;
        }
    }

    /// The configurations to save, or why they cannot be saved (and which one to show).
    fn validate(&self) -> Result<Vec<RunConfig>, (usize, Field, String)> {
        let cfgs: Vec<RunConfig> = self.drafts.iter().map(Draft::commit).collect();
        for (i, c) in cfgs.iter().enumerate() {
            if c.name.is_empty() {
                return Err((i, Field::Name, "Name is empty".into()));
            }
            if cfgs[..i].iter().any(|o| o.name == c.name) {
                return Err((i, Field::Name, format!("Two configurations are named '{}'", c.name)));
            }
            if c.kind == RunKind::Python && c.target.trim().is_empty() {
                let what = if c.module { "Module name" } else { "Script path" };
                return Err((i, Field::Target, format!("'{}': {what} is empty", c.name)));
            }
        }
        Ok(cfgs)
    }
}

impl Workspace {
    /// Run › Edit Configurations…: open the dialog on the selected configuration.
    pub(super) fn open_run_dialog(&mut self, cx: &mut Context<Self>) {
        self.reload_runs();
        let f = &self.runs.file;
        let mut drafts: Vec<Draft> = f.configurations.iter().cloned().map(Draft::new).collect();
        let current = f.selected.as_deref().and_then(|s| f.configurations.iter().position(|c| c.name == s));
        if let Some(i) = current {
            drafts[i].selected = true;
        }
        let mut d = RunDialog { drafts, current: current.unwrap_or(0), field: Field::Name, caret: 0, select_all: false, error: None };
        d.focus(Field::Name, false);
        self.runs.dialog = Some(d);
        cx.notify();
    }

    /// OK (`close`) / Apply: validate, write `.brisk/run.json`; OK also closes.
    fn run_dialog_save(&mut self, close: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(d) = self.runs.dialog.as_mut() else { return false };
        let cfgs = match d.validate() {
            Ok(c) => c,
            Err((i, f, msg)) => {
                d.current = i;
                d.focus(f, false);
                d.error = Some(msg);
                cx.notify();
                return false;
            }
        };
        // The toolbar keeps the selected configuration, renamed or not; removed: Current File.
        let selected = d.drafts.iter().zip(&cfgs).find(|(dr, _)| dr.selected).map(|(_, c)| c.name.clone());
        self.runs.file.configurations = cfgs;
        self.runs.file.selected = selected;
        d.error = None;
        self.save_runs();
        self.status = format!("Saved {} run configuration(s) to {}", self.runs.file.configurations.len(), brisk_run::CONFIG_FILE);
        if close {
            self.runs.dialog = None;
            self.focus_editor(window, cx);
        }
        cx.notify();
        true
    }

    fn run_dialog_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.runs.dialog = None;
        self.focus_editor(window, cx);
        cx.notify();
    }

    pub(super) fn run_dialog_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let m = k.modifiers;
        let Some(d) = self.runs.dialog.as_mut() else { return };
        let order = d.fields();
        let pos = order.iter().position(|f| *f == d.field).unwrap_or(0);
        let text = d.field.is_text();
        match k.key.as_str() {
            "escape" => return self.run_dialog_cancel(window, cx),
            "enter" => {
                self.run_dialog_save(true, window, cx);
                return;
            }
            // Configuration list: Alt+↑/↓ select, Alt+Shift+↑/↓ move, Alt+Insert add,
            // Ctrl+D copy, Alt+Delete remove.
            "up" | "down" if m.alt && m.shift => d.move_by(k.key == "up"),
            "up" | "down" if m.alt => {
                let i = if k.key == "up" { d.current.saturating_sub(1) } else { (d.current + 1).min(d.drafts.len().saturating_sub(1)) };
                d.select_config(i);
            }
            "insert" if m.alt => d.add(Kind::Script),
            "d" if m.control => d.copy(),
            "delete" if m.alt => d.remove(),
            "tab" | "down" | "up" if !order.is_empty() => {
                let back = (k.key == "tab" && m.shift) || k.key == "up";
                let n = order.len();
                let f = order[if back { (pos + n - 1) % n } else { (pos + 1) % n }];
                d.focus(f, true);
            }
            "left" | "right" | "space" if !text => d.toggle(k.key == "left"),
            "left" if text => {
                d.caret = if m.control || d.select_all { 0 } else { d.caret.saturating_sub(1) };
                d.select_all = false;
            }
            "right" if text => {
                let n = d.text_len(d.field);
                d.caret = if m.control || d.select_all { n } else { (d.caret + 1).min(n) };
                d.select_all = false;
            }
            "home" if text => {
                d.caret = 0;
                d.select_all = false;
            }
            "end" if text => {
                d.caret = d.text_len(d.field);
                d.select_all = false;
            }
            "backspace" if text => d.delete(false, m.control),
            "delete" if text => d.delete(true, m.control),
            "a" if m.control && text => d.select_all = d.text_len(d.field) > 0,
            "v" if m.control && text => {
                if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    // Environment: pasted lines become `;`-separated entries.
                    let t = if d.field == Field::Env { t.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join(";") } else { t.lines().next().unwrap_or("").to_string() };
                    d.insert(&t);
                }
            }
            "c" if m.control && text => {
                let f = d.field;
                if let Some(t) = d.text(f).cloned() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(t));
                }
            }
            _ => {
                if text && !(m.control || m.alt || m.platform) {
                    if let Some(ch) = &k.key_char {
                        if !ch.chars().any(|c| c.is_control()) {
                            d.insert(ch);
                        }
                    }
                }
            }
        }
        cx.notify();
    }

    pub(super) fn render_run_dialog(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let d = self.runs.dialog.as_ref()?;
        let kind = d.kind();
        let project_interp = self.interpreter_label();
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .px(px(14.))
                .h(px(28.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .cursor_pointer()
                .when(primary, |b| b.bg(Theme::accent()).text_color(gpui::white()))
                .when(!primary, |b| b.border_1().border_color(Theme::divider()).hover(|s| s.bg(Theme::hover_row())))
                .active(|s| s.bg(Theme::press()))
                .child(label)
        };
        let tool = |id: &'static str, glyph: &'static str, tip: &'static str, cx: &mut Context<Self>, act: fn(&mut RunDialog)| {
            div()
                .id(id)
                .px(px(6.))
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .cursor_pointer()
                .text_color(Theme::text_muted())
                .hover(|s| s.bg(Theme::hover_row()).text_color(Theme::text()))
                .active(|s| s.bg(Theme::press()))
                .tooltip(move |_, cx| cx.new(|_| super::Tip(tip.into())).into())
                .child(glyph)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(d) = this.runs.dialog.as_mut() {
                        act(d);
                        d.error = None;
                    }
                    cx.notify();
                }))
        };

        // Left: the list and its toolbar.
        let list_tools = div()
            .flex()
            .gap(px(2.))
            .pb(px(4.))
            .child(tool("rc-add-py", "+ Py", "Add Python script configuration (Alt+Insert)", cx, |d| d.add(Kind::Script)))
            .child(tool("rc-add-mod", "+ -m", "Add Python module configuration", cx, |d| d.add(Kind::Module)))
            .child(tool("rc-add-pt", "+ pytest", "Add pytest configuration", cx, |d| d.add(Kind::Pytest)))
            .child(tool("rc-copy", "⧉", "Copy configuration (Ctrl+D)", cx, |d| d.copy()))
            .child(tool("rc-remove", "−", "Remove configuration (Alt+Delete)", cx, |d| d.remove()))
            .child(tool("rc-up", "↑", "Move up (Alt+Shift+↑)", cx, |d| d.move_by(true)))
            .child(tool("rc-down", "↓", "Move down (Alt+Shift+↓)", cx, |d| d.move_by(false)));
        let list = div()
            .id("rc-list")
            .flex_1()
            .overflow_y_scroll()
            .children(d.drafts.iter().enumerate().map(|(i, dr)| {
                let on = i == d.current;
                let glyph = match Kind::of(&dr.cfg) {
                    Kind::Pytest => "⚗",
                    _ => "▶",
                };
                let name = if dr.cfg.name.trim().is_empty() { "<unnamed>".to_string() } else { dr.cfg.name.clone() };
                div()
                    .id(("rc-item", i))
                    .w_full()
                    .h(px(24.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(on, |x| x.bg(Theme::selection_row()))
                    .when(!on, |x| x.hover(|s| s.bg(Theme::hover_row())))
                    .child(div().text_color(Theme::file_added()).child(glyph))
                    .child(div().flex_1().overflow_hidden().whitespace_nowrap().when(dr.cfg.temporary, |x| x.text_color(Theme::text_muted())).child(name))
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        if let Some(d) = this.runs.dialog.as_mut() {
                            d.select_config(i);
                        }
                        cx.notify();
                    }))
            }))
            .when(d.drafts.is_empty(), |x| x.child(div().p(px(8.)).text_color(Theme::text_dim()).child("No configurations. Add one with + above.")));
        let left = div().w(px(220.)).flex_shrink_0().flex().flex_col().pr(px(10.)).border_r_1().border_color(Theme::divider()).child(list_tools).child(list);

        // Right: the form.
        let caret = || div().w(px(1.5)).h(px(16.)).bg(Theme::caret());
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        if let Some(dr) = d.drafts.get(d.current) {
            for f in fields(kind, dr.cfg.temporary) {
                let active = d.field == f;
                let value: gpui::AnyElement = match f {
                    Field::Kind => div()
                        .flex()
                        .gap(px(4.))
                        .children(Kind::ALL.iter().enumerate().map(|(i, &k)| {
                            let on = k == kind;
                            div()
                                .id(("rc-kind", i))
                                .px(px(8.))
                                .active(|s| s.bg(Theme::press()))
                                .rounded(px(4.))
                                .cursor_pointer()
                                .when(on, |x| x.bg(Theme::selection_row()))
                                .when(!on, |x| x.hover(|s| s.bg(Theme::hover_row())))
                                .text_color(if on { Theme::text() } else { Theme::text_muted() })
                                .child(k.title())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    if let Some(d) = this.runs.dialog.as_mut() {
                                        if let Some(dr) = d.draft() {
                                            k.apply(&mut dr.cfg);
                                        }
                                        d.field = Field::Kind;
                                    }
                                    cx.notify();
                                }))
                        }))
                        .into_any_element(),
                    Field::Temporary => div()
                        .flex()
                        .gap(px(8.))
                        .child(div().text_color(Theme::text_muted()).child(if dr.cfg.temporary { "yes — replaced after 5 newer ones" } else { "no" }))
                        .when(dr.cfg.temporary, |x| {
                            x.child(
                                div()
                                    .id("rc-keep")
                                    .px(px(8.))
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .text_color(Theme::accent())
                                    .hover(|s| s.bg(Theme::hover_row()))
                                    .child("Save configuration")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        if let Some(dr) = this.runs.dialog.as_mut().and_then(|d| d.draft()) {
                                            dr.cfg.temporary = false;
                                        }
                                        cx.notify();
                                    })),
                            )
                        })
                        .into_any_element(),
                    _ => {
                        let text = match f {
                            Field::Name => dr.cfg.name.clone(),
                            Field::Target => dr.cfg.target.clone(),
                            Field::Keywords => dr.cfg.keywords.clone(),
                            Field::Parameters => dr.cfg.parameters.clone(),
                            Field::InterpreterOptions => dr.cfg.interpreter_options.clone(),
                            Field::WorkingDirectory => dr.cfg.working_directory.clone(),
                            Field::Env => dr.env.clone(),
                            Field::Interpreter => dr.cfg.interpreter.clone().unwrap_or_default(),
                            _ => String::new(),
                        };
                        let placeholder = match f {
                            Field::Target => match kind {
                                Kind::Script => "main.py (relative to the project)".to_string(),
                                Kind::Module => "package.module".to_string(),
                                Kind::Pytest => "tests, tests/test_x.py::test_y (empty: everything)".to_string(),
                            },
                            Field::WorkingDirectory => if kind == Kind::Script { "the script's folder".into() } else { "the project root".into() },
                            Field::Env => "KEY=VALUE;KEY2=VALUE2".into(),
                            Field::Interpreter => format!("project default: {project_interp}"),
                            Field::Keywords => "e.g. slow and not network".into(),
                            _ => String::new(),
                        };
                        if text.is_empty() {
                            div().flex().when(active, |x| x.child(caret())).child(div().text_color(Theme::text_dim()).child(placeholder)).into_any_element()
                        } else if active && d.select_all {
                            div().flex().items_center().child(div().bg(Theme::selection()).child(text)).child(caret()).into_any_element()
                        } else if active {
                            let at = text.char_indices().nth(d.caret).map_or(text.len(), |(b, _)| b);
                            let (a, b) = text.split_at(at);
                            div()
                                .flex()
                                .items_center()
                                .whitespace_nowrap()
                                .child(div().child(a.to_string()))
                                .child(caret())
                                .child(div().child(b.to_string()))
                                .into_any_element()
                        } else {
                            div().overflow_hidden().whitespace_nowrap().child(text).into_any_element()
                        }
                    }
                };
                rows.push(
                    div()
                        .id(("rc-field", f as usize))
                        .h(px(32.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(div().w(px(130.)).flex_shrink_0().text_color(Theme::text_muted()).child(label(f, kind)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h(px(28.))
                                .px(px(8.))
                                .flex()
                                .items_center()
                                .overflow_hidden()
                                .rounded(px(5.))
                                .border_1()
                                .border_color(if active { Theme::accent() } else { Theme::divider() })
                                .bg(Theme::bg())
                                .cursor(if f.is_text() { gpui::CursorStyle::IBeam } else { gpui::CursorStyle::Arrow })
                                .child(value),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(d) = this.runs.dialog.as_mut() {
                                if d.field == f && !f.is_text() {
                                    d.toggle(false);
                                }
                                d.focus(f, false);
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
        }
        let preview = d.drafts.get(d.current).map(|dr| command_preview(&self.root, &dr.commit(), &project_interp)).unwrap_or_default();
        let right = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .pl(px(12.))
            .children(rows)
            .when(!preview.is_empty(), |x| x.child(div().pt(px(8.)).text_size(px(11.)).text_color(Theme::text_dim()).overflow_hidden().child(preview)));

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(60.))
                // Modal: clicks outside do nothing (no lost input), Esc closes it.
                .occlude()
                .child(
                    div()
                        .id("run-config-dialog")
                        .w(px(860.))
                        .h(px(470.))
                        .flex()
                        .flex_col()
                        .p(px(16.))
                        .bg(Theme::panel())
                        .border_1()
                        .border_color(Theme::divider())
                        .rounded(px(10.))
                        .shadow_lg()
                        .text_size(px(13.))
                        .text_color(Theme::text())
                        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).pb(px(10.)).child("Run/Debug Configurations"))
                        .child(div().flex_1().min_h_0().flex().child(left).child(right))
                        .when_some(d.error.clone(), |x, e| x.child(div().pt(px(6.)).text_color(Theme::error()).child(e)))
                        .child(
                            div()
                                .pt(px(10.))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(div().flex_1().text_size(px(11.)).text_color(Theme::text_dim()).child("Tab / ↑↓ fields · Alt+↑↓ configurations · Enter OK · Esc Cancel"))
                                .child(button("rc-cancel", "Cancel", false).on_click(cx.listener(|this, _, w, cx| this.run_dialog_cancel(w, cx))))
                                .child(button("rc-apply", "Apply", false).on_click(cx.listener(|this, _, w, cx| {
                                    this.run_dialog_save(false, w, cx);
                                })))
                                .child(button("rc-ok", "OK", true).on_click(cx.listener(|this, _, w, cx| {
                                    this.run_dialog_save(true, w, cx);
                                }))),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Type text into the dialog as key presses with characters.
    fn rc_type(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        for ch in text.chars() {
            let key = if ch == ' ' { "space".to_string() } else { ch.to_lowercase().to_string() };
            let keystroke = gpui::Keystroke { modifiers: gpui::Modifiers::default(), key, key_char: Some(ch.to_string()) };
            self.on_key_down(&KeyDownEvent { keystroke, is_held: false }, window, cx);
        }
    }

    /// `BRISK_SMOKE_RUNCFG=1`: open the dialog, add a module and a pytest configuration
    /// by typing, copy, reorder, remove, fail validation, save, and read run.json back.
    pub(super) fn run_dialog_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let show = |this: &Self| {
            let d = this.runs.dialog.as_ref();
            let names: Vec<String> = d.map(|d| d.drafts.iter().map(|x| x.cfg.name.clone()).collect()).unwrap_or_default();
            format!("open={} current={:?} field={:?} names={names:?} error={:?}", d.is_some(), d.map(|d| d.current), d.map(|d| d.field), d.and_then(|d| d.error.clone()))
        };
        self.dispatch(brisk_keymap::Action::EditRunConfigurations, window, cx);
        eprintln!("smoke-runcfg: open -> {}", show(self));
        // Alt+Insert adds a script configuration with its name selected: type over it.
        self.smoke_press("alt-insert", window, cx);
        self.rc_type("demo", window, cx);
        self.smoke_press("tab", window, cx); // Kind
        self.smoke_press("right", window, cx); // Script -> Module
        self.smoke_press("tab", window, cx); // Target
        self.rc_type("http.server", window, cx);
        self.smoke_press("tab", window, cx); // Parameters
        self.rc_type("8000 --bind 127.0.0.1", window, cx);
        self.smoke_press("tab tab", window, cx); // Working directory
        self.smoke_press("tab", window, cx); // Env
        self.rc_type("A=1;B=two words", window, cx);
        self.smoke_press("home right right", window, cx);
        self.rc_type("X", window, cx); // "A=X1;…"
        eprintln!("smoke-runcfg: typed module -> {}", show(self));
        // Copy it, then make the copy pytest via a click-equivalent toggle.
        self.smoke_press("ctrl-d", window, cx);
        self.smoke_press("tab", window, cx);
        self.smoke_press("right", window, cx); // Module -> pytest
        self.smoke_press("tab", window, cx);
        self.smoke_press("ctrl-a backspace", window, cx);
        self.rc_type("tests", window, cx);
        self.smoke_press("tab", window, cx);
        self.rc_type("not slow", window, cx);
        self.smoke_press("alt-shift-up", window, cx);
        eprintln!("smoke-runcfg: copied as pytest, moved up -> {}", show(self));
        // An empty name fails validation and keeps the dialog open on that field.
        self.smoke_press("alt-insert", window, cx);
        self.smoke_press("ctrl-a backspace enter", window, cx);
        eprintln!("smoke-runcfg: empty name + enter -> {}", show(self));
        self.smoke_press("alt-delete", window, cx);
        self.smoke_press("enter", window, cx);
        eprintln!("smoke-runcfg: removed + enter -> {}", show(self));
        let json = std::fs::read_to_string(self.root.join(brisk_run::CONFIG_FILE)).unwrap_or_default();
        eprintln!("smoke-runcfg: run.json:\n{json}");
        let back = brisk_run::load(&self.root).0;
        let demo = back.get("demo").cloned();
        eprintln!("smoke-runcfg: reloaded demo -> {demo:?}");
        if let Some(c) = demo {
            let python = std::path::PathBuf::from("python");
            let cmd = brisk_run::build(&self.root, &c, &python);
            eprintln!("smoke-runcfg: command -> {} cwd {} env A={:?} B={:?}", cmd.display(), cmd.cwd.display(), cmd.env.get("A"), cmd.env.get("B"));
        }
        // Escape discards edits.
        self.dispatch(brisk_keymap::Action::EditRunConfigurations, window, cx);
        self.smoke_press("ctrl-a", window, cx);
        self.rc_type("renamed", window, cx);
        self.smoke_press("escape", window, cx);
        eprintln!("smoke-runcfg: rename + escape -> {} names on disk {:?}", show(self), brisk_run::load(&self.root).0.configurations.iter().map(|c| c.name.clone()).collect::<Vec<_>>());
        eprintln!("smoke-runcfg: done");
        cx.quit();
    }
}

/// What the configuration runs, as a one-line hint under the form.
fn command_preview(root: &std::path::Path, c: &RunConfig, interp: &str) -> String {
    let python = c.interpreter.clone().unwrap_or_else(|| interp.to_string());
    let target = match (c.kind, c.module) {
        (RunKind::Pytest, _) => {
            let mut s = format!("-m pytest {}", c.target);
            if !c.keywords.is_empty() {
                s.push_str(&format!(" -k \"{}\"", c.keywords));
            }
            s
        }
        (_, true) => format!("-m {}", c.target),
        _ => c.target.clone(),
    };
    let mut parts = vec![python];
    if !c.interpreter_options.is_empty() {
        parts.push(c.interpreter_options.clone());
    }
    parts.push(target);
    if !c.parameters.is_empty() {
        parts.push(c.parameters.clone());
    }
    let _ = root;
    format!("› {}", parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_text_round_trip() {
        let env = env_from_text("A=1; B = x y ;;C=a\\;b\nD=");
        assert_eq!(env.get("A").map(String::as_str), Some("1"));
        assert_eq!(env.get("B").map(String::as_str), Some(" x y "));
        assert_eq!(env.get("C").map(String::as_str), Some("a;b"));
        assert_eq!(env.get("D").map(String::as_str), Some(""));
        assert_eq!(env.len(), 4);
        let mut m = BTreeMap::new();
        m.insert("K".to_string(), "v".to_string());
        m.insert("Z".to_string(), "1".to_string());
        assert_eq!(env_from_text(&env_to_text(&m)), m);
    }

    #[test]
    fn dialog_editing() {
        let mut d = RunDialog { drafts: Vec::new(), current: 0, field: Field::Name, caret: 0, select_all: false, error: None };
        d.add(Kind::Script);
        assert_eq!(d.drafts[0].cfg.name, "Unnamed");
        assert!(d.select_all);
        d.insert("main");
        d.caret = 2;
        d.insert("X");
        assert_eq!(d.drafts[0].cfg.name, "maXin");
        d.delete(false, false);
        assert_eq!(d.drafts[0].cfg.name, "main");
        assert!(d.validate().is_err(), "empty script path");
        d.focus(Field::Target, false);
        d.insert("app.py");
        d.copy();
        assert_eq!(d.drafts[1].cfg.name, "main copy");
        d.move_by(true);
        assert_eq!(d.drafts[0].cfg.name, "main copy");
        d.drafts[0].cfg.name = "main".into();
        assert!(d.validate().unwrap_err().2.contains("Two configurations"));
        d.remove();
        d.add(Kind::Pytest);
        assert_eq!(d.drafts[1].cfg.kind, RunKind::Pytest);
        assert!(d.validate().is_ok(), "pytest may have an empty target");
    }
}
