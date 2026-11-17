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
}

impl Lsp {
    pub fn new() -> Self {
        Lsp { ty: None, state: TyState::Unknown, synced: HashMap::new(), epoch: 0 }
    }
}

impl Workspace {
    /// ty next to the project interpreter, else next to any other Python found, else on PATH.
    fn ty_tool(&self) -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("BRISK_TY") {
            return Some(PathBuf::from(p));
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
        let epoch = self.lsp.epoch;
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
    fn no_ty(&mut self, what: &str) {
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
            A::GotoDeclaration | A::GotoTypeDeclaration | A::QuickDocumentation | A::ParameterInfo | A::CodeCompletion | A::SmartCompletion | A::FindUsages | A::ShowUsages | A::Rename => {}
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
            A::ParameterInfo => self.parameter_info(ed, cx),
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
                if !ed.read(cx).is_python() || self.lsp.state != TyState::Ready {
                    return;
                }
                let (open, sig_open, word) = {
                    let e = ed.read(cx);
                    let (line, col) = e.caret_line_col();
                    let prefix: String = e.buffer.line(line).chars().take(col).collect();
                    let start = e.word_start_before_caret();
                    let word: String = e.buffer.rope().slice(start..e.buffer.primary().head).to_string();
                    let in_comment = prefix.contains('#');
                    (e.completion_open() || in_comment, e.signature_open(), word)
                };
                if c == '(' || (c == ',' && sig_open) {
                    self.parameter_info(ed.clone(), cx);
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
            E::HoverDoc(pos, at) => self.quick_doc(ed, Some((pos, at)), cx),
            E::CtrlClick(pos) => self.goto_declaration(ed, Some(pos), false, window, cx),
            E::ShowProblems | E::ContextMenu(_) | E::BreakpointsChanged | E::EditBreakpoint(_) => {}
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
        let Some((ty, path, p)) = self.ty_at_caret(&ed, cx) else { return self.no_ty("Go to Declaration") };
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
    fn quick_doc(&mut self, ed: Entity<Editor>, mouse: Option<(usize, gpui::Point<gpui::Pixels>)>, cx: &mut Context<Self>) {
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

    /// Ctrl+P, and automatically after `(` and `,`.
    fn parameter_info(&mut self, ed: Entity<Editor>, cx: &mut Context<Self>) {
        let Some((ty, path, p)) = self.ty_at_caret(&ed, cx) else { return self.no_ty("Parameter Info") };
        let fut = ty.signature_help(&path, p);
        cx.spawn(async move |_, cx| {
            if let Ok(sig) = fut.await {
                let _ = ed.update(cx, |e, cx| e.show_signature(sig, cx));
            }
        })
        .detach();
    }

    /// Ctrl+Space (`explicit`), or automatically while typing an identifier or after `.`.
    fn complete(&mut self, ed: Entity<Editor>, explicit: bool, cx: &mut Context<Self>) {
        let Some((ty, path, p)) = self.ty_at_caret(&ed, cx) else {
            if explicit {
                self.no_ty("Code completion");
            }
            return;
        };
        let start = ed.read(cx).word_start_before_caret();
        self.nav.completion_gen += 1;
        let my = self.nav.completion_gen;
        let fut = ty.completion(&path, p);
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            let _ = this.update(cx, |this, cx| {
                if this.nav.completion_gen != my {
                    return;
                }
                match res {
                    Ok(items) if !items.is_empty() => ed.update(cx, |e, cx| {
                        // Still typing the same word?
                        if e.word_start_before_caret() == start {
                            e.show_completion(items, start, cx);
                        }
                    }),
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
        let Some((ty, path, p)) = self.ty_at_caret(&ed, cx) else { return self.no_ty("Find Usages") };
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
                        let n = locs.len();
                        let usages: Vec<(PathBuf, usize, std::ops::Range<usize>)> = locs
                            .into_iter()
                            .map(|l| {
                                let end = if l.end.line == l.start.line { l.end.col } else { l.start.col + name.chars().count() };
                                (l.path, l.start.line, l.start.col..end)
                            })
                            .collect();
                        this.show_usages(format!("Usages of '{name}'"), usages, window, cx);
                        this.status = format!("{n} usage(s) of '{name}'");
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
        let Some((ty, path, p)) = self.ty_at_caret(&ed, cx) else { return self.no_ty("Rename") };
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
                        let (count, failed) = this.apply_file_edits(files, cx);
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
