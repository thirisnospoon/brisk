//! ty language server: type diagnostics next to ruff's, go to declaration, documentation,
//! completion, parameter info, find usages and rename.

use super::ts_ui::is_ts_path;
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
    /// TypeScript's server for JS and TS files (workspace/ts_ui.rs).
    pub ts: super::ts_ui::TsLsp,
}

impl Lsp {
    pub fn new() -> Self {
        Lsp { ty: None, state: TyState::Unknown, synced: HashMap::new(), epoch: 0, moved: None, ts: Default::default() }
    }
}

impl Workspace {
    /// ty next to the project interpreter, else next to any other Python found, else on PATH.
    fn ty_tool(&self, cx: &gpui::App) -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("BRISK_TY") {
            // Smokes point it at a missing file to run without ty.
            return Some(PathBuf::from(p)).filter(|p| p.is_file());
        }
        let project = self.interpreter(cx).map(|i| i.path);
        let others = self.env.read(cx).venvs.iter().chain(self.env.read(cx).all.iter().flatten()).map(|i| i.path.clone());
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
        if self.lsp.state != TyState::Unknown {
            return;
        }
        // The project interpreter is known before every Python on the machine has been probed
        // when it is configured or a project venv: ty need not wait for the rest. Without ty
        // next to it or on PATH, the other interpreters are waited for.
        let known = self.env.read(cx).local_known();
        if self.env.read(cx).all.is_none() && !(known && self.ty_tool(cx).is_some()) {
            return;
        }
        let Some(tool) = self.ty_tool(cx) else {
            self.lsp.state = TyState::Missing;
            let msg = "Code insight is off: ty not found. Completion offers words from the file; for types, navigation and quick fixes run `pip install ty` in the project interpreter";
            self.status = msg.into();
            self.show_toast(msg.into(), false, cx);
            return;
        };
        self.lsp.state = TyState::Starting;
        let root = self.root.clone();
        let python = self.interpreter(cx).map(|i| i.path);
        let epoch = self.lsp.epoch;
        let marks = brisk_project::Structure::load(&root);
        let settings = brisk_lsp::ty_settings(&marks.import_roots(&root), &marks.excluded);
        let task = cx.background_executor().spawn(async move { Ty::start(&tool, &root, python.as_deref(), settings) });
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

    /// Tell the file's server (ty, or TypeScript for JS / TS) about its current text when it
    /// changed since it last saw it.
    pub(super) fn ty_sync(&mut self, path: &Path, version: u64, text: &str) -> Option<Arc<Ty>> {
        let (ty, synced) = if is_ts_path(path) { (self.lsp.ts.server.clone()?, &mut self.lsp.ts.synced) } else { (self.lsp.ty.clone()?, &mut self.lsp.synced) };
        match synced.get(path) {
            Some(&v) if v == version => {}
            Some(_) => ty.did_change(path, version, text),
            None => ty.did_open(path, version, text),
        }
        synced.insert(path.to_path_buf(), version);
        Some(ty)
    }

    /// Start the server a file needs.
    fn ensure_server(&mut self, path: &Path, cx: &mut Context<Self>) {
        if is_ts_path(path) {
            self.ensure_ts(cx);
        } else {
            self.ensure_ty(cx);
        }
    }

    /// The running server for a file.
    pub(super) fn server_for(&self, path: &Path) -> Option<Arc<Ty>> {
        if is_ts_path(path) {
            self.lsp.ts.server.clone()
        } else {
            self.lsp.ty.clone()
        }
    }

    /// ty for a request at the editor's caret, with the text synced first.
    pub(super) fn ty_at_caret(&mut self, ed: &Entity<Editor>, cx: &mut Context<Self>) -> Option<(Arc<Ty>, PathBuf, Pos)> {
        let path = ed.read(cx).path.clone()?;
        let ts = is_ts_path(&path);
        self.lsp.ts.last = ts;
        if !ts && !ed.read(cx).is_python() {
            return None;
        }
        self.ensure_server(&path, cx);
        let e = ed.read(cx);
        let version = e.buffer.version();
        let text = e.text_string();
        let (line, col) = e.caret_line_col();
        let ty = self.ty_sync(&path, version, &text)?;
        Some((ty, path, Pos { line, col }))
    }

