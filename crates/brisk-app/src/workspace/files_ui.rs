//! File operations in the Project tool window, as in PyCharm: New File / Python File / Directory
//! (Alt+Insert), Rename (Shift+F6), Copy (F5), Move (F6), Delete (to the Recycle Bin), plus the
//! tree's context menu. Open tabs follow renames and moves; deleted files' tabs close unsaved.

use super::menu_ui::Entry;
use super::{PopupItem, PopupKind, Tab, Target, Workspace};
use brisk_project::{FileList, NodeId};
use gpui::{Context, Keystroke, Pixels, Point, Window};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NewKind {
    File,
    Python,
    /// A folder with an empty `__init__.py`.
    Package,
    Dir,
}

#[derive(Clone, Debug)]
pub(super) enum FileOp {
    /// Create in this folder.
    New(PathBuf, NewKind),
    Rename(PathBuf),
    Copy(PathBuf),
    Move(PathBuf),
    Delete(PathBuf),
}

impl FileOp {
    pub(super) fn title(&self) -> &'static str {
        match self {
            FileOp::New(_, NewKind::File) => "New File",
            FileOp::New(_, NewKind::Python) => "New Python File",
            FileOp::New(_, NewKind::Package) => "New Python Package",
            FileOp::New(_, NewKind::Dir) => "New Directory",
            FileOp::Rename(p) if p.is_dir() => "Rename Directory",
            FileOp::Rename(_) => "Rename File",
            FileOp::Copy(_) => "Copy",
            FileOp::Move(_) => "Move",
            FileOp::Delete(_) => "Delete",
        }
    }

    pub(super) fn placeholder(&self) -> &'static str {
        match self {
            FileOp::New(_, NewKind::Dir) => "Name (a/b creates nested directories)",
            FileOp::New(_, NewKind::Package) => "Name (a.b or a/b creates nested packages)",
            FileOp::New(..) => "Name (dir/name.ext creates folders too)",
            FileOp::Rename(_) => "New name",
            FileOp::Copy(_) | FileOp::Move(_) => "Target path, relative to the project",
            FileOp::Delete(_) => "",
        }
    }
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Move to the Recycle Bin (Windows) so a delete can be undone from Explorer.
#[cfg(windows)]
fn to_trash(p: &Path) -> std::io::Result<()> {
    #[repr(C)]
    struct ShFileOp {
        hwnd: isize,
        func: u32,
        from: *const u16,
        to: *const u16,
        flags: u16,
        aborted: i32,
        mappings: *mut std::ffi::c_void,
        title: *const u16,
    }
    #[link(name = "shell32")]
    extern "system" {
        fn SHFileOperationW(op: *mut ShFileOp) -> i32;
    }
    use std::os::windows::ffi::OsStrExt;
    let mut from: Vec<u16> = p.as_os_str().encode_wide().collect();
    from.extend([0, 0]); // double-NUL terminated list
    const FO_DELETE: u32 = 3;
    const FOF_SILENT: u16 = 0x4;
    const FOF_NOCONFIRMATION: u16 = 0x10;
    const FOF_ALLOWUNDO: u16 = 0x40;
    const FOF_NOERRORUI: u16 = 0x400;
    let mut op = ShFileOp {
        hwnd: 0,
        func: FO_DELETE,
        from: from.as_ptr(),
        to: std::ptr::null(),
        flags: FOF_SILENT | FOF_NOCONFIRMATION | FOF_ALLOWUNDO | FOF_NOERRORUI,
        aborted: 0,
        mappings: std::ptr::null_mut(),
        title: std::ptr::null(),
    };
    let rc = unsafe { SHFileOperationW(&mut op) };
    if rc != 0 || op.aborted != 0 || p.exists() {
        return Err(std::io::Error::other(format!("could not move to the Recycle Bin (code {rc})")));
    }
    Ok(())
}

#[cfg(not(windows))]
fn to_trash(p: &Path) -> std::io::Result<()> {
    if p.is_dir() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    }
}

fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            copy_recursive(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(d) = to.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::copy(from, to).map(|_| ())
    }
}

