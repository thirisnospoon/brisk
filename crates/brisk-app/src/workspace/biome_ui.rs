//! Biome for JS, TS, JSON, CSS and GraphQL files, as PyCharm's Biome plugin: on in projects
//! that use it (`biome.json` or the package in `node_modules`), or everywhere when Settings
//! says so. Its server starts with the first such file and reports problems as you type;
//! Alt+Enter offers its quick fixes and suppressions; Reformat Code and Fix All run its
//! command line. Without an executable, a balloon offers the download (about 38 MB).

use super::lsp_ui::TyState;
use super::problems_ui::Intent;
use super::{PopupItem, PopupKind, Target, Workspace};
use crate::editor::{DiagSource, Editor};
use crate::theme::{ui_px, Theme};
use brisk_lsp::{biome, Ty};
use gpui::prelude::*;
use gpui::{div, px, Context, Entity, Window};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub(super) struct BiomeLsp {
    pub server: Option<Arc<Ty>>,
    pub state: TyState,
    /// Buffer version the server last saw for each open file.
    synced: HashMap<PathBuf, u64>,
    /// The download balloon is showing.
    pub offer: bool,
    /// Declined in this window: not offered again.
    declined: bool,
    downloading: bool,
    /// Whether the project uses Biome, and when that was looked up (it is asked on every
    /// editor change).
    in_use: std::cell::Cell<Option<(std::time::Instant, bool)>>,
}

impl Default for BiomeLsp {
    fn default() -> Self {
        BiomeLsp {
            server: None,
            state: TyState::Unknown,
            synced: HashMap::new(),
            offer: false,
            declined: false,
            downloading: false,
            in_use: Default::default(),
        }
    }
}

impl Workspace {
    /// Biome checks this file: a type it knows, in a project that uses Biome (or everywhere,
    /// when Settings says so).
    pub(super) fn biome_wanted(&self, path: &Path) -> bool {
        biome::supports(path) && (brisk_settings::biome_everywhere() || self.biome_in_use())
    }

    /// `biome::in_use`, looked up again after a few seconds (a `biome.json` may appear).
    fn biome_in_use(&self) -> bool {
        let cache = &self.lsp.biome.in_use;
        match cache.get() {
            Some((at, v)) if at.elapsed() < Duration::from_secs(5) => v,
            _ => {
                let v = biome::in_use(&self.root);
                cache.set(Some((std::time::Instant::now(), v)));
                v
            }
        }
    }

