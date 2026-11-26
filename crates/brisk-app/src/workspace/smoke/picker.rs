//! `BRISK_SMOKE_PICKER=1` (on an empty scratch folder): the folder browser of Open… and New
//! Project…. In `picker/` of the project it makes `alpha` (with `.git`), `beta` and `.hidden`,
//! then checks: the listing (hidden folder left out, `alpha` marked as a project), the places
//! (home and the drives), `↓` / `→` step into a folder and `Backspace` back up with it
//! selected, Ctrl+H shows hidden folders, and New Project creates the typed folder and asks
//! where to open it. With BRISK_TOUR_SYNC=<dir> it also asks for screenshots the way the tour
//! does. Exits with code 1 on the first failed check.

use crate::workspace::{PopupKind, Workspace};
use brisk_keymap::Action;
use gpui::{AsyncWindowContext, Context, KeyDownEvent, WeakEntity, Window};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn wait(cx: &AsyncWindowContext, ms: u64) -> gpui::Task<()> {
    cx.background_executor().timer(Duration::from_millis(ms))
}

fn key(this: &mut Workspace, k: &str, window: &mut Window, cx: &mut Context<Workspace>) {
    let ev = KeyDownEvent { keystroke: gpui::Keystroke::parse(k).unwrap(), is_held: false };
    this.picker_key(&ev, window, cx);
}

/// Types `text` into the browser's focused field, one key at a time.
fn type_text(this: &mut Workspace, text: &str, window: &mut Window, cx: &mut Context<Workspace>) {
    for c in text.chars() {
        let mut stroke = gpui::Keystroke::parse(&c.to_string()).unwrap();
        stroke.key_char = Some(c.to_string());
        this.picker_key(&KeyDownEvent { keystroke: stroke, is_held: false }, window, cx);
    }
}

/// The folder shown and its rows as `name[project]`, once listed.
async fn rows(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext) -> (PathBuf, Vec<String>) {
    let mut last = (PathBuf::new(), Vec::new());
    for _ in 0..100 {
        let got = this
            .update(cx, |this, _| {
                let b = this.folder_picker.as_ref()?;
                b.listing.as_ref()?;
                Some((b.dir.clone(), b.visible().iter().map(|f| if f.project { format!("{}[project]", f.name) } else { f.name.clone() }).collect()))
            })
            .ok()
            .flatten();
        if let Some(g) = got {
            last = g;
            break;
        }
        wait(cx, 20).await;
    }
    last
}

/// With BRISK_TOUR_SYNC: asks the screenshot script for a picture and waits for it.
async fn shot(name: &str, cx: &AsyncWindowContext) {
    let Some(dir) = std::env::var_os("BRISK_TOUR_SYNC").map(std::path::PathBuf::from) else { return };
    wait(cx, 400).await;
    let _ = std::fs::write(dir.join(format!("{name}.ready")), name);
    for _ in 0..150 {
        if dir.join(format!("{name}.done")).exists() {
            return;
        }
        wait(cx, 100).await;
    }
}

fn check(ok: bool, what: &str, got: impl std::fmt::Debug) {
    if ok {
        eprintln!("smoke-picker: ok {what}");
    } else {
        eprintln!("smoke-picker: FAIL {what}: {got:?}");
        std::process::exit(1);
    }
}

fn prepare(root: &Path) {
    let base = root.join("picker");
    for d in ["alpha/.git", "beta", ".hidden"] {
        let _ = std::fs::create_dir_all(base.join(d));
    }
}

impl Workspace {
    pub(in crate::workspace) fn picker_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        prepare(&root);
        let base = root.join("picker");
        cx.spawn_in(window, async move |this, cx| {
            wait(cx, 1000).await;
            // Open…: the folder's subfolders, hidden ones left out.
            let b = base.clone();
            let _ = this.update_in(cx, |this, window, cx| {
                this.dispatch(Action::OpenProject, window, cx);
                this.picker_go(b, None, cx);
            });
            let (_, got) = rows(&this, cx).await;
            check(got == ["alpha[project]", "beta"], "listing", &got);
            let places = this.update(cx, |this, _| this.folder_picker.as_ref().map_or(0, |b| b.place_count())).unwrap_or(0);
            check(places >= 2, "places", places);
            shot("open", cx).await;

            // ↓ selects the first folder, → steps into it, Backspace comes back with it selected.
            let _ = this.update_in(cx, |this, window, cx| {
                key(this, "down", window, cx);
                key(this, "right", window, cx);
            });
            let (dir, _) = rows(&this, cx).await;
            check(dir == base.join("alpha"), "step in", &dir);
            let _ = this.update_in(cx, |this, window, cx| key(this, "backspace", window, cx));
            let (dir, _) = rows(&this, cx).await;
            let target = this.update(cx, |this, _| this.folder_picker.as_ref().map(|b| b.target())).ok().flatten();
            check(dir == base && target == Some(base.join("alpha")), "up selects where it came from", &target);

            // Ctrl+H: hidden folders too.
            let _ = this.update_in(cx, |this, window, cx| key(this, "ctrl-h", window, cx));
            let (_, got) = rows(&this, cx).await;
            check(got.iter().any(|r| r == ".hidden"), "show hidden", &got);

            // New Project…: the typed name becomes a new folder, then This Window / New Window.
            let b = base.clone();
            let _ = this.update_in(cx, |this, window, cx| {
                key(this, "escape", window, cx);
                this.dispatch(Action::NewProject, window, cx);
                this.picker_go(b, None, cx);
            });
            let _ = rows(&this, cx).await;
            let _ = this.update_in(cx, |this, window, cx| type_text(this, "fresh", window, cx));
            shot("new", cx).await;
            let kind = this
                .update_in(cx, |this, window, cx| {
                    key(this, "enter", window, cx);
                    this.popup.as_ref().map(|p| p.kind)
                })
                .ok()
                .flatten();
            check(root.join("picker/fresh").is_dir() && kind == Some(PopupKind::OpenWhere), "created and asks where", kind);
            eprintln!("smoke-picker: PASS");
            std::process::exit(0);
        })
        .detach();
    }
}
