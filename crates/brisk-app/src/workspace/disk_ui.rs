//! Keeping the window in step with the disk: open files changed by other programs reload (or
//! show a conflict banner when they have unsaved edits), deleted files close their tabs, and
//! expanded Project view folders pick up added and removed files. Polling, about every 1.5 s:
//! one metadata call per open file and per expanded folder, made off the UI thread together
//! with the re-read of the folders that changed (an `npm install` or a `git checkout` can put
//! tens of thousands of files in one folder); the UI thread only applies the result.

use super::Workspace;
use crate::editor::{DiskCheck, Editor};
use gpui::{Context, Entity, Window};
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

fn file_stamp(p: &std::path::Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// What the UI thread hands the background poll: open files and expanded folders.
struct Snapshot {
    editors: Vec<(Entity<Editor>, PathBuf)>,
    dirs: Vec<PathBuf>,
    stamps: HashMap<PathBuf, Option<SystemTime>>,
    /// Re-read every expanded folder whether its time changed or not (Reload All from Disk:
    /// some file systems and tools leave the folder's time alone).
    all_dirs: bool,
}

/// What the background poll hands back.
struct Polled {
    editors: Vec<(Entity<Editor>, Option<(SystemTime, u64)>)>,
    stamps: HashMap<PathBuf, Option<SystemTime>>,
    /// Changed folders with their fresh listings, shallowest first.
    changed: Vec<(PathBuf, Vec<brisk_project::Entry>)>,
}

impl Snapshot {
    fn poll(self) -> Polled {
        let editors = self.editors.into_iter().map(|(e, p)| (e, file_stamp(&p))).collect();
        let mut stamps = HashMap::with_capacity(self.dirs.len());
        let mut changed: Vec<PathBuf> = Vec::new();
        for p in &self.dirs {
            let now = dir_mtime(p);
            if self.all_dirs || matches!(self.stamps.get(p), Some(old) if *old != now) {
                changed.push(p.clone());
            }
            stamps.insert(p.clone(), now);
        }
        changed.sort_by_key(|p| p.components().count());
        let changed = changed.into_iter().map(|d| { let e = brisk_project::read_entries(&d); (d, e) }).collect();
        Polled { editors, stamps, changed }
    }
}

impl Workspace {
    pub(super) fn start_disk_poll(&self, window: &mut Window, cx: &mut Context<Self>) {
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| loop {
            bg.timer(POLL).await;
            let Ok(snap) = this.update(cx, |this, cx| this.disk_snapshot(cx)) else { break };
            let polled = bg.spawn(async move { snap.poll() }).await;
            if this.update_in(cx, |this, window, cx| this.apply_disk_poll(polled, window, cx)).is_err() {
                break;
            }
        })
        .detach();
    }

    fn disk_snapshot(&mut self, cx: &mut Context<Self>) -> Snapshot {
        let editors = self.editors().filter_map(|e| Some((e.clone(), e.read(cx).path.clone()?))).collect();
        let dirs = self.tree.expanded_dirs().into_iter().map(|i| self.tree.nodes[i].path.clone()).collect();
        Snapshot { editors, dirs, stamps: self.dir_stamps.0.clone(), all_dirs: false }
    }

    /// Reload All from Disk (Ctrl+Alt+Y): the disk poll now, re-reading every expanded folder.
    /// Unsaved edits are kept; a file changed under them gets the conflict banner.
    pub(super) fn reload_all_from_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let snap = Snapshot { all_dirs: true, ..self.disk_snapshot(cx) };
        self.status = "Reloading from disk…".into();
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let polled = bg.spawn(async move { snap.poll() }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.apply_disk_poll(polled, window, cx);
                this.status = "Reloaded all files from disk".into();
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_disk_poll(&mut self, polled: Polled, window: &mut Window, cx: &mut Context<Self>) {
        crate::stall::note("disk poll: apply");
        // Open files.
        let mut gone = Vec::new();
        let mut reloaded = false;
        for (e, now) in &polled.editors {
            let r = e.update(cx, |e, cx| {
                let r = e.disk_check_at(*now);
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
            let name = e.read(cx).title();
            self.close_editor(&e, window, cx);
            self.status = format!("{name} was deleted on disk; its tab was closed");
        }
        if reloaded {
            self.symbols.stale = true;
        }

        // Expanded folders in the Project view, from the listings read in the background (a
        // folder expanded or collapsed meanwhile is read here or skipped).
        self.dir_stamps.0 = polled.stamps;
        let changed = !polled.changed.is_empty();
        if changed {
            for (p, entries) in polled.changed {
                if let Some(id) = self.tree.find(&p) {
                    self.tree.set_entries(id, entries);
                }
            }
            self.reindex_files(cx);
            self.refresh_git(cx);
        }
        if reloaded || changed {
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

            // A file in the ANSI code page, which non-UTF-8 files are read as: the label is
            // this machine's code page (windows-1251 on Russian Windows, windows-1252 on
            // English), the text one that code page has.
            let legacy = brisk_text::encoding::system_legacy();
            let sample = ["# привет\n", "# café\n"].into_iter().find(|s| !legacy.encode(s).2).unwrap_or("# x\n");
            let w = root.join("disk_legacy.py");
            let (bytes, _, _) = legacy.encode(sample);
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
                    eprintln!("smoke-disk: {} file -> label={label}, bytes kept after edit+save={}", legacy.name(), raw == want);
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
