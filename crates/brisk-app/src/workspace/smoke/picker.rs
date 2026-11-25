//! `BRISK_SMOKE_PICKER=1` (on an empty scratch folder): the folder picker of Open… and New
//! Project…. In `picker/` of the project it makes `alpha` (with `.git`), `beta` and `.hidden`,
//! then checks: the listing (hidden folder left out, `alpha` marked as a project), typing a
//! name selects the matching folder, Tab completes it, and New Project creates the typed
//! folder and asks where to open it. With BRISK_TOUR_SYNC=<dir> it also asks for screenshots
//! the way the tour does. Exits with code 1 on the first failed check.

use crate::workspace::{PopupKind, Workspace};
use brisk_keymap::Action;
use gpui::{AsyncWindowContext, Context, KeyDownEvent, WeakEntity, Window};
use std::path::Path;
use std::time::Duration;

fn wait(cx: &AsyncWindowContext, ms: u64) -> gpui::Task<()> {
    cx.background_executor().timer(Duration::from_millis(ms))
}

fn key(this: &mut Workspace, k: &str, window: &mut Window, cx: &mut Context<Workspace>) {
    let ev = KeyDownEvent { keystroke: gpui::Keystroke::parse(k).unwrap(), is_held: false };
    this.popup_key(&ev, window, cx);
}

/// Types `text` into the open popup, one key at a time.
fn type_text(this: &mut Workspace, text: &str, window: &mut Window, cx: &mut Context<Workspace>) {
    for c in text.chars() {
        let mut stroke = gpui::Keystroke::parse(&c.to_string()).unwrap();
        stroke.key_char = Some(c.to_string());
        this.popup_key(&KeyDownEvent { keystroke: stroke, is_held: false }, window, cx);
    }
}

/// The open picker's rows as `label[hint]`, once its folder is listed.
async fn rows(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext) -> Vec<String> {
    let mut last = Vec::new();
    for _ in 0..100 {
        last = this
            .update(cx, |this, _| {
                this.popup
                    .as_ref()
                    .filter(|p| p.kind == PopupKind::PickFolder)
                    .map(|p| p.items.iter().map(|i| if i.hint.is_empty() { i.label.clone() } else { format!("{}[{}]", i.label, i.hint) }).collect())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        if !last.is_empty() && last[0] != "Loading…" {
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
        let base = brisk_project::folders::as_query(&root.join("picker"));
        cx.spawn_in(window, async move |this, cx| {
            wait(cx, 1000).await;
            // Open…: the folder's subfolders under the row that opens it.
            let q = base.clone();
            let _ = this.update_in(cx, |this, window, cx| {
                this.dispatch(Action::OpenProject, window, cx);
                this.set_picker_query(q, cx);
            });
            let got = rows(&this, cx).await;
            check(got == ["Open picker[Enter]", "alpha[project]", "beta"], "listing", &got);
            shot("open", cx).await;

            // Typing a name selects the folder it starts; Tab completes it.
            let selected = this
                .update_in(cx, |this, window, cx| {
                    type_text(this, "be", window, cx);
                    this.popup.as_ref().and_then(|p| p.items.get(p.selected)).map(|i| i.label.clone())
                })
                .ok()
                .flatten();
            check(selected.as_deref() == Some("beta"), "typing selects", &selected);
            let query = this
                .update_in(cx, |this, window, cx| {
                    key(this, "tab", window, cx);
                    this.popup.as_ref().map(|p| p.query.clone())
                })
                .ok()
                .flatten();
            check(query == Some(format!("{base}beta{}", std::path::MAIN_SEPARATOR)), "tab completes", &query);
            let got = rows(&this, cx).await;
            check(got == ["Open beta[Enter]"], "empty folder", &got);

            // New Project…: the typed name becomes a new folder, then This Window / New Window.
            let q = base.clone();
            let _ = this.update_in(cx, |this, window, cx| {
                key(this, "escape", window, cx);
                this.dispatch(Action::NewProject, window, cx);
                this.set_picker_query(q, cx);
            });
            let _ = rows(&this, cx).await;
            let _ = this.update_in(cx, |this, window, cx| type_text(this, "fresh", window, cx));
            let got = rows(&this, cx).await;
            check(got.first().is_some_and(|r| r.starts_with("Create fresh")), "create row", &got);
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
