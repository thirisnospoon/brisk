//! Keeping the window in step with the disk: open files changed by other programs reload (or
//! show a conflict banner when they have unsaved edits), deleted files close their tabs, and
//! expanded Project view folders pick up added and removed files. Polling, about every 1.5 s:
//! one metadata call per open file and per expanded folder.

use super::{Tab, Workspace};
use crate::editor::DiskCheck;
use gpui::{Context, Window};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

pub const POLL: Duration = Duration::from_millis(1500);

/// Last seen modification time of each expanded folder.
#[derive(Default)]
pub struct DirStamps(HashMap<PathBuf, Option<SystemTime>>);

fn dir_mtime(p: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

impl Workspace {
    pub(super) fn start_disk_poll(&self, window: &mut Window, cx: &mut Context<Self>) {
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| loop {
            bg.timer(POLL).await;
            if this.update_in(cx, |this, window, cx| this.poll_disk(window, cx)).is_err() {
                break;
            }
        })
        .detach();
    }

    pub(super) fn poll_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Open files.
        let mut gone = Vec::new();
        let mut reloaded = false;
        for t in &self.tabs {
            let Tab::Editor(e) = t else { continue };
            let r = e.update(cx, |e, cx| {
                let r = e.disk_check();
                if r != DiskCheck::Same {
                    cx.notify();
                }
                r
            });
            match r {
                DiskCheck::Deleted => gone.push(e.clone()),
                DiskCheck::Reloaded => reloaded = true,
                DiskCheck::Conflict | DiskCheck::Same => {}
            }
        }
        for e in gone {
            if let Some(i) = self.tabs.iter().position(|t| matches!(t, Tab::Editor(x) if *x == e)) {
                let name = e.read(cx).title();
                self.close_tab(i, window, cx);
                self.status = format!("{name} was deleted on disk; its tab was closed");
            }
        }
        if reloaded {
            self.symbols.stale = true;
        }

        // Expanded folders in the Project view.
        let watched: Vec<(usize, PathBuf)> = self
            .tree
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.is_dir && n.expanded && n.children.is_some())
            .map(|(i, n)| (i, n.path.clone()))
            .collect();
        let mut changed = Vec::new();
        let mut seen = HashMap::with_capacity(watched.len());
        for (id, p) in watched {
            let now = dir_mtime(&p);
            match self.dir_stamps.0.get(&p) {
                Some(old) if *old != now => changed.push(id),
                _ => {}
            }
            seen.insert(p, now);
        }
        self.dir_stamps.0 = seen;
        if !changed.is_empty() {
            // Reload deepest first: reloading a parent rebuilds its children's ids.
            let mut paths: Vec<PathBuf> = changed.iter().map(|&i| self.tree.nodes[i].path.clone()).collect();
            paths.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
            for p in paths {
                if let Some(id) = self.tree.find(&p) {
                    self.tree.reload(id);
                }
            }
            self.reindex_files(cx);
            self.refresh_git(cx);
        }
        if reloaded || !changed.is_empty() {
            cx.notify();
        }
    }
}

impl Workspace {
    /// Smoke check (BRISK_SMOKE_DISK=1, run on a scratch project): external edits, conflicts,
    /// files appearing in the tree, legacy encodings, binary files.
    pub(super) fn disk_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let a = root.join("disk_a.py");
        let _ = std::fs::write(&a, "a = 1\n");
        self.open_file(&a, window, cx);
        let Some(ed) = self.active_editor().cloned() else { return };
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let wait = |ms| bg.timer(Duration::from_millis(ms));
            let text = |cx: &mut gpui::AsyncWindowContext| ed.read_with(cx, |e, _| e.buffer.text()).unwrap_or_default();
            wait(300).await;
            let _ = std::fs::write(&a, "a = 2\n");
            wait(2000).await;
            eprintln!("smoke-disk: external write, clean editor -> {:?}", text(cx));