    /// Start the server once a Biome file needs it. Cheap to call often.
    fn ensure_biome(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = &self.lsp.biome.server {
            if !s.is_alive() {
                self.lsp.biome.server = None;
                self.lsp.biome.synced.clear();
                self.lsp.biome.state = TyState::Failed("Biome exited".into());
                self.status = "Biome exited; reopen the project to restart it".into();
            }
            return;
        }
        if self.lsp.biome.state != TyState::Unknown || self.lsp.biome.downloading {
            return;
        }
        let Some(exe) = biome::find(&self.root) else {
            self.lsp.biome.state = TyState::Missing;
            if !self.lsp.biome.declined && std::env::var_os("BRISK_BIOME").is_none() {
                self.lsp.biome.offer = true;
                cx.notify();
            }
            return;
        };
        self.lsp.biome.state = TyState::Starting;
        let root = self.root.clone();
        let task = cx.background_executor().spawn(async move { Ty::start_biome(&exe, &root) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(s) => {
                        this.lsp.biome.server = Some(Arc::new(s));
                        this.lsp.biome.state = TyState::Ready;
                        this.relint_all(cx);
                    }
                    Err(e) => {
                        this.status = format!("Biome: {e}");
                        this.lsp.biome.state = TyState::Failed(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The server with the file's current text, when it runs.
    fn biome_sync(&mut self, path: &Path, version: u64, text: &str) -> Option<Arc<Ty>> {
        let server = self.lsp.biome.server.clone()?;
        match self.lsp.biome.synced.get(path) {
            Some(&v) if v == version => {}
            Some(_) => server.did_change(path, version, text),
            None => server.did_open(path, version, text),
        }
        self.lsp.biome.synced.insert(path.to_path_buf(), version);
        Some(server)
    }

    /// Part of the as-you-type check: Biome's problems for this text.
    pub(super) fn biome_check(&mut self, ed: Entity<Editor>, path: PathBuf, version: u64, text: &str, cx: &mut Context<Self>) {
        self.ensure_biome(cx);
        let Some(server) = self.biome_sync(&path, version, text) else { return };
        let fut = server.pushed_diagnostics(&path, version);
        let timeout = cx.background_executor().timer(Duration::from_secs(10));
        cx.spawn(async move |_, cx| {
            use futures::FutureExt;
            // A newer check of the file replaces this one; a server that never answers is
            // given up on.
            let futures::future::Either::Left((Ok(diags), _)) = futures::future::select(fut.boxed(), timeout.boxed()).await else { return };
            let _ = ed.update(cx, |e, cx| {
                if e.set_diagnostics(DiagSource::Biome, version, diags) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// A tab closed: Biome can forget the unsaved text.
    pub(super) fn biome_close(&mut self, path: &Path) {
        if self.lsp.biome.synced.remove(path).is_some() {
            if let Some(s) = &self.lsp.biome.server {
                s.did_close(path);
            }
        }
    }

    /// Alt+Enter in a Biome file: its quick fixes for the problems at the caret, Fix All,
    /// then its suppressions.
    pub(super) fn show_biome_intentions(&mut self, ed: Entity<Editor>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = ed.read(cx).path.clone() else { return };
        let (probs, caret, version, text) = {
            let e = ed.read(cx);
            let (line, col) = e.caret_line_col();
            let probs: Vec<_> = e.problems_at_caret().into_iter().filter(|d| d.code.starts_with("lint/") || d.code.contains('/')).collect();
            (probs, brisk_lint::Pos { line, col }, e.buffer.version(), e.text_string())
        };
        let any_biome = ed.read(cx).diagnostics.iter().any(|d| d.code.starts_with("lint/") || d.code.starts_with("assist/"));
        self.ensure_biome(cx);
        let server = self.biome_sync(&path, version, &text);
        let (start, end) = match (probs.iter().map(|d| d.start).min(), probs.iter().map(|d| d.end).max()) {
            (Some(a), Some(b)) => (a, b),
            _ => (caret, caret),
        };
        let fixall = move |items: &mut Vec<PopupItem>| {
            if any_biome {
                items.push(PopupItem {
                    label: "Biome: fix all safe problems in file".into(),
                    detail: String::new(),
                    hint: String::new(),
                    target: Target::Intention(Box::new(Intent::BiomeFixAll)),
                });
            }
        };
        let Some(server) = server else {
            let mut items = Vec::new();
            fixall(&mut items);
            return self.open_biome_intentions(items);
        };
        let fut = server.code_actions(&path, start, end, &[]);
        let timeout = cx.background_executor().timer(Duration::from_secs(5));
        cx.spawn_in(window, async move |this, cx| {
            use futures::FutureExt;
            let actions = match futures::future::select(fut.boxed(), timeout.boxed()).await {
                futures::future::Either::Left((Ok(a), _)) => a,
                _ => Vec::new(),
            };
            let _ = this.update(cx, |this, cx| {
                let item = |a: &brisk_lsp::CodeAction| PopupItem {
                    label: a.title.trim_end_matches('.').to_string(),
                    detail: "Biome".into(),
                    hint: String::new(),
                    target: Target::Intention(Box::new(Intent::Lsp(a.title.clone(), a.edits.clone()))),
                };
                let suppress = |a: &&brisk_lsp::CodeAction| a.kind.contains("suppressRule");
                let mut items: Vec<PopupItem> = actions.iter().filter(|a| a.kind.starts_with("quickfix") && !suppress(a)).map(item).collect();
                fixall(&mut items);
                items.extend(actions.iter().filter(suppress).map(item));
                this.open_biome_intentions(items);
                cx.notify();
            });
        })
        .detach();
    }

    fn open_biome_intentions(&mut self, items: Vec<PopupItem>) {
        if items.is_empty() {
            self.status = "No context actions available here".into();
            return;
        }
        self.problems.intents = items;
        self.problems.intents_title = None;
        self.open_popup(PopupKind::Intentions);
    }

    /// "Biome: fix all safe problems in file" and Code Cleanup (`biome check --write` on the
    /// text), or Optimize Imports (`imports`: only the import sorting).
    pub(super) fn biome_fix_all(&mut self, ed: Entity<Editor>, imports: bool, cx: &mut Context<Self>) {
        let Some(path) = ed.read(cx).path.clone() else { return };
        let Some(exe) = biome::find(&self.root) else {
            self.status = "Biome is not installed".into();
            self.lsp.biome.offer = !self.lsp.biome.downloading;
            cx.notify();
            return;
        };
        let (text, version) = (ed.read(cx).text_string(), ed.read(cx).buffer.version());
        let root = self.root.clone();
        let task = cx.background_executor().spawn(async move {
            let out = if imports { biome::organize_imports(&exe, &root, &path, &text) } else { biome::fix_all(&exe, &root, &path, &text) };
            out.map(|out| (text, out))
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok((before, after)) if ed.read(cx).buffer.version() == version => {
                        let changed = after.replace("\r\n", "\n") != before.replace("\r\n", "\n");
                        if changed {
                            ed.update(cx, |e, cx| e.replace_all_text(&after, cx));
                        }
                        this.status = match (imports, changed) {
                            (true, true) => "Imports sorted (Biome)".into(),
                            (true, false) => "Imports are already optimized".into(),
                            (false, true) => "Biome: applied the safe fixes".into(),
                            (false, false) => "Biome: nothing to fix".into(),
                        };
                    }
                    Ok(_) => {}
                    Err(e) => this.show_toast(e, true, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn biome_download(&mut self, cx: &mut Context<Self>) {
        self.lsp.biome.offer = false;
        self.lsp.biome.downloading = true;
        self.status = "Downloading Biome…".into();
        let task = cx.background_executor().spawn(async move { biome::download() });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.lsp.biome.downloading = false;
                match res {
                    Ok(_) => {
                        this.status = "Biome downloaded".into();
                        this.lsp.biome.state = TyState::Unknown;
                        this.ensure_biome(cx);
                    }
                    Err(e) => {
                        this.show_toast(format!("Cannot download Biome: {e}"), true, cx);
                        this.lsp.biome.state = TyState::Failed(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_biome_offer(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.lsp.biome.offer || self.lsp.ts.offer {
            return None;
        }
        let button = |id: &'static str, text: &'static str, primary: bool| {
            div()
                .id(id)
                .h(ui_px(24.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .cursor_pointer()
                .when(primary, |d| d.bg(Theme::accent()).text_color(gpui::white()))
                .when(!primary, |d| d.border_1().border_color(Theme::border()))
                .map(|d| crate::ui_kit::button_states(d, primary))
                .child(text)
        };
        let why = if biome::in_use(&self.root) {
            "This project uses Biome, but it is not installed here. Download Biome (about 38 MB) to see its problems as you type and to format with it?"
        } else {
            "Download Biome (about 38 MB) to check and format JS, TS, CSS and JSON files?"
        };
        Some(
            div()
                .id("biome-offer")
                .occlude()
                .absolute()
                .bottom(ui_px(28.) + px(60.))
                .right(super::toast_right())
                .w(ui_px(420.))
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .rounded(px(8.))
                .bg(Theme::panel())
                .border_1()
                .border_color(Theme::divider())
                .map(crate::ui_kit::floating)
                .text_size(ui_px(13.))
                .text_color(Theme::text())
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Lint and format with Biome"))
                .child(div().text_size(ui_px(12.5)).text_color(Theme::text_muted()).child(why))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(button("biome-offer-no", "Not Now", false).on_click(cx.listener(|this, _, _, cx| {
                            this.lsp.biome.offer = false;
                            this.lsp.biome.declined = true;
                            cx.notify();
                        })))
                        .child(button("biome-offer-yes", "Download", true).on_click(cx.listener(|this, _, _, cx| this.biome_download(cx)))),
                )
                .into_any_element(),
        )
    }
}
