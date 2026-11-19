//! ty language server: type diagnostics next to ruff's, go to declaration, documentation,
//! completion, parameter info, find usages and rename.

use super::Workspace;
use crate::editor::{DiagSource, Editor};
use brisk_lint::Pos;
use brisk_lsp::Ty;
use gpui::{Context, Entity, Window};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TyState {
    /// Not looked for yet (interpreter discovery still running).
    Unknown,
    Starting,
    Ready,
    /// ty is not installed: features stay off, quietly.
    Missing,
    Failed(String),
}

pub(super) struct Lsp {
    pub ty: Option<Arc<Ty>>,
    pub state: TyState,
    /// Buffer version ty last saw for each open file.
    synced: HashMap<PathBuf, u64>,
    /// Bumped on restart so late replies from an old server are dropped.
    epoch: u64,
    /// Import updates ty gave for the file move in progress.
    pub moved: Option<Vec<brisk_lsp::FileEdits>>,
}

impl Lsp {
    pub fn new() -> Self {
        Lsp { ty: None, state: TyState::Unknown, synced: HashMap::new(), epoch: 0, moved: None }
    }
}

impl Workspace {
    /// ty next to the project interpreter, else next to any other Python found, else on PATH.
    fn ty_tool(&self) -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("BRISK_TY") {
            // Smokes point it at a missing file to run without ty.
            return Some(PathBuf::from(p)).filter(|p| p.is_file());
        }
        let project = self.runs.interpreter(&self.root).map(|i| i.path);
        let others = self.runs.venvs.iter().chain(self.runs.all.iter().flatten()).map(|i| i.path.clone());
        project.into_iter().chain(others).find_map(|p| brisk_lsp::find_tool("ty", Some(&p))).or_else(|| brisk_lsp::find_tool("ty", None))
    }

    /// Start ty once interpreters are known. Cheap to call often.
    pub(super) fn ensure_ty(&mut self, cx: &mut Context<Self>) {
        if let Some(ty) = &self.lsp.ty {
            if !ty.is_alive() {
                self.lsp.ty = None;
                self.lsp.synced.clear();
                self.lsp.state = TyState::Failed("ty exited".into());
                self.status = "ty language server exited; type checks are off until the interpreter changes".into();
            }
            return;
        }
        if self.lsp.state != TyState::Unknown || self.runs.all.is_none() {
            return;
        }
        let Some(tool) = self.ty_tool() else {
            self.lsp.state = TyState::Missing;
            let msg = "Code insight is off: ty not found. Completion offers words from the file; for types, navigation and quick fixes run `pip install ty` in the project interpreter";
            self.status = msg.into();
            self.show_toast(msg.into(), false, cx);
            return;
        };
        self.lsp.state = TyState::Starting;
        let root = self.root.clone();
        let python = self.runs.interpreter(&self.root).map(|i| i.path);
        let epoch = self.lsp.epoch;
        let task = cx.background_executor().spawn(async move { Ty::start(&tool, &root, python.as_deref()) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.lsp.epoch != epoch {
                    return;
                }
                match res {
                    Ok(ty) => {
                        this.lsp.ty = Some(Arc::new(ty));
                        this.lsp.state = TyState::Ready;
                        this.relint_all(cx);
                    }
                    Err(e) => {
                        this.status = format!("ty: {e}");
                        this.lsp.state = TyState::Failed(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The project interpreter changed: ty resolves imports from it, so start a new server.
    pub(super) fn restart_ty(&mut self, cx: &mut Context<Self>) {
        self.lsp.epoch += 1;
        self.lsp.ty = None;
        self.lsp.synced.clear();
        self.lsp.state = TyState::Unknown;
        let eds: Vec<_> = self.editors().cloned().collect();
        for ed in eds {
            ed.update(cx, |e, cx| {
                let v = e.buffer.version();
                if e.set_diagnostics(DiagSource::Ty, v, Vec::new()) {
                    cx.notify();
                }
            });
        }
        self.ensure_ty(cx);
    }

    /// Tell ty about the file's current text when it changed since it last saw it.
    fn ty_sync(&mut self, path: &Path, version: u64, text: &str) -> Option<Arc<Ty>> {
        let ty = self.lsp.ty.clone()?;
        match self.lsp.synced.get(path) {
            Some(&v) if v == version => {}
            Some(_) => ty.did_change(path, version, text),
            None => ty.did_open(path, version, text),
        }
        self.lsp.synced.insert(path.to_path_buf(), version);
        Some(ty)
    }

    /// ty for a request at the editor's caret, with the text synced first.
    pub(super) fn ty_at_caret(&mut self, ed: &Entity<Editor>, cx: &mut Context<Self>) -> Option<(Arc<Ty>, PathBuf, Pos)> {
        self.ensure_ty(cx);
        let e = ed.read(cx);
        if !e.is_python() {
            return None;
        }
        let path = e.path.clone()?;
        let version = e.buffer.version();
        let text = e.text_string();
        let (line, col) = e.caret_line_col();
        let ty = self.ty_sync(&path, version, &text)?;
        Some((ty, path, Pos { line, col }))
    }

    /// Part of the as-you-type check: sync the text and pull ty's diagnostics.
    pub(super) fn ty_check(&mut self, ed: Entity<Editor>, path: PathBuf, version: u64, text: &str, cx: &mut Context<Self>) {
        self.ensure_ty(cx);
        let Some(ty) = self.ty_sync(&path, version, text) else { return };
        let fut = ty.diagnostics(&path);
        // Inlay hints for the whole file, with the same text; off in large files.
        let big = ed.read(cx).is_big();
        let hints = (!big).then(|| ty.inlay_hints(&path, 0..text.lines().count() + 1));
        let epoch = self.lsp.epoch;
        let hint_ed = ed.clone();
        cx.spawn(async move |_, cx| {
            let Some(hints) = hints else { return };
            if let Ok(h) = hints.await {
                let _ = hint_ed.update(cx, |e, cx| {
                    if e.set_inlay_hints(version, h) {
                        cx.notify();
                    }
                });
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            let _ = this.update(cx, |this, cx| {
                if this.lsp.epoch != epoch {
                    return;
                }
                match res {
                    Ok(mut diags) => {
                        for d in &mut diags {
                            d.noqa_line = Some(d.start.line);
                        }
                        ed.update(cx, |e, cx| {
                            if e.set_diagnostics(DiagSource::Ty, version, diags) {
                                cx.notify();
                            }
                        });
                    }
                    Err(_) => this.ensure_ty(cx),
                }
            });
        })
        .detach();
    }

    /// A tab closed: ty can drop the unsaved text and fall back to the file on disk.
    pub(super) fn ty_close(&mut self, path: &Path) {
        if self.lsp.synced.remove(path).is_some() {
            if let Some(ty) = &self.lsp.ty {
                ty.did_close(path);
            }
        }
    }
}

// --- Navigation, documentation, completion, usages, rename -----------------------------

/// A place in a file for Back / Forward (Ctrl+Alt+Left / Right).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NavPos {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
}

#[derive(Default)]
pub(super) struct Nav {
    pub back: Vec<NavPos>,
    pub forward: Vec<NavPos>,
    /// Completion request generation: a newer keystroke drops older replies.
    pub completion_gen: u64,
    /// What Shift+F6 renames: file, symbol position, old name.
    pub rename: Option<(PathBuf, Pos, String)>,
}

const NAV_LIMIT: usize = 100;

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Workspace {
    pub(super) fn no_ty(&mut self, what: &str) {
        self.status = match &self.lsp.state {
            TyState::Missing => format!("{what} needs ty: install it in the project interpreter (pip install ty)"),
            TyState::Failed(e) => format!("{what}: {e}"),
            _ => format!("{what}: ty is starting…"),
        };
    }

    fn here(&self, cx: &gpui::App) -> Option<NavPos> {
        let e = self.active_editor()?.read(cx);
        let (line, col) = e.caret_line_col();
        Some(NavPos { path: e.path.clone()?, line, col })
    }

    fn jump(&mut self, to: &NavPos, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file(&to.path, window, cx);
        if let Some(ed) = self.active_editor().cloned() {
            ed.update(cx, |e, cx| e.select_on_line(to.line, to.col..to.col, cx));
        }
    }

    /// Navigate and remember where we came from.
    pub(super) fn navigate_to(&mut self, to: NavPos, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(h) = self.here(cx) {
            if h != to && self.nav.back.last() != Some(&h) {
                self.nav.back.push(h);
                if self.nav.back.len() > NAV_LIMIT {
                    self.nav.back.remove(0);
                }
            }
        }
        self.nav.forward.clear();
        self.jump(&to, window, cx);
    }

    fn nav_step(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let target = if back { self.nav.back.pop() } else { self.nav.forward.pop() };
        let Some(target) = target else {
            self.status = if back { "No place to go back to".into() } else { "No place to go forward to".into() };
            return;
        };
        if let Some(h) = self.here(cx) {
            if back {
                self.nav.forward.push(h);
            } else {
                self.nav.back.push(h);
            }
        }
        if target.path.is_file() {
            self.places.jumping = true;
            self.jump(&target, window, cx);
        }
    }

    fn editor_for_lsp(&self, window: &Window, cx: &gpui::App) -> Option<Entity<Editor>> {
        self.focused_editor(window, cx).or_else(|| self.active_editor().cloned()).filter(|e| e.read(cx).is_python())
    }

    /// Code-insight actions; false when the action is not one of them.
    pub(super) fn lsp_action(&mut self, action: brisk_keymap::Action, window: &mut Window, cx: &mut Context<Self>) -> bool {
        use brisk_keymap::Action as A;
        match action {
            A::NavigateBack | A::NavigateForward => {
                self.nav_step(action == A::NavigateBack, window, cx);
                return true;
            }
            A::ExtractVariable | A::ExtractMethod | A::ExtractConstant => {
                self.extract_action(action, window, cx);
                return true;
            }
            A::Inline => {
                self.inline_action(window, cx);
                return true;
            }
            A::GotoDeclaration | A::GotoTypeDeclaration | A::QuickDefinition | A::QuickDocumentation | A::ParameterInfo | A::CodeCompletion | A::SmartCompletion | A::FindUsages | A::ShowUsages | A::Rename => {}
            _ => return false,
        }
        let Some(ed) = self.editor_for_lsp(window, cx) else {
            self.status = format!("{} works in Python files", action.info().title);
            return true;
        };
        match action {
            A::GotoDeclaration => self.goto_declaration(ed, None, false, window, cx),
            A::GotoTypeDeclaration => self.goto_declaration(ed, None, true, window, cx),
            A::QuickDocumentation => self.quick_doc(ed, None, cx),
            A::QuickDefinition => self.quick_definition(ed, cx),
            A::ParameterInfo => self.parameter_info(ed, true, cx),
            A::CodeCompletion | A::SmartCompletion => self.complete(ed, true, cx),
            A::FindUsages | A::ShowUsages => self.find_usages(ed, None, window, cx),
            A::Rename => self.start_rename(ed, window, cx),
            _ => {}
        }
        true
    }

    /// Editor events that drive code insight.
    pub(super) fn lsp_editor_event(&mut self, ed: Entity<Editor>, ev: &crate::editor::EditorEvent, window: &mut Window, cx: &mut Context<Self>) {
        use crate::editor::EditorEvent as E;
        match *ev {
            E::Typed(c) => {
                if !ed.read(cx).is_python() {
                    return;
                }
                let ready = self.lsp.state == TyState::Ready;
                let (open, sig_open, word, requery) = {
                    let e = ed.read(cx);
                    let (line, col) = e.caret_line_col();
                    let prefix: String = e.buffer.line(line).chars().take(col).collect();
                    let start = e.word_start_before_caret();
                    let word: String = e.buffer.rope().slice(start..e.buffer.primary().head).to_string();
                    let in_comment = prefix.contains('#');
                    (e.completion_open() || in_comment, e.signature_open(), word, e.completion_needs_requery() && !in_comment)
                };
                if c == '(' || (c == ',' && sig_open) {
                    self.parameter_info(ed.clone(), false, cx);
                }
                if ready && is_ident(c) && requery {
                    self.complete(ed, false, cx);
                    return;
                }
                if open {
                    return;
                }
                // Auto-popup on the first letter of a word, or after a dot.
                let starts_word = is_ident(c) && word.chars().count() == 1 && !c.is_ascii_digit();
                if c == '.' || starts_word {
                    self.complete(ed, false, cx);
                }
            }
            // While the debugger is paused the hover shows the value (falls back to the docs).
            E::HoverDoc(pos, at) if self.debug_hover(&ed, pos, at, cx) => {}
            E::HoverDoc(pos, at) => self.quick_doc(ed, Some((pos, at)), cx),
            E::CtrlClick(pos) => self.goto_declaration(ed, Some(pos), false, window, cx),
            E::ShowProblems | E::ContextMenu(_) | E::BreakpointsChanged | E::EditBreakpoint(_) | E::GutterMenu(_) | E::ShowCommit(_) | E::ShowVcsDiff | E::RunMark(..) | E::SetNextStatement(_) => {}
        }
    }

    fn pos_in(ed: &Entity<Editor>, at: Option<usize>, p: Pos, cx: &gpui::App) -> Pos {
        match at {
            Some(c) => {
                let (line, col) = ed.read(cx).buffer.pos_to_line_col(c);
                Pos { line, col }
            }
            None => p,
        }
    }

    /// Ctrl+B / Ctrl+Click; on the declaration itself it shows usages (PyCharm's
    /// "Go to Declaration or Usages"). Ctrl+Shift+B goes to the type's declaration.
    fn goto_declaration(&mut self, ed: Entity<Editor>, at: Option<usize>, type_def: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((ty, path, p)) = self.ty_at_caret(&ed, cx) else {
            // No ty: a `def`, `class` or first assignment of the name in this file.
            let found = ed.read(cx).local_declaration(at.unwrap_or_else(|| ed.read(cx).buffer.primary().head));
            match (found, ed.read(cx).path.clone()) {
                (Some((line, col)), Some(path)) if !type_def => self.navigate_to(NavPos { path, line, col }, window, cx),
                _ => self.no_ty("Go to Declaration"),
            }
            return;
        };
        let p = Self::pos_in(&ed, at, p, cx);
        let fut = if type_def { futures::future::Either::Left(ty.type_definition(&path, p)) } else { futures::future::Either::Right(ty.definition(&path, p)) };
        cx.spawn_in(window, async move |this, cx| {
            let res = fut.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match res {
                    Ok(locs) if locs.is_empty() => {
                        this.status = if type_def { "Cannot find type declaration to go to".into() } else { "Cannot find declaration to go to".into() }
                    }
                    Ok(locs) => {
                        let l = &locs[0];
                        let on_itself = l.path == path && l.start.line == p.line && l.start.col <= p.col && p.col <= l.end.col;
                        if on_itself && !type_def {
                            this.find_usages(ed, Some(p), window, cx);
                        } else {
                            this.navigate_to(NavPos { path: l.path.clone(), line: l.start.line, col: l.start.col }, window, cx);
                        }
                    }
                    Err(e) => this.status = format!("ty: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Ctrl+Q at the caret, or the mouse resting on a word.
    pub(super) fn quick_doc(&mut self, ed: Entity<Editor>, mouse: Option<(usize, gpui::Point<gpui::Pixels>)>, cx: &mut Context<Self>) {
        let Some((ty, path, p)) = self.ty_at_caret(&ed, cx) else {
            if mouse.is_none() {
                self.no_ty("Quick Documentation");
            }
            return;
        };
        let p = Self::pos_in(&ed, mouse.map(|m| m.0), p, cx);
        let fut = ty.hover(&path, p);
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            let _ = this.update(cx, |this, cx| match res {
                Ok(Some(md)) => {
                    let anchor = match mouse {
                        Some((pos, at)) => crate::editor::Anchor::Mouse(pos, at),
                        None => crate::editor::Anchor::Caret,
                    };
                    ed.update(cx, |e, cx| e.show_doc(&md, anchor, cx));
                }
                Ok(None) if mouse.is_none() => {
                    this.status = "No documentation found".into();
                    cx.notify();
                }
                _ => {}
            });
        })
        .detach();
    }

    /// Extract Variable (Ctrl+Alt+V), Constant (Ctrl+Alt+C) and Method (Ctrl+Alt+M), as in
    /// PyCharm: without a selection the "Expressions" popup picks one of the expressions at the
    /// caret; with several identical ones a popup asks whether to replace this one or all.
    pub(super) fn extract_action(&mut self, action: brisk_keymap::Action, window: &mut Window, cx: &mut Context<Self>) {
        use super::problems_ui::Intent;
        use super::{PopupItem, PopupKind, Target};
        use brisk_keymap::Action as A;
        let title = action.info().title;
        let Some(ed) = self.editor_for_lsp(window, cx) else {
            self.status = format!("{title} works in Python files");
            cx.notify();
            return;
        };
        if action == A::ExtractMethod {
            self.status = ed.update(cx, |e, cx| e.extract_method(cx)).unwrap_or_else(|e| format!("Extract Method: {e}"));
            cx.notify();
            return;
        }
        let constant = action == A::ExtractConstant;
        let item = |label: String, intent: Intent| PopupItem { label, detail: String::new(), hint: String::new(), target: Target::Intention(Box::new(intent)) };
        if ed.read(cx).extract_count() == 0 {
            let choices = ed.update(cx, |e, _| e.expression_choices());
            match choices.len() {
                0 => self.status = format!("{title}: no expression here"),
                1 => {
                    ed.update(cx, |e, _| e.buffer.selections = vec![brisk_text::Selection { anchor: choices[0].start, head: choices[0].end, goal_col: None }]);
                    return self.extract_action(action, window, cx);
                }
                _ => {
                    let chars: Vec<char> = ed.read(cx).text_string().chars().collect();
                    self.problems.intents = choices
                        .into_iter()
                        .map(|r| {
                            let s: String = chars[r.clone()].iter().collect();
                            let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
                            let label = if one.chars().count() > 60 { format!("{}…", one.chars().take(59).collect::<String>()) } else { one };
                            item(label, Intent::PickExpression(r, constant))
                        })
                        .collect();
                    self.problems.intents_title = Some("Expressions");
                    self.open_popup(PopupKind::Intentions);
                }
            }
            cx.notify();
            return;
        }
        let n = if constant { ed.read(cx).constant_count() } else { ed.read(cx).extract_count() };
        if n == 1 {
            ed.update(cx, |e, cx| if constant { e.extract_constant(false, cx) } else { e.extract_variable(false, cx) });
        } else {
            let pick = |all: bool| if constant { Intent::ExtractConstant(all) } else { Intent::ExtractVariable(all) };
            self.problems.intents = vec![item("Replace this occurrence only".into(), pick(false)), item(format!("Replace all {n} occurrences"), pick(true))];
            self.problems.intents_title = Some("Multiple occurrences found");
            self.open_popup(PopupKind::Intentions);
        }
        cx.notify();
    }

    /// Inline (Ctrl+Alt+N): a variable set once, or a function whose body is one `return`
    /// (PyCharm asks: all invocations and remove it, or this one only).
    pub(super) fn inline_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use super::problems_ui::Intent;
        use super::{PopupItem, PopupKind, Target};
        use brisk_syntax::Inline;
        let Some(ed) = self.editor_for_lsp(window, cx) else {
            self.status = "Inline works in Python files".into();
            cx.notify();
            return;
        };
        match ed.update(cx, |e, _| e.inline_at_caret()) {
            Err(e) => self.status = format!("Inline: {e}"),
            Ok(Inline::Variable { name, edits, uses }) => {
                ed.update(cx, |e, cx| e.apply_byte_edits(edits, cx));
                self.status = format!("Inlined variable '{name}' ({uses} usage{})", if uses == 1 { "" } else { "s" });
            }
            Ok(Inline::Function { name, all, this, calls }) => {
                let item = |label: String, edits: Vec<_>| PopupItem { label, detail: String::new(), hint: String::new(), target: Target::Intention(Box::new(Intent::Edits(edits))) };
                let mut items = Vec::new();
                if let Some(a) = all {
                    items.push(item(format!("Inline all {calls} invocation{} and remove '{name}'", if calls == 1 { "" } else { "s" }), a));
                }
                if let Some(t) = this {
                    items.push(item("Inline this invocation only and keep the function".into(), t));
                }
                self.problems.intents = items;
                self.problems.intents_title = Some("Inline Function");
                self.open_popup(PopupKind::Intentions);
            }
        }
        cx.notify();
    }

    /// Ctrl+Shift+I: the definition's source in a popup at the caret. Without ty, a `def`,
    /// `class` or assignment of the name in this file.
    fn quick_definition(&mut self, ed: Entity<Editor>, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let place = move |path: &Path, line: usize| format!("{}:{}", path.strip_prefix(&root).unwrap_or(path).to_string_lossy().replace('\\', "/"), line + 1);
        let Some((ty, path, p)) = self.ty_at_caret(&ed, cx) else {
            let (found, text, path) = {
                let e = ed.read(cx);
                (e.local_declaration(e.buffer.primary().head), e.text_string(), e.path.clone())
            };
            match (found.and_then(|(line, _)| crate::editor::definition_source(&text, line)), path) {
                (Some((first, src)), Some(path)) => ed.update(cx, |e, cx| e.show_definition(&place(&path, first), &src, cx)),
                _ => self.no_ty("Quick Definition"),
            }
            return;
        };
        let fut = ty.definition(&path, p);
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(locs) if !locs.is_empty() => {
                        let l = &locs[0];
                        // The open editor's text when the file is open (it may be unsaved).
                        let open = this.editors().find(|e| e.read(cx).path.as_deref() == Some(l.path.as_path())).map(|e| e.read(cx).text_string());
                        let text = open.or_else(|| std::fs::read_to_string(&l.path).ok()).unwrap_or_default();
                        match crate::editor::definition_source(&text, l.start.line) {
                            Some((first, src)) => ed.update(cx, |e, cx| e.show_definition(&place(&l.path, first), &src, cx)),
                            None => this.status = "Cannot find definition".into(),
                        }
                    }
                    Ok(_) => this.status = "Cannot find definition".into(),
                    Err(e) => this.status = format!("ty: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Ctrl+P, and automatically after `(` and `,`.
    fn parameter_info(&mut self, ed: Entity<Editor>, explicit: bool, cx: &mut Context<Self>) {
        let Some((ty, path, p)) = self.ty_at_caret(&ed, cx) else {
            // No ty: a `def` in this file or a builtin.
            let sig = ed.read(cx).local_signature();
            if sig.is_none() && explicit {
                self.status = "No parameter info here (without ty only functions of this file and builtins are known)".into();
            }
            ed.update(cx, |e, cx| e.show_signature(sig, cx));
            return;
        };
        // Show the file's own `def` or the builtin at once; ty's answer replaces it when it comes
        // (ty may take a while, or have nothing, while it checks a just-edited file).
        let local = ed.read(cx).local_signature();
        let had_local = local.is_some();
        if had_local {
            ed.update(cx, |e, cx| e.show_signature(local, cx));
        }
        let fut = ty.signature_help(&path, p);
        cx.spawn(async move |this, cx| {
            let sig = fut.await.ok().flatten();
            let none = ed
                .update(cx, |e, cx| {
                    match sig {
                        // Not if the local one was shown and closed meanwhile.
                        Some(sig) if !had_local || e.signature_open() => e.show_signature(Some(sig), cx),
                        Some(_) => {}
                        None if !had_local => e.show_signature(None, cx),
                        None => {}
                    }
                    !had_local && !e.signature_open()
                })
                .unwrap_or(false);
            if none && explicit {
                let _ = this.update(cx, |this, cx| {
                    this.status = "No parameter info here".into();
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Ctrl+Space (`explicit`), or automatically while typing an identifier or after `.`.
    fn complete(&mut self, ed: Entity<Editor>, explicit: bool, cx: &mut Context<Self>) {
        let Some((ty, path, p)) = self.ty_at_caret(&ed, cx) else {
            // No ty (yet): words of the file, keywords and builtins.
            self.nav.completion_gen += 1;
            let shown = ed.update(cx, |e, cx| e.show_word_completion(cx));
            if explicit && !shown {
                self.status = "No suggestions".into();
            }
            return;
        };
        let start = ed.read(cx).word_start_before_caret();
        let asked_at = ed.read(cx).buffer.primary().head;
        self.nav.completion_gen += 1;
        let my = self.nav.completion_gen;
        let fut = ty.completion_list(&path, p);
        // ty's first answers can take seconds while it indexes: show the words of the file
        // meanwhile; ty's list replaces them when it comes.
        let timer = cx.background_executor().timer(std::time::Duration::from_millis(400));
        let ed2 = ed.clone();
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                if this.nav.completion_gen == my {
                    ed2.update(cx, |e, cx| {
                        if !e.completion_open() && e.word_start_before_caret() == start {
                            e.show_word_completion(cx);
                        }
                    });
                }
            });
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            let _ = this.update(cx, |this, cx| {
                if this.nav.completion_gen != my {
                    return;
                }
                match res {
                    // Postfix templates show even when ty has nothing for `expr.`.
                    Ok((items, incomplete)) if !items.is_empty() || !ed.read(cx).postfix_items(start).is_empty() => {
                        // Still typing the same word?
                        let again = ed.update(cx, |e, cx| {
                            if e.word_start_before_caret() != start {
                                return false;
                            }
                            e.show_completion_list(items, start, incomplete, cx);
                            // Typed on while ty answered for a shorter prefix: its capped list
                            // may lack the match (auto-import items), so ask for the new one.
                            incomplete && e.buffer.primary().head != asked_at
                        });
                        if again {
                            this.complete(ed, explicit, cx);
                        }
                    }
                    Ok(_) if explicit => {
                        this.status = "No suggestions".into();
                        cx.notify();
                    }
                    _ => {}
                }
            });
        })
        .detach();
    }

    /// Alt+F7: usages in the Find tool window, like PyCharm's Find Usages.
    fn find_usages(&mut self, ed: Entity<Editor>, at: Option<Pos>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((ty, path, p)) = self.ty_at_caret(&ed, cx) else { return self.local_usages(ed, window, cx) };
        let p = at.unwrap_or(p);
        let name = {
            let e = ed.read(cx);
            let pos = e.buffer.line_col_to_pos(p.line, p.col);
            let r = e.buffer.word_range_at(pos);
            e.buffer.rope().slice(r).to_string()
        };
        let fut = ty.references(&path, p);
        cx.spawn_in(window, async move |this, cx| {
            let res = fut.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match res {
                    Ok(locs) if locs.is_empty() => this.status = format!("No usages of '{name}' found"),
                    Ok(locs) => {
                        let usages: Vec<(PathBuf, usize, std::ops::Range<usize>)> = locs
                            .into_iter()
                            .map(|l| {
                                let end = if l.end.line == l.start.line { l.end.col } else { l.start.col + name.chars().count() };
                                (l.path, l.start.line, l.start.col..end)
                            })
                            .collect();
                        let n = this.show_usages(format!("Usages of '{name}'"), usages, window, cx);
                        this.status = if n == 0 { format!("No usages of '{name}' found") } else { format!("{n} usage(s) of '{name}'") };
                    }
                    Err(e) => this.status = format!("ty: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Shift+F6: check the symbol can be renamed, then ask for the new name.
    fn start_rename(&mut self, ed: Entity<Editor>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((ty, path, p)) = self.ty_at_caret(&ed, cx) else { return self.local_rename(ed, cx) };
        let fut = ty.prepare_rename(&path, p);
        cx.spawn_in(window, async move |this, cx| {
            let res = fut.await;
            let _ = this.update_in(cx, |this, _, cx| {
                match res {
                    Ok(Some((s, e))) => {
                        let old: String = ed.read(cx).buffer.line(s.line).chars().skip(s.col).take(e.col.saturating_sub(s.col)).collect();
                        this.nav.rename = Some((path, p, old.clone()));
                        this.open_popup(super::PopupKind::Rename);
                        if let Some(pp) = this.popup.as_mut() {
                            pp.query = old;
                        }
                        this.refresh_popup();
                    }
                    Ok(None) => this.status = "Cannot rename this element".into(),
                    Err(e) => this.status = format!("ty: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn rename_items(&self, q: &str) -> Vec<super::PopupItem> {
        let Some((_, _, old)) = &self.nav.rename else { return Vec::new() };
        let valid = !q.is_empty() && q.chars().all(is_ident) && !q.starts_with(|c: char| c.is_ascii_digit());
        if !valid || q == old {
            return Vec::new();
        }
        vec![super::PopupItem { label: format!("Rename '{old}' to '{q}'"), detail: "in all files".into(), hint: "Enter".into(), target: super::Target::Rename(q.to_string()) }]
    }

    /// Apply the rename across the project: open files as one undo step each, the rest on disk.
    pub(super) fn apply_rename(&mut self, new_name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_editor(window, cx);
        let Some((path, p, old)) = self.nav.rename.take() else { return };
        let Some(ty) = self.lsp.ty.clone() else { return self.no_ty("Rename") };
        let fut = ty.rename(&path, p, &new_name);
        cx.spawn_in(window, async move |this, cx| {
            let res = fut.await;
            let _ = this.update_in(cx, |this, _, cx| {
                match res {
                    Ok(files) if files.is_empty() => this.status = "Nothing to rename".into(),
                    Ok(files) => {
                        let (count, failed) = this.apply_file_edits(brisk_lsp::merge_file_edits(files), cx);
                        this.status = format!("Renamed '{old}' to '{new_name}': {count} occurrence(s)");
                        if !failed.is_empty() {
                            this.show_toast(format!("Rename could not write: {}", failed.join("; ")), true, cx);
                        }
                        this.refresh_git(cx);
                    }
                    Err(e) => this.status = format!("ty: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Edits from the server: open files change in their editors (one undo step each),
    /// closed files on disk. Returns (edit count, write errors).
    pub(super) fn apply_file_edits(&mut self, files: Vec<brisk_lsp::FileEdits>, cx: &mut Context<Self>) -> (usize, Vec<String>) {
        let mut on_disk = Vec::new();
        let mut count = 0;
        let mut failed = Vec::new();
        for (file, edits) in files {
            count += edits.len();
            let open = self.editors().find(|e| e.read(cx).path.as_deref() == Some(file.as_path())).cloned();
            // Local History: every file as it was before the refactoring.
            match &open {
                Some(ed) => crate::local_history::record(&file, &ed.read(cx).buffer.text(), crate::local_history::Label::BeforeRefactor),
                None => crate::local_history::record_path(&file, crate::local_history::Label::BeforeRefactor),
            }
            match open {
                Some(ed) => {
                    let fix = brisk_lint::Fix { message: String::new(), safe: true, edits };
                    ed.update(cx, |e, cx| e.apply_fix(&fix, cx));
                }
                None => {
                    let r = std::fs::read_to_string(&file).and_then(|t| {
                        let crlf = t.contains("\r\n");
                        let new = brisk_lint::apply_edits(&t.replace("\r\n", "\n"), &edits);
                        std::fs::write(&file, if crlf { new.replace('\n', "\r\n") } else { new })
                    });
                    match r {
                        Ok(()) => on_disk.push(file),
                        Err(e) => failed.push(format!("{}: {e}", file.display())),
                    }
                }
            }
        }
        if let (Some(ty), false) = (&self.lsp.ty, on_disk.is_empty()) {
            ty.files_changed(&on_disk);
        }
        (count, failed)
    }
}

// --- Moving modules ----------------------------------------------------------------------

impl Workspace {
    /// The project file list was rescanned: tell ty which Python files appeared or went away.
    pub(super) fn ty_files_rescanned(&mut self, old: Option<Arc<brisk_project::FileList>>, new: &brisk_project::FileList) {
        let (Some(old), Some(ty)) = (old, self.lsp.ty.as_ref()) else { return };
        let py = |f: &&String| f.ends_with(".py") || f.ends_with(".pyi");
        let before: std::collections::HashSet<&String> = old.files.iter().filter(py).collect();
        let after: std::collections::HashSet<&String> = new.files.iter().filter(py).collect();
        let abs = |f: &&String| new.root.join(f.as_str());
        let created: Vec<PathBuf> = after.difference(&before).map(abs).collect();
        let deleted: Vec<PathBuf> = before.difference(&after).map(abs).collect();
        ty.files_created_deleted(&created, &deleted);
    }

    /// Rename / Move of a Python file or a folder in the Project tree: ask ty for the import
    /// updates (`workspace/willRenameFiles`), then run the move again. False when there is
    /// nothing to ask (not Python, no ty, or the answer is already here).
    pub(super) fn will_move_python(&mut self, src: &Path, target: &Path, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.lsp.moved.is_some() {
            return false;
        }
        let python = src.is_dir() || matches!(src.extension().and_then(|e| e.to_str()), Some("py" | "pyi"));
        let Some(ty) = self.lsp.ty.clone().filter(|_| python && self.lsp.state == TyState::Ready) else { return false };
        // ty should see the unsaved text of open files, which the move saves anyway.
        let open: Vec<(PathBuf, u64, String)> = self
            .editors()
            .filter_map(|e| {
                let e = e.read(cx);
                Some((e.path.clone()?, e.buffer.version(), e.text_string())).filter(|_| e.is_python())
            })
            .collect();
        for (p, v, t) in open {
            self.ty_sync(&p, v, &t);
        }
        let fut = ty.will_rename_files(&[(src.to_path_buf(), target.to_path_buf())]);
        let timeout = cx.background_executor().timer(std::time::Duration::from_secs(3));
        let (src, target) = (src.to_path_buf(), target.to_path_buf());
        cx.spawn_in(window, async move |this, cx| {
            use futures::FutureExt;
            let edits = match futures::future::select(fut.boxed(), timeout.boxed()).await {
                futures::future::Either::Left((Ok(e), _)) => e,
                _ => Vec::new(),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let mut edits = brisk_lsp::merge_file_edits(edits);
                // ty 0.0.104 answers every folder rename, and any module moved to another
                // folder, with nothing: update the imports ourselves then.
                if edits.is_empty() {
                    edits = this.module_rename_edits(&src, &target, cx);
                }
                this.lsp.moved = Some(edits);
                this.run_file_op(target, window, cx);
            });
        })
        .detach();
        true
    }

    /// After the move: apply ty's import updates (files that moved along are at their new
    /// place now) and tell ty about the rename.
    pub(super) fn after_python_move(&mut self, src: &Path, target: &Path, cx: &mut Context<Self>) {
        let Some(edits) = self.lsp.moved.take() else { return };
        let edits: Vec<brisk_lsp::FileEdits> = edits
            .into_iter()
            .filter(|(_, e)| !e.is_empty())
            .map(|(p, e)| match p.strip_prefix(src) {
                Ok(rest) if rest.as_os_str().is_empty() => (target.to_path_buf(), e),
                Ok(rest) => (target.join(rest), e),
                Err(_) => (p, e),
            })
            .collect();
        if let Some(ty) = &self.lsp.ty {
            ty.did_rename_files(&[(src.to_path_buf(), target.to_path_buf())]);
            // A moved folder: name every module that left and arrived, as ty does not watch
            // the disk and may not expand a folder rename itself.
            if target.is_dir() {
                let arrived = python_files_under(target);
                let mut left: Vec<PathBuf> = arrived.iter().filter_map(|f| Some(src.join(f.strip_prefix(target).ok()?))).collect();
                // The folder itself too: ty then drops everything it had under it.
                left.push(src.to_path_buf());
                ty.files_created_deleted(&arrived, &left);
            }
        }
        if edits.is_empty() {
            return;
        }
        let files = edits.len();
        let (count, failed) = self.apply_file_edits(edits, cx);
        self.status = format!("{}; updated {count} import reference(s) in {files} file(s)", self.status);
        if !failed.is_empty() {
            self.show_toast(format!("Could not update imports: {}", failed.join("; ")), true, cx);
        }
    }
}

impl Workspace {
    /// Import updates for moving `src` to `target`, found by Brisk in the project's Python
    /// files (open ones with their unsaved text): `from`/`import` lines and `old.attr` uses.
    fn module_rename_edits(&self, src: &Path, target: &Path, cx: &gpui::App) -> Vec<brisk_lsp::FileEdits> {
        let (Some(old), Some(new)) = (brisk_lsp::module_name(&self.root, src), brisk_lsp::module_name(&self.root, target)) else { return Vec::new() };
        if old == new {
            return Vec::new();
        }
        let open: HashMap<PathBuf, String> = self.editors().filter_map(|e| Some((e.read(cx).path.clone()?, e.read(cx).text_string()))).collect();
        python_files_under(&self.root)
            .into_iter()
            .filter_map(|f| {
                let text = open.get(&f).cloned().or_else(|| std::fs::read_to_string(&f).ok())?;
                let edits = brisk_lsp::rename_module_refs(&text, &old, &new);
                (!edits.is_empty()).then_some((f, edits))
            })
            .collect()
    }
}

/// `.py` / `.pyi` files under `dir`, skipping hidden folders (`.venv`, `.git`) and caches.
fn python_files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            match e.file_type() {
                Ok(t) if t.is_dir() => {
                    if !name.starts_with('.') && name != "__pycache__" && name != "node_modules" {
                        stack.push(p);
                    }
                }
                Ok(_) if name.ends_with(".py") || name.ends_with(".pyi") => out.push(p),
                _ => {}
            }
        }
    }
    out.sort();
    out
}

// --- Smoke test ----------------------------------------------------------------------

impl Workspace {
    /// `BRISK_SMOKE_LSP=<main.py>`: with ty, check diagnostics, Ctrl+B and back, Ctrl+Q,
    /// completion, parameter info, find usages and rename. `main.py` must call `add` from
    /// `pkg/util.py` on line 4 (see the README).
    pub(super) fn lsp_smoke(&mut self, file: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        use brisk_keymap::Action as A;
        let t0 = std::time::Instant::now();
        self.open_file(&file, window, cx);
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            // ty starts after interpreter discovery; then the file gets both checks.
            for _ in 0..200 {
                pause(50).await;
                let ready = this
                    .update(cx, |this, cx| {
                        this.lsp.state != TyState::Unknown && this.lsp.state != TyState::Starting
                            && this.active_editor().is_some_and(|e| e.read(cx).diagnostics.iter().any(|d| brisk_lint::is_ty_code(&d.code)))
                    })
                    .unwrap_or(true);
                if ready {
                    break;
                }
            }
            let step = |this: &gpui::WeakEntity<Self>, cx: &mut gpui::AsyncWindowContext, f: &mut dyn FnMut(&mut Self, &mut Window, &mut Context<Self>)| {
                let _ = this.update_in(cx, |this, window, cx| f(this, window, cx));
            };
            step(&this, cx, &mut |this, _, cx| {
                eprintln!("smoke-lsp: ty {:?} after {:.1?}", this.lsp.state, t0.elapsed());
                if let Some(ed) = this.active_editor() {
                    for d in &ed.read(cx).diagnostics {
                        eprintln!("smoke-lsp: {:?} {}:{} {}", d.severity, d.start.line + 1, d.start.col + 1, d.title());
                    }
                }
            });
            let where_ = |this: &Self, cx: &gpui::App| this.here(cx).map(|h| format!("{}:{}:{}", h.path.file_name().unwrap().to_string_lossy(), h.line + 1, h.col + 1));
            // Ctrl+B on `add` in `print(add(3, 4))` (line 4).
            step(&this, cx, &mut |this, window, cx| {
                if let Some(ed) = this.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.select_on_line(3, 7..7, cx));
                }
                this.dispatch(A::GotoDeclaration, window, cx);
            });
            pause(400).await;
            step(&this, cx, &mut |this, window, cx| {
                eprintln!("smoke-lsp: Ctrl+B -> {:?}", where_(this, cx));
                this.dispatch(A::NavigateBack, window, cx);
                eprintln!("smoke-lsp: Back -> {:?}", where_(this, cx));
                this.dispatch(A::NavigateForward, window, cx);
                eprintln!("smoke-lsp: Forward -> {:?}", where_(this, cx));
                this.dispatch(A::NavigateBack, window, cx);
                this.dispatch(A::QuickDocumentation, window, cx);
            });
            pause(300).await;
            step(&this, cx, &mut |this, _, cx| {
                if let Some(ed) = this.active_editor().cloned() {
                    eprintln!("smoke-lsp: Ctrl+Q -> {}", ed.read(cx).insight_summary());
                    // Type at the end of the file: completion after a dot, then a call.
                    ed.update(cx, |e, cx| {
                        let end = e.buffer.len_chars();
                        e.buffer.set_caret(end, false);
                        e.type_for_smoke("\nimport pkg.util\npkg.util.", cx);
                    });
                }
            });
            pause(500).await;
            step(&this, cx, &mut |this, _, cx| {
                if let Some(ed) = this.active_editor().cloned() {
                    eprintln!("smoke-lsp: after 'pkg.util.' -> {}", ed.read(cx).insight_summary());
                    ed.update(cx, |e, cx| e.type_for_smoke("a", cx));
                    eprintln!("smoke-lsp: after 'a' -> {}", ed.read(cx).insight_summary());
                    ed.update(cx, |e, cx| e.completion_key("enter", cx));
                }
            });
            pause(400).await;
            step(&this, cx, &mut |this, _, cx| {
                if let Some(ed) = this.active_editor().cloned() {
                    let (l, _) = ed.read(cx).caret_line_col();
                    eprintln!("smoke-lsp: accepted -> {:?}, {}", ed.read(cx).buffer.line(l), ed.read(cx).insight_summary());
                    ed.update(cx, |e, cx| e.type_for_smoke("1,", cx));
                }
            });
            pause(400).await;
            step(&this, cx, &mut |this, window, cx| {
                if let Some(ed) = this.active_editor().cloned() {
                    eprintln!("smoke-lsp: after '1,' -> {}", ed.read(cx).insight_summary());
                    ed.update(cx, |e, cx| e.select_on_line(3, 7..7, cx));
                }
                this.dispatch(A::FindUsages, window, cx);
            });
            pause(400).await;
            step(&this, cx, &mut |this, window, cx| {
                eprintln!("smoke-lsp: Alt+F7 -> {} / {}", this.status, this.find.window.as_ref().map(|w| w.rows_len()).unwrap_or(0));
                this.focus_editor(window, cx);
                this.dispatch(A::Rename, window, cx);
            });
            pause(400).await;
            step(&this, cx, &mut |this, window, cx| {
                let q = this.popup.as_ref().map(|p| p.query.clone());
                eprintln!("smoke-lsp: Shift+F6 popup prefilled {q:?}");
                this.popup = None;
                this.apply_rename("plus".into(), window, cx);
            });
            pause(500).await;
            step(&this, cx, &mut |this, _, cx| {
                eprintln!("smoke-lsp: {}", this.status);
                if let Some(ed) = this.active_editor() {
                    eprintln!("smoke-lsp: main.py now {:?}", ed.read(cx).text_string().lines().take(4).collect::<Vec<_>>());
                }
                // Ctrl+B opened util.py, so its rename is an unsaved edit in that tab.
                let util = this.root.join("pkg").join("util.py");
                let tab = this.editors().find(|e| e.read(cx).path.as_deref() == Some(util.as_path())).map(|e| e.read(cx).text_string());
                eprintln!("smoke-lsp: util.py tab {:?}", tab.and_then(|t| t.lines().next().map(str::to_string)));
                eprintln!("smoke-lsp: done in {:.1?}", t0.elapsed());
            });
        })
        .detach();
    }
}