            let _ = ed.update(cx, |e, cx| {
                e.buffer.set_caret(0, false);
                e.buffer.insert("# mine\n");
                cx.notify();
            });
            let _ = std::fs::write(&a, "a = 3  # theirs\n");
            wait(2000).await;
            let conflict = ed.read_with(cx, |e, _| e.disk_conflict).ok().flatten();
            let _ = this.update(cx, |this, cx| this.save_all(cx));
            let disk = std::fs::read_to_string(&a).unwrap_or_default();
            eprintln!("smoke-disk: external write, dirty editor -> conflict={conflict:?}, after save_all disk={disk:?}");
            let tabs = this.update_in(cx, |this, window, cx| {
                let i = this.active.unwrap_or(0);
                this.close_tab(i, window, cx);
                this.tabs.len()
            });
            eprintln!("smoke-disk: close tab during the conflict -> tabs={tabs:?} (still open)");
            let _ = ed.update(cx, |e, cx| e.resolve_conflict(false, cx));
            eprintln!("smoke-disk: Keep Mine -> disk={:?}", std::fs::read_to_string(&a).unwrap_or_default());
            let _ = std::fs::write(&a, "a = 4\n");
            wait(2000).await;
            let _ = ed.update(cx, |e, cx| {
                e.buffer.set_caret(0, false);
                e.buffer.insert("# again\n");
                cx.notify();
            });
            let _ = std::fs::write(&a, "a = 5\n");
            wait(2000).await;
            let _ = ed.update(cx, |e, cx| e.resolve_conflict(true, cx));
            eprintln!("smoke-disk: Reload from Disk -> editor={:?} dirty={:?}", text(cx), ed.read_with(cx, |e, _| e.buffer.is_dirty()).ok());

            // A file created by another program shows up in the Project view.
            let _ = std::fs::write(root.join("disk_created.py"), "");
            wait(2000).await;
            let listed = this
                .update(cx, |this, _| this.tree.rows().to_vec().iter().any(|r| this.tree.nodes[r.id].name == "disk_created.py"))
                .unwrap_or(false);
            eprintln!("smoke-disk: created outside -> in tree={listed}");

            // Deleted outside with nothing unsaved: the tab closes.
            let _ = std::fs::remove_file(&a);
            wait(2000).await;
            let open = this.read_with(cx, |this, cx| this.editors().any(|e| e.read(cx).path.as_deref() == Some(a.as_path()))).unwrap_or(true);
            eprintln!("smoke-disk: deleted outside -> tab open={open}");

            // windows-1251 file: kept in its encoding.
            let w = root.join("disk_cp1251.py");
            let (bytes, _, _) = brisk_text::encoding::encoding_rs::WINDOWS_1251.encode("# привет\n");
            let _ = std::fs::write(&w, &bytes);
            let _ = this.update_in(cx, |this, window, cx| {
                this.open_file(&w, window, cx);
                if let Some(e) = this.active_editor().cloned() {
                    let label = e.read(cx).encoding_label();
                    e.update(cx, |e, _| {
                        e.buffer.set_caret(0, false);
                        e.buffer.insert("x = 1\n");
                    });
                    this.save_all(cx);
                    let raw = std::fs::read(&w).unwrap_or_default();
                    let mut want = b"x = 1\n".to_vec();
                    want.extend_from_slice(&bytes);
                    eprintln!("smoke-disk: cp1251 file -> label={label}, bytes kept after edit+save={}", raw == want);
                }
                let b = this.root.join("disk_blob.bin");
                let _ = std::fs::write(&b, [0x89, b'P', b'N', b'G', 0, 0, 1, 2]);
                this.open_file(&b, window, cx);
                let err = this.active_editor().and_then(|e| e.read(cx).read_error.clone());
                eprintln!("smoke-disk: binary file -> {err:?}");
            });
            eprintln!("smoke-disk: done");
            let _ = cx.update(|_, cx| cx.quit());
        })
        .detach();
    }
}