    /// Part of the as-you-type check: sync the text and pull ty's diagnostics.
    pub(super) fn ty_check(&mut self, ed: Entity<Editor>, path: PathBuf, version: u64, text: &str, cx: &mut Context<Self>) {
        self.ensure_server(&path, cx);
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
                            d.noqa_line = (!is_ts_path(&path)).then_some(d.start.line);
                        }
                        ed.update(cx, |e, cx| {
                            if e.set_diagnostics(DiagSource::Ty, version, diags) {
                                cx.notify();
                            }
                        });
                    }
                    Err(_) => this.ensure_server(&path, cx),
                }
            });
        })
        .detach();
    }

    /// A tab closed: ty can drop the unsaved text and fall back to the file on disk.
    pub(super) fn ty_close(&mut self, path: &Path) {
        let synced = if is_ts_path(path) { &mut self.lsp.ts.synced } else { &mut self.lsp.synced };
        if synced.remove(path).is_some() {
            if let Some(ty) = self.server_for(path) {
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
    /// Shift+F6 on a module name: its file (or package folder), renamed along with the usages.
    pub rename_module: Option<PathBuf>,
    /// Rename also changes the name in comments and strings (PyCharm's checkbox, remembered).
    pub rename_text: bool,
    /// A rename waiting in the Find window's Refactoring Preview: edits, old and new name.
    pub pending: Option<(Vec<brisk_lsp::FileEdits>, String, String)>,
    /// The last refactoring, undone at once from any of its files: its name, then each file
    /// with its text before and after and whether it was open in an editor.
    pub last_refactor: Option<(String, Vec<RefactoredFile>)>,
}

/// A file a refactoring changed: its path, text before and after, and whether it was open.
type RefactoredFile = (PathBuf, String, String, bool);

const NAV_LIMIT: usize = 100;

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Workspace {
    pub(super) fn no_ty(&mut self, what: &str) {
        let ts = self.lsp.ts.last;
        if ts {
            self.status = match &self.lsp.ts.state {
                TyState::Missing => format!("{what} needs TypeScript 7: add it to the project or download it when Brisk offers"),
                TyState::Failed(e) => format!("{what}: {e}"),
                _ => format!("{what}: TypeScript is starting…"),
            };
            return;
        }
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
        self.focused_editor(window, cx)
            .or_else(|| self.active_editor().cloned())
            .filter(|e| e.read(cx).is_python() || e.read(cx).path.as_deref().is_some_and(is_ts_path))
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
            A::GotoDeclaration
            | A::GotoTypeDeclaration
            | A::QuickDefinition
            | A::QuickDocumentation
            | A::TypeInfo
            | A::ParameterInfo
            | A::CodeCompletion
            | A::SmartCompletion
            | A::FindUsages
            | A::ShowUsages
            | A::Rename => {}
            _ => return false,
        }
        // Templates, URL names and static files of a web project, also in templates.
        if action == A::GotoDeclaration {
            if let Some(ed) = self.focused_editor(window, cx).or_else(|| self.active_editor().cloned()) {
                if self.js_goto(&ed, None, window, cx) || self.web_goto(&ed, None, window, cx) {
                    return true;
                }
            }
        }
        if matches!(action, A::CodeCompletion | A::SmartCompletion) {
            if let Some(ed) = self.focused_editor(window, cx) {
                if self.js_path_completion(&ed, cx) || self.css_completion(&ed, cx) {
                    return true;
                }
            }
        }
        let Some(ed) = self.editor_for_lsp(window, cx) else {
            self.status = format!("{} works in Python files", action.info().title);
            return true;
        };
        match action {
            A::GotoDeclaration => self.goto_declaration(ed, None, false, window, cx),
            A::GotoTypeDeclaration => self.goto_declaration(ed, None, true, window, cx),
            A::QuickDocumentation => self.quick_doc(ed, None, cx),
            A::TypeInfo => self.type_info(ed, cx),
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
                    // A path in an import, src, href or url(): its folder's entries.
                    let web = ed.read(cx).path.as_deref().is_some_and(super::js_ui::is_web_file);
                    if web && matches!(c, '/' | '"' | '\'' | '`' | '(') && !ed.read(cx).completion_open() {
                        self.js_path_completion(&ed, cx);
                    }
                    // CSS properties and values as you type.
                    if (c.is_ascii_alphabetic() || matches!(c, '-' | ':' | ' ')) && !ed.read(cx).completion_open() && ed.read(cx).is_stylesheet() {
                        let after_colon = {
                            let e = ed.read(cx);
                            let (line, col) = e.caret_line_col();
                            let before: String = e.buffer.line(line).chars().take(col).collect();
                            before.trim_end().ends_with(':')
                        };
                        if c.is_ascii_alphabetic() || c == '-' || after_colon {
                            self.css_completion(&ed, cx);
                        }
                    }
                    if !ed.read(cx).path.as_deref().is_some_and(is_ts_path) || ed.read(cx).completion_open() {
                        return;
                    }
                }
                let ts = ed.read(cx).path.as_deref().is_some_and(is_ts_path);
                let ready = if ts { self.lsp.ts.state == TyState::Ready } else { self.lsp.state == TyState::Ready };
                let (open, sig_open, word, requery) = {
                    let e = ed.read(cx);
                    let (line, col) = e.caret_line_col();
                    let prefix: String = e.buffer.line(line).chars().take(col).collect();
                    let start = e.word_start_before_caret();
                    let word: String = e.buffer.rope().slice(start..e.buffer.primary().head).to_string();
                    let in_comment = in_line_comment(&prefix, if ts { "//" } else { "#" });
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
            E::CtrlClick(pos) if self.js_goto(&ed, Some(pos), window, cx) || self.web_goto(&ed, Some(pos), window, cx) => {}
            E::CtrlClick(pos) => self.goto_declaration(ed, Some(pos), false, window, cx),
            E::OpenLocation(ref path, line) => self.open_symbol(path.clone(), line, 0, window, cx),
            E::ResolveCompletion(ref raw) => self.resolve_completion(ed, raw.clone(), cx),
            E::ShowProblems
            | E::ShowContextActions
            | E::ContextMenu(_)
            | E::BreakpointsChanged
            | E::EditBreakpoint(_)
            | E::GutterMenu(_)
            | E::ShowCommit(_)
            | E::ShowVcsDiff
            | E::RunMark(..)
            | E::SetNextStatement(_) => {}
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
        let def = ty.definition(&path, p);
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let (res, def) = futures::join!(fut, def);
            // PyCharm's header link: the module (or file) and line of the definition.
            let source = def.ok().and_then(|d| d.into_iter().next()).map(|l| {
                let name =
                    brisk_lsp::module_name(&root, &l.path).unwrap_or_else(|| l.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                (l.path, l.start.line, format!("{name}:{}", l.start.line + 1))
            });
            let _ = this.update(cx, |this, cx| match res {
                Ok(Some(md)) => {
                    let anchor = match mouse {
                        Some((pos, at)) => crate::editor::Anchor::Mouse(pos, at),
                        None => crate::editor::Anchor::Caret,
                    };
                    ed.update(cx, |e, cx| {
                        e.show_doc(&md, anchor, cx);
                        if let Some((p, line, label)) = source {
                            e.set_doc_source(p, line, label, cx);
                        }
                    });
                }
                Ok(None) if mouse.is_none() => {
                    this.status = "No documentation found".into();
                    cx.notify();
                }
                // Ctrl+Q says why it failed; a hover that fails stays quiet, as a hover with
                // no documentation does.
                Err(e) if mouse.is_none() => {
                    this.status = format!("Quick Documentation: ty: {e}");
                    cx.notify();
                }
                _ => {}
            });
        })
        .detach();
    }

    /// Ctrl+Shift+P Type Info: the type of the name at the caret in a small popup, as PyCharm:
    /// a variable's type, `(a: int) -> str` for a function, `type[C]` for a class.
    fn type_info(&mut self, ed: Entity<Editor>, cx: &mut Context<Self>) {
        let Some((ty, path, p)) = self.ty_at_caret(&ed, cx) else { return self.no_ty("Type Info") };
        let fut = ty.hover(&path, p);
        let lang = if is_ts_path(&path) { "typescript" } else { "python" };
        cx.spawn(async move |this, cx| {
            let t = fut.await.ok().flatten().and_then(|h| super::problems_ui::type_of_hover(&h));
            let _ = this.update(cx, |this, cx| {
                match t {
                    Some(t) => {
                        this.status = format!("Type: {t}");
                        ed.update(cx, |e, cx| e.show_doc(&format!("```{lang}\n{t}\n```"), crate::editor::Anchor::Caret, cx));
                    }
                    None => this.status = "No type information here".into(),
                }
                cx.notify();
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
                let item = |label: String, edits: Vec<_>| PopupItem {
                    label,
                    detail: String::new(),
                    hint: String::new(),
                    target: Target::Intention(Box::new(Intent::Edits(edits))),
                };
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

    /// Shift+F6, as in PyCharm: a local variable or parameter is renamed in place; another
    /// symbol through the Rename popup (with "Search in comments and strings" and a preview);
    /// a module name also renames its file.
    fn start_rename(&mut self, ed: Entity<Editor>, window: &mut Window, cx: &mut Context<Self>) {
        if ed.update(cx, |e, _| e.name_refs_at_caret()).is_some_and(|r| r.local) {
            return self.local_rename(ed, cx);
        }
        let Some((ty, path, p)) = self.ty_at_caret(&ed, cx) else { return self.local_rename(ed, cx) };
        let line: Vec<char> = ed.read(cx).buffer.line(p.line).chars().collect();
        let (mut a, mut z) = (p.col.min(line.len()), p.col.min(line.len()));
        while a > 0 && is_ident(line[a - 1]) {
            a -= 1;
        }
        while z < line.len() && is_ident(line[z]) {
            z += 1;
        }
        let word: String = line[a..z].iter().collect();
        let (prep, def) = (ty.prepare_rename(&path, p), ty.definition(&path, p));
        let root = self.root.clone();
        cx.spawn_in(window, async move |this, cx| {
            let (prep, def) = futures::join!(prep, def);
            // A module of the project: the definition is the start of `word.py` or `word/__init__.py`.
            let module = def.ok().into_iter().flatten().find_map(|l| {
                let stem = l.path.file_stem()?.to_str()?;
                let m = if stem == "__init__" { l.path.parent()?.to_path_buf() } else { l.path.clone() };
                let name = m.file_stem()?.to_str()?.to_string();
                (l.start.line == 0 && l.start.col == 0 && name == word && m.starts_with(&root)).then_some(m)
            });
            let _ = this.update_in(cx, |this, _, cx| {
                let old = match (&module, prep) {
                    (Some(_), _) => Ok(word.clone()),
                    (None, Ok(Some((s, e)))) => Ok(ed.read(cx).buffer.line(s.line).chars().skip(s.col).take(e.col.saturating_sub(s.col)).collect::<String>()),
                    (None, Ok(None)) => Err("Cannot rename this element".to_string()),
                    (None, Err(e)) => Err(format!("ty: {e}")),
                };
                match old {
                    Ok(old) => {
                        this.nav.rename_module = module;
                        this.nav.rename = Some((path, p, old.clone()));
                        this.open_popup(super::PopupKind::Rename);
                        if let Some(pp) = this.popup.as_mut() {
                            pp.query = old;
                        }
                        this.refresh_popup();
                    }
                    Err(e) => this.status = e,
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn rename_items(&self, q: &str) -> Vec<super::PopupItem> {
        use super::problems_ui::Intent;
        use super::{PopupItem, Target};
        let Some((_, _, old)) = &self.nav.rename else { return Vec::new() };
        let valid = !q.is_empty() && q.chars().all(is_ident) && !q.starts_with(|c: char| c.is_ascii_digit());
        let item = |label: String, detail: String, hint: &str, target: Target| PopupItem { label, detail, hint: hint.into(), target };
        let mut items = Vec::new();
        if let Some(m) = &self.nav.rename_module {
            if valid && q != old {
                let file = |n: &str| if m.is_dir() { format!("{n}/") } else { format!("{n}.{}", m.extension().and_then(|e| e.to_str()).unwrap_or("py")) };
                items.push(item(
                    format!("Rename module '{old}' and its usages to '{q}'"),
                    format!("{} → {}", file(old), file(q)),
                    "Enter",
                    Target::Rename(q.to_string()),
                ));
            }
            return items;
        }
        if valid && q != old {
            let detail = if self.nav.rename_text { "in all files, comments and strings" } else { "in all files" };
            items.push(item(format!("Rename '{old}' to '{q}'"), detail.into(), "Enter", Target::Rename(q.to_string())));
            items.push(item(
                "Preview…".into(),
                "usages in the Find window, then Do Refactor".into(),
                "",
                Target::Intention(Box::new(Intent::RenamePreview(q.to_string()))),
            ));
        }
        let mark = if self.nav.rename_text { "☑" } else { "☐" };
        items.push(item(format!("{mark} Search in comments and strings"), String::new(), "", Target::Intention(Box::new(Intent::RenameText(q.to_string())))));
        items
    }

    /// The Rename popup's checkbox: flip it and show the popup again with the same name.
    pub(super) fn toggle_rename_text(&mut self, q: String) {
        self.nav.rename_text = !self.nav.rename_text;
        self.open_popup(super::PopupKind::Rename);
        if let Some(pp) = self.popup.as_mut() {
            pp.query = q;
        }
        self.refresh_popup();
    }

    pub(super) fn apply_rename(&mut self, new_name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.rename_with(new_name, false, window, cx);
    }

    /// Rename across the project (open files as one undo step each, the rest on disk), or
    /// with `preview` show the usages first in the Find window.
    pub(super) fn rename_with(&mut self, new_name: String, preview: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_editor(window, cx);
        let Some((path, p, old)) = self.nav.rename.take() else { return };
        if let Some(m) = self.nav.rename_module.take() {
            let target = if m.is_dir() {
                m.with_file_name(&new_name)
            } else {
                m.with_file_name(format!("{new_name}.{}", m.extension().and_then(|e| e.to_str()).unwrap_or("py")))
            };
            if target.exists() {
                self.status = format!("Rename: {} already exists", target.display());
                return;
            }
            self.file_op = Some(super::files_ui::FileOp::Rename(m));
            return self.run_file_op(target, window, cx);
        }
        let Some(ty) = self.server_for(&path) else { return self.no_ty("Rename") };
        let fut = ty.rename(&path, p, &new_name);
        let text = self.nav.rename_text;
        cx.spawn_in(window, async move |this, cx| {
            let res = fut.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match res {
                    Ok(files) => {
                        let mut files = brisk_lsp::merge_file_edits(files);
                        if text {
                            files = this.with_text_occurrences(files, &old, &new_name, cx);
                        }
                        if files.is_empty() {
                            this.status = "Nothing to rename".into();
                        } else if preview {
                            let usages = files
                                .iter()
                                .flat_map(|(f, es)| {
                                    es.iter().filter(|e| e.start.line == e.end.line).map(move |e| (f.clone(), e.start.line, e.start.col..e.end.col))
                                })
                                .collect();
                            this.show_usages(format!("Refactoring Preview: rename '{old}' to '{new_name}'"), usages, window, cx);
                            this.nav.pending = Some((files, old, new_name));
                        } else {
                            this.do_rename(files, &old, &new_name, cx);
                        }
                    }
                    Err(e) => this.status = format!("ty: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Whole-word `old` in comments and strings of the project's Python files, added to `files`.
    fn with_text_occurrences(&self, files: Vec<brisk_lsp::FileEdits>, old: &str, new: &str, cx: &gpui::App) -> Vec<brisk_lsp::FileEdits> {
        let mut groups = files;
        for f in self.project_python_files(&self.root) {
            let open = self.editors().find(|e| e.read(cx).path.as_deref() == Some(f.as_path())).map(|e| e.read(cx).text_string());
            let Some(text) = open.or_else(|| std::fs::read_to_string(&f).ok().map(|t| t.replace("\r\n", "\n"))) else { continue };
            if !text.contains(old) {
                continue;
            }
            let Some(tree) = brisk_syntax::parse_full(brisk_syntax::Lang::Python, &text, None) else { continue };
            let pos = |b: usize| {
                let ls = text[..b].rfind('\n').map_or(0, |n| n + 1);
                Pos { line: text[..ls].matches('\n').count(), col: text[ls..b].chars().count() }
            };
            let edits: Vec<brisk_lint::Edit> = brisk_syntax::text_occurrences(&tree, &text, old)
                .into_iter()
                .map(|r| brisk_lint::Edit { start: pos(r.start), end: pos(r.end), content: new.to_string() })
                .collect();
            if !edits.is_empty() {
                groups.push((f, edits));
            }
        }
        brisk_lsp::merge_file_edits(groups)
    }

    /// The text of a file: its editor's when open, else the disk's.
    fn file_text_now(&self, f: &Path, cx: &gpui::App) -> Option<String> {
        match self.editors().find(|e| e.read(cx).path.as_deref() == Some(f)) {
            Some(e) => Some(e.read(cx).text_string()),
            None => std::fs::read_to_string(f).ok(),
        }
    }

    fn do_rename(&mut self, files: Vec<brisk_lsp::FileEdits>, old: &str, new: &str, cx: &mut Context<Self>) {
        let before: Vec<(PathBuf, Option<String>, bool)> = files
            .iter()
            .map(|(f, _)| (f.clone(), self.file_text_now(f, cx), self.editors().any(|e| e.read(cx).path.as_deref() == Some(f.as_path()))))
            .collect();
        let n = files.len();
        let (count, failed) = self.apply_file_edits(files, cx);
        let undo = before.into_iter().filter_map(|(f, b, open)| Some((f.clone(), b?, self.file_text_now(&f, cx)?, open))).collect();
        self.nav.last_refactor = Some((format!("Rename '{old}' to '{new}'"), undo));
        self.status = format!("Renamed '{old}' to '{new}': {count} occurrence(s) in {n} file(s)");
        if !failed.is_empty() {
            self.show_toast(format!("Rename could not write: {}", failed.join("; ")), true, cx);
        }
        self.refresh_git(cx);
    }

    /// The Refactoring Preview's Do Refactor.
    pub(super) fn do_pending_refactor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((files, old, new)) = self.nav.pending.take() else { return };
        self.find.window = None;
        self.bottom = None;
        self.focus_editor(window, cx);
        self.do_rename(files, &old, &new, cx);
        cx.notify();
    }

    /// Ctrl+Z in a file the last refactoring changed, while every file it changed is still as
    /// it left them: undo it in all of them at once (PyCharm's global undo).
    pub(super) fn undo_refactor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(ed) = self.focused_editor(window, cx).or_else(|| self.active_editor().cloned()) else { return false };
        let Some(p) = ed.read(cx).path.clone() else { return false };
        let Some((_, files)) = &self.nav.last_refactor else { return false };
        if !files.iter().any(|f| f.0 == p) {
            return false;
        }
        if files.iter().any(|(f, _, after, _)| self.file_text_now(f, cx).as_ref() != Some(after)) {
            self.nav.last_refactor = None;
            return false;
        }
        let Some((what, files)) = self.nav.last_refactor.take() else { return false };
        let mut on_disk = Vec::new();
        let n = files.len();
        for (f, before, _, was_open) in files {
            match self.editors().find(|e| e.read(cx).path.as_deref() == Some(f.as_path())).cloned() {
                Some(e) if was_open => e.update(cx, |e, cx| {
                    e.perform(brisk_keymap::Action::Undo, window, cx);
                }),
                Some(e) => e.update(cx, |e, cx| {
                    let len = e.text_string().len();
                    e.apply_byte_edits(vec![(0..len, before)], cx);
                }),
                None => {
                    if std::fs::write(&f, before).is_ok() {
                        on_disk.push(f);
                    }
                }
            }
        }
        if let (Some(ty), false) = (&self.lsp.ty, on_disk.is_empty()) {
            ty.files_changed(&on_disk);
        }
        self.status = format!("Undo {what}: {n} file(s)");
        self.refresh_git(cx);
        cx.notify();
        true
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
                Some(ed) => brisk_history::record(&file, &ed.read(cx).buffer.text(), brisk_history::Label::BeforeRefactor),
                None => brisk_history::record_path(&file, brisk_history::Label::BeforeRefactor),
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
    /// The project file list was rescanned: tell ty which Python files appeared or went away
    /// (worked out off the UI thread).
    pub(super) fn ty_files_rescanned(&mut self, created: &[PathBuf], deleted: &[PathBuf]) {
        let Some(ty) = self.lsp.ty.as_ref() else { return };
        ty.files_created_deleted(created, deleted);
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
                e.is_python().then_some((e.path.clone()?, e.buffer.version(), e.text_string()))
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
    /// Python files under `dir` from the project index (it honours `.gitignore` and Excluded
    /// folders); a walk of the disk until the first scan is done.
    pub(super) fn project_python_files(&self, dir: &Path) -> Vec<PathBuf> {
        let Some(list) = &self.files else { return python_files_under(dir) };
        list.files.iter().filter(|f| f.ends_with(".py") || f.ends_with(".pyi")).map(|f| list.root.join(f)).filter(|p| p.starts_with(dir)).collect()
    }

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

/// `.py` / `.pyi` files under `dir`, read from the disk: hidden folders (`.git`), caches
/// and environments (any folder with `pyvenv.cfg`, `site-packages`) are skipped, so a
/// refactoring never rewrites installed packages. For files the index already knows, use
/// [`Workspace::project_python_files`].
pub(super) fn python_files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            match e.file_type() {
                Ok(t) if t.is_dir() => {
                    let skip = name.starts_with('.')
                        || matches!(name.as_str(), "__pycache__" | "node_modules" | "site-packages" | "dist-packages")
                        || p.join("pyvenv.cfg").is_file();
                    if !skip {
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
    #[cfg(feature = "smoke")]
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
                        this.lsp.state != TyState::Unknown
                            && this.lsp.state != TyState::Starting
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
            let where_ =
                |this: &Self, cx: &gpui::App| this.here(cx).map(|h| format!("{}:{}:{}", h.path.file_name().unwrap().to_string_lossy(), h.line + 1, h.col + 1));
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

/// Whether `prefix` (a line up to the caret) ends inside a line comment started by
/// `marker`, outside strings (`'http://x'` is not one).
fn in_line_comment(prefix: &str, marker: &str) -> bool {
    let mut quote: Option<char> = None;
    let mut chars = prefix.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match quote {
            Some(_) if c == '\\' => {
                chars.next();
            }
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if matches!(c, '"' | '\'' | '`') => quote = Some(c),
            None if prefix[i..].starts_with(marker) => return true,
            None => {}
        }
    }
    false
}

impl Workspace {
    /// An accepted auto-import item: ask the server for its import line and add it.
    fn resolve_completion(&mut self, ed: Entity<Editor>, raw: serde_json::Value, cx: &mut Context<Self>) {
        let Some(path) = ed.read(cx).path.clone() else { return };
        let Some(server) = self.server_for(&path) else { return };
        let fut = server.resolve_completion(&path, raw);
        cx.spawn(async move |_, cx| {
            let Ok(edits) = fut.await else { return };
            if edits.is_empty() {
                return;
            }
            let _ = ed.update(cx, |e, cx| {
                let fix = brisk_lint::Fix { message: String::new(), safe: true, edits };
                e.apply_fix(&fix, cx);
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod comment_tests {
    #[test]
    fn line_comments_outside_strings() {
        assert!(super::in_line_comment("x = 1 // a", "//"));
        assert!(!super::in_line_comment("const url = 'http://x'; u.", "//"));
        assert!(!super::in_line_comment("s = \"a#b\"; s.", "#"));
        assert!(super::in_line_comment("s = 'a' # c", "#"));
    }
}