fn under(p: &Path, base: &Path) -> bool {
    p == base || p.starts_with(base)
}

impl Workspace {
    fn selected_path(&self) -> Option<(NodeId, PathBuf, bool)> {
        let id = self.tree_selected?;
        let n = self.tree.nodes.get(id)?;
        Some((id, n.path.clone(), n.is_dir))
    }

    /// The folder new files go to: the selected folder, or the selected file's folder.
    fn target_dir(&self) -> PathBuf {
        match self.selected_path() {
            Some((_, p, true)) => p,
            Some((_, p, false)) => p.parent().map(Path::to_path_buf).unwrap_or_else(|| self.root.clone()),
            None => self.root.clone(),
        }
    }

    pub(super) fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.root).unwrap_or(p).to_string_lossy().replace('\\', "/")
    }

    pub(super) fn start_file_op(&mut self, op: FileOp, cx: &mut Context<Self>) {
        let prefill = match &op {
            FileOp::Rename(p) => name_of(p),
            FileOp::Copy(p) | FileOp::Move(p) => self.rel(p),
            _ => String::new(),
        };
        let delete = matches!(op, FileOp::Delete(_));
        self.file_op = Some(op);
        self.open_popup(if delete { PopupKind::FileDelete } else { PopupKind::FileName });
        if let Some(p) = self.popup.as_mut() {
            p.query = prefill;
        }
        self.refresh_popup();
        cx.notify();
    }

    /// Where the operation would put things for the typed name, or why it cannot.
    fn file_op_target(&self, q: &str) -> Result<PathBuf, String> {
        let q = q.trim().trim_matches(|c| c == '/' || c == '\\');
        let op = self.file_op.as_ref().ok_or("nothing to do")?;
        if q.is_empty() {
            return Err(String::new());
        }
        if q.contains(['<', '>', ':', '"', '|', '?', '*']) {
            return Err("A name cannot contain < > : \" | ? *".into());
        }
        let t = match op {
            FileOp::New(dir, NewKind::Package) => dir.join(q.replace('.', "/")),
            FileOp::New(dir, kind) => {
                let mut t = dir.join(q);
                if *kind == NewKind::Python && t.extension().is_none() {
                    t.set_extension("py");
                }
                t
            }
            FileOp::Rename(p) => {
                if q.contains(['/', '\\']) {
                    return Err("Use Move (F6) to move to another folder".into());
                }
                p.with_file_name(q)
            }
            FileOp::Copy(_) | FileOp::Move(_) => {
                let t = self.root.join(q);
                let src = match op {
                    FileOp::Copy(p) | FileOp::Move(p) => p,
                    _ => unreachable!(),
                };
                // An existing folder means "into that folder".
                if t.is_dir() && !under(&t, src) { t.join(name_of(src)) } else { t }
            }
            FileOp::Delete(p) => p.clone(),
        };
        // Renaming to the same name: nothing to say yet.
        if let FileOp::Rename(src) = op {
            if *src == t {
                return Err(String::new());
            }
        }
        if !matches!(op, FileOp::Delete(_)) && t.exists() {
            return Err(format!("{} already exists", self.rel(&t)));
        }
        if let FileOp::Move(src) | FileOp::Copy(src) = op {
            if src.is_dir() && under(&t, src) {
                return Err("Cannot put a folder inside itself".into());
            }
        }
        Ok(t)
    }

    pub(super) fn file_op_items(&self, q: &str) -> Vec<PopupItem> {
        let Some(op) = self.file_op.as_ref() else { return Vec::new() };
        if let FileOp::Delete(p) = op {
            let what = if p.is_dir() { "directory" } else { "file" };
            return vec![
                PopupItem { label: format!("Delete {what} '{}'", name_of(p)), detail: "to the Recycle Bin".into(), hint: String::new(), target: Target::FileDo(p.clone()) },
                PopupItem { label: "Cancel".into(), detail: String::new(), hint: String::new(), target: Target::Cancel },
            ];
        }
        match self.file_op_target(q) {
            Ok(t) => {
                let verb = match op {
                    FileOp::New(_, NewKind::Dir) => "Create directory",
                    FileOp::New(_, NewKind::Package) => "Create package",
                    FileOp::New(..) => "Create",
                    FileOp::Rename(_) => "Rename to",
                    FileOp::Copy(_) => "Copy to",
                    FileOp::Move(_) => "Move to",
                    FileOp::Delete(_) => "Delete",
                };
                vec![PopupItem { label: format!("{verb} {}", self.rel(&t)), detail: String::new(), hint: "Enter".into(), target: Target::FileDo(t) }]
            }
            Err(e) if e.is_empty() => Vec::new(),
            Err(e) => vec![PopupItem { label: format!("✗ {e}"), detail: String::new(), hint: String::new(), target: Target::Cancel }],
        }
    }

    /// Enter in the name / confirmation popup.
    pub(super) fn run_file_op(&mut self, target: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // Moving a Python module: first ask ty which imports change (comes back here).
        if let Some(FileOp::Rename(src) | FileOp::Move(src)) = self.file_op.clone() {
            if self.will_move_python(&src, &target, window, cx) {
                return;
            }
        }
        let Some(op) = self.file_op.take() else { return };
        let result: std::io::Result<()> = (|| {
            match &op {
                FileOp::New(_, NewKind::Dir) => std::fs::create_dir_all(&target),
                FileOp::New(base, NewKind::Package) => {
                    // Every new folder on the way is a package too (a.b → a/__init__.py, a/b/__init__.py).
                    std::fs::create_dir_all(&target)?;
                    let mut d = target.as_path();
                    while d != base.as_path() && d.starts_with(base) {
                        let init = d.join("__init__.py");
                        if !init.exists() {
                            std::fs::write(&init, "")?;
                        }
                        let Some(up) = d.parent() else { break };
                        d = up;
                    }
                    Ok(())
                }
                FileOp::New(_, kind) => {
                    if let Some(d) = target.parent() {
                        std::fs::create_dir_all(d)?;
                    }
                    use std::io::Write;
                    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&target)?;
                    let text = if *kind == NewKind::Python { self.python_template(&target) } else { String::new() };
                    f.write_all(text.as_bytes())
                }
                FileOp::Rename(src) | FileOp::Move(src) => {
                    if let Some(d) = target.parent() {
                        std::fs::create_dir_all(d)?;
                    }
                    self.save_all(cx);
                    std::fs::rename(src, &target)
                }
                FileOp::Copy(src) => {
                    self.save_all(cx);
                    copy_recursive(src, &target)
                }
                FileOp::Delete(p) => {
                    // Local History keeps what is deleted (unsaved editor text too).
                    crate::local_history::record_path(p, crate::local_history::Label::Deleted);
                    for ed in self.editors() {
                        let e = ed.read(cx);
                        if let Some(f) = e.path.as_deref().filter(|f| f.starts_with(p.as_path()) && e.buffer.is_dirty()) {
                            crate::local_history::record(f, &e.buffer.text(), crate::local_history::Label::Deleted);
                        }
                    }
                    to_trash(p)
                }
            }
        })();
        if let Err(e) = result {
            self.lsp.moved = None;
            self.status = format!("{} failed: {e}", op.title());
            cx.notify();
            return;
        }
        // Tabs follow renames/moves; tabs of deleted files close without saving.
        match &op {
            FileOp::Rename(src) | FileOp::Move(src) => self.retarget_tabs(src, &target, cx),
            FileOp::Delete(p) => self.close_tabs_under(p, window, cx),
            _ => {}
        }
        // Refresh the folders that changed and select the result.
        let mut dirs: Vec<PathBuf> = vec![target.parent().map(Path::to_path_buf).unwrap_or_else(|| self.root.clone())];
        if let FileOp::Rename(src) | FileOp::Move(src) | FileOp::Delete(src) = &op {
            dirs.push(src.parent().map(Path::to_path_buf).unwrap_or_else(|| self.root.clone()));
        }
        for d in dirs {
            // The nearest loaded ancestor (new nested folders are not loaded yet).
            let mut cur = Some(d.as_path());
            while let Some(c) = cur {
                if let Some(id) = self.tree.find(c) {
                    self.tree.reload(id);
                    break;
                }
                cur = c.parent();
            }
        }
        if !matches!(op, FileOp::Delete(_)) {
            if let Some(id) = self.tree.reveal(&target) {
                self.tree_selected = Some(id);
                if let Some(ix) = self.tree.rows().iter().position(|r| r.id == id) {
                    self.tree_scroll.scroll_to_item(ix, gpui::ScrollStrategy::Center);
                }
            }
        } else {
            self.tree_selected = target.parent().and_then(|p| self.tree.find(p));
        }
        self.status = match &op {
            FileOp::Delete(p) => format!("Moved {} to the Recycle Bin", self.rel(p)),
            _ => format!("{}: {}", op.title(), self.rel(&target)),
        };
        if let FileOp::Rename(src) | FileOp::Move(src) = &op {
            self.after_python_move(src, &target, cx);
        }
        self.reindex_files(cx);
        self.refresh_git(cx);
        // A new or copied file opens in the editor, like PyCharm.
        if matches!(op, FileOp::New(_, NewKind::File | NewKind::Python) | FileOp::Copy(_)) && target.is_file() {
            self.open_file(&target, window, cx);
        } else {
            window.focus(&self.tree_focus);
        }
        cx.notify();
    }

    fn retarget_tabs(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        let mut moved = Vec::new();
        for t in self.all_tabs() {
            if let Tab::Editor(e) = t {
                let Some(p) = e.read(cx).path.clone() else { continue };
                if under(&p, from) {
                    let np = to.join(p.strip_prefix(from).unwrap_or(Path::new("")));
                    let np = if p == from { to.to_path_buf() } else { np };
                    e.update(cx, |e, cx| {
                        e.path = Some(np.clone());
                        cx.notify();
                    });
                    moved.push(p);
                }
            }
        }
        for p in moved {
            self.ty_close(&p);
        }
        for p in self.recent.iter_mut() {
            if under(p, from) {
                *p = if *p == from { to.to_path_buf() } else { to.join(p.strip_prefix(from).unwrap_or(Path::new(""))) };
            }
        }
    }

    fn close_tabs_under(&mut self, base: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let doomed: Vec<_> = self
            .editors()
            .filter(|e| e.read(cx).path.as_deref().is_some_and(|p| under(p, base)))
            .cloned()
            .collect();
        for e in doomed {
            // Nothing to save: the file is gone.
            e.update(cx, |e, _| e.buffer.mark_saved());
            self.close_editor(&e, window, cx);
        }
        self.recent.retain(|p| !under(p, base));
    }

    /// Rebuild the file list (Go to File, Find in Files, symbols) after files changed.
    pub(super) fn reindex_files(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let task = cx.background_executor().spawn(async move { FileList::scan(&root) });
        cx.spawn(async move |this, cx| {
            let list = task.await;
            let _ = this.update(cx, |this, cx| {
                crate::stall::note("reindex: apply file list");
                this.ty_files_rescanned(this.files.clone(), &list);
                this.files = Some(Arc::new(list));
                this.symbols.stale = true;
                this.ensure_symbols(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Keys in the Project view for file operations. True if handled.
    pub(super) fn tree_file_key(&mut self, k: &Keystroke, cx: &mut Context<Self>) -> bool {
        let m = &k.modifiers;
        let Some((id, path, _)) = self.selected_path() else { return false };
        let root = self.tree.is_root(id);
        let op = match (k.key.as_str(), m.control, m.shift, m.alt) {
            ("insert", false, false, true) => {
                self.tree_new_menu(cx);
                return true;
            }
            ("f6", false, true, false) if !root => FileOp::Rename(path),
            ("f5", false, false, false) if !root => FileOp::Copy(path),
            ("f6", false, false, false) if !root => FileOp::Move(path),
            ("delete", false, false, false) if !root => FileOp::Delete(path),
            _ => return false,
        };
        self.start_file_op(op, cx);
        true
    }

    /// Alt+Insert in the tree: the "New" choices as a small menu under the selected row.
    fn tree_new_menu(&mut self, cx: &mut Context<Self>) {
        let dir = self.target_dir();
        let pos = gpui::point(gpui::px(60.), gpui::px(120.));
        let entries = self.new_entries(dir);
        self.open_menu(pos, entries, cx);
    }

    fn new_entries(&self, dir: PathBuf) -> Vec<Entry> {
        let item = |label: &str, kind: NewKind, shortcut: String| {
            let dir = dir.clone();
            Entry::Item {
                label: label.into(),
                shortcut,
                enabled: true,
                run: Rc::new(move |this: &mut Workspace, _, cx| this.start_file_op(FileOp::New(dir.clone(), kind), cx)),
            }
        };
        // PyCharm's New menu order: File, Scratch File, Directory, Python Package, Python File.
        let scratch = Entry::Item {
            label: "Scratch File".into(),
            shortcut: "Ctrl+Alt+Shift+Insert".into(),
            enabled: true,
            run: Rc::new(|this: &mut Workspace, _, cx| this.start_new_scratch(cx)),
        };
        vec![
            item("New File…", NewKind::File, "Alt+Insert".into()),
            scratch,
            item("New Directory…", NewKind::Dir, String::new()),
            item("New Python Package…", NewKind::Package, String::new()),
            item("New Python File…", NewKind::Python, String::new()),
        ]
    }

    /// Right click on a row of the Project view.
    pub(super) fn tree_menu(&mut self, id: NodeId, pos: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.tree_selected = Some(id);
        window.focus(&self.tree_focus);
        let Some((_, path, is_dir)) = self.selected_path() else { return };
        let root = self.tree.is_root(id);
        let op = |label: &str, shortcut: &str, enabled: bool, op: FileOp| Entry::Item {
            label: label.into(),
            shortcut: shortcut.into(),
            enabled,
            run: Rc::new(move |this: &mut Workspace, _, cx| this.start_file_op(op.clone(), cx)),
        };
        let mut v = self.new_entries(self.target_dir());
        v.push(Entry::Separator);
        v.push(op("Rename…", "Shift+F6", !root, FileOp::Rename(path.clone())));
        v.push(op("Copy…", "F5", !root, FileOp::Copy(path.clone())));
        v.push(op("Move…", "F6", !root, FileOp::Move(path.clone())));
        v.push(op("Delete…", "Delete", !root, FileOp::Delete(path.clone())));
        v.push(Entry::Separator);
        if !is_dir {
            let p = path.clone();
            v.push(Entry::Item {
                label: "Open".into(),
                shortcut: "F4".into(),
                enabled: true,
                run: Rc::new(move |this: &mut Workspace, window, cx| this.open_file(&p, window, cx)),
            });
        }
        v.extend(self.git_menu_entries(&path, None, cx));
        v.push(Entry::Submenu {
            label: "Local History".into(),
            entries: vec![
                Entry::Item {
                    label: "Show History".into(),
                    shortcut: String::new(),
                    enabled: true,
                    run: Rc::new({
                        let p = path.clone();
                        move |this: &mut Workspace, _, cx| this.show_path_history(p.clone(), cx)
                    }),
                },
                Entry::Item { label: "Put Label…".into(), shortcut: String::new(), enabled: true, run: Rc::new(|this: &mut Workspace, _, _| this.start_put_label()) },
            ],
        });
        v.push(self.path_entries(path.clone()));
        v.push(Entry::Item {
            label: "Show in Explorer".into(),
            shortcut: String::new(),
            enabled: true,
            run: Rc::new({
                let p = path.clone();
                move |_, _, cx| cx.reveal_path(&p)
            }),
        });
        v.push(Entry::Separator);
        v.push(Entry::Item {
            label: "Reload from Disk".into(),
            shortcut: String::new(),
            enabled: true,
            run: Rc::new(move |this: &mut Workspace, _, cx| {
                let node = if is_dir { Some(id) } else { this.tree.nodes[id].parent };
                if let Some(n) = node {
                    this.tree.reload(n);
                }
                this.reindex_files(cx);
                cx.notify();
            }),
        });
        self.open_menu(pos, v, cx);
    }
}
