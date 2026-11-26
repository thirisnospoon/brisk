//! Updates, as in PyCharm: once a day Brisk asks GitHub for the latest release, and a newer
//! one shows a balloon. Its dialog has the version, the date, the size and the release notes;
//! Update downloads the zip, swaps the files of the portable install (`brisk_update`) and
//! offers to restart. Help › Check for Updates asks at once, Help › What's New in Brisk shows
//! the notes of the running version. An install that is not the portable zip (a build from
//! source) gets the release page instead of Update.

use super::Workspace;
use crate::theme::{ui_px, Theme};
use brisk_update::{Release, CURRENT};
use gpui::prelude::*;
use gpui::{div, px, Context, ElementId, KeyDownEvent, Window};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The automatic check runs in the first window of a process only.
static AUTO_CHECKED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    Checking,
    Downloading,
    Installing,
    /// The new files are in place; a restart starts them.
    Ready,
    Failed(String),
}

pub struct Update {
    pub phase: Phase,
    /// The newer release, or (with `whats_new`) the running one.
    pub release: Option<Release>,
    pub balloon: bool,
    pub dialog: bool,
    /// The dialog shows the notes of the running version, without Update.
    pub whats_new: bool,
    got: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
}

impl Default for Update {
    fn default() -> Self {
        Update { phase: Phase::Idle, release: None, balloon: false, dialog: false, whats_new: false, got: Arc::default(), cancel: Arc::default() }
    }
}

impl Update {
    pub fn dialog_open(&self) -> bool {
        self.dialog
    }

    fn busy(&self) -> bool {
        matches!(self.phase, Phase::Checking | Phase::Downloading | Phase::Installing)
    }
}

fn state_path() -> PathBuf {
    brisk_platform::dirs::config_dir().join("update.json")
}

/// A block of the release notes.
enum Note {
    Heading(String),
    /// Nesting level (0, 1, …) and text.
    Bullet(usize, String),
    Para(String),
}

/// GitHub's release Markdown as headings, bullets and paragraphs; inline markup stays for
/// `inline_markdown`. Continuation lines join the bullet or paragraph above them. The
/// Install section (download the zip, unpack it) is left out: inside Brisk that is done.
fn notes(md: &str) -> Vec<Note> {
    let mut out: Vec<Note> = Vec::new();
    let mut open = false;
    let mut skip = false;
    for line in md.lines() {
        let t = line.trim();
        if t.is_empty() {
            open = false;
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if let Some(h) = t.strip_prefix('#') {
            let h = h.trim_start_matches('#').trim();
            skip = h.eq_ignore_ascii_case("install");
            if !skip {
                out.push(Note::Heading(h.to_string()));
            }
            open = false;
        } else if skip {
            continue;
        } else if let Some(b) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            out.push(Note::Bullet(indent / 2, b.trim().to_string()));
            open = true;
        } else if open {
            match out.last_mut() {
                Some(Note::Bullet(_, s) | Note::Para(s)) => {
                    s.push(' ');
                    s.push_str(t);
                }
                _ => out.push(Note::Para(t.to_string())),
            }
        } else {
            out.push(Note::Para(t.to_string()));
            open = true;
        }
    }
    out
}

fn button(id: &'static str, text: impl Into<gpui::SharedString>, primary: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(ui_px(26.))
        .px(px(12.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .rounded(px(5.))
        .cursor_pointer()
        .text_size(ui_px(12.5))
        .when(primary, |d| d.bg(Theme::accent()).text_color(gpui::white()))
        .when(!primary, |d| d.border_1().border_color(Theme::divider()))
        .map(|d| crate::ui_kit::button_states(d, primary))
        .child(text.into())
}

fn link(id: &'static str, text: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(4.))
        .cursor_pointer()
        .text_size(ui_px(12.5))
        .text_color(Theme::accent())
        .hover(|s| s.underline())
        .child(text)
        .child(crate::icons::ui_icon("external-link", 11., Theme::accent()))
}

/// A thin bar, `part` of it filled.
fn progress(part: f32) -> gpui::Div {
    div()
        .h(px(4.))
        .w_full()
        .rounded(px(2.))
        .bg(Theme::divider())
        .child(div().h_full().rounded(px(2.)).bg(Theme::accent()).w(gpui::relative(part.clamp(0., 1.))))
}

impl Workspace {
    /// At startup: remove what the last update retired, then, once a day and when Settings
    /// allows it, ask for a newer release. Release builds only, and never under a smoke
    /// unless it brings its own feed.
    pub(super) fn update_on_start(&mut self, cx: &mut Context<Self>) {
        if AUTO_CHECKED.swap(true, Ordering::Relaxed) {
            return;
        }
        if let Some(dir) = brisk_update::install_dir() {
            cx.background_executor().spawn(async move { brisk_update::clean_old(&dir) }).detach();
        }
        let feed = brisk_update::feed_overridden();
        if (crate::smoke::active() || cfg!(debug_assertions)) && !feed {
            return;
        }
        if !brisk_settings::current().check_updates {
            return;
        }
        if !feed && !brisk_update::State::load(&state_path()).due(brisk_update::now()) {
            return;
        }
        // A moment after the window is up, so the check never competes with startup.
        let delay = cx.background_executor().timer(Duration::from_secs(if feed { 0 } else { 5 }));
        cx.spawn(async move |this, cx| {
            delay.await;
            let _ = this.update(cx, |this, cx| this.check_for_updates(false, cx));
        })
        .detach();
    }

    /// Asks GitHub for the latest release. `manual` (Help › Check for Updates) opens the
    /// dialog and also says when there is nothing new or the check failed; the automatic
    /// check only shows the balloon, and not for a version the user skipped.
    pub(super) fn check_for_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
        if self.update.busy() {
            if manual && self.update.release.is_some() {
                self.update.dialog = true;
                self.update.balloon = false;
                cx.notify();
            }
            return;
        }
        if manual && self.update.phase == Phase::Ready {
            self.update.dialog = true;
            self.update.balloon = false;
            cx.notify();
            return;
        }
        self.update.phase = Phase::Checking;
        if manual {
            self.status = "Checking for updates…".into();
        }
        let task = cx.background_executor().spawn(async move { brisk_update::latest() });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.update.phase = Phase::Idle;
                let mut state = brisk_update::State::load(&state_path());
                match res {
                    Ok(r) => {
                        state.last_check = brisk_update::now();
                        let _ = state.save(&state_path());
                        let newer = brisk_update::is_newer(&r.version, CURRENT);
                        if manual {
                            this.status.clear();
                        }
                        if newer && (manual || state.skipped.as_deref() != Some(r.version.as_str())) {
                            this.update.release = Some(r);
                            this.update.whats_new = false;
                            this.update.dialog = manual;
                            this.update.balloon = !manual;
                        } else if manual {
                            this.show_toast(format!("Brisk {CURRENT} is the latest version"), false, cx);
                        }
                    }
                    Err(e) => {
                        if manual {
                            this.status.clear();
                            this.show_toast(format!("Cannot check for updates: {e}"), true, cx);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Help › What's New in Brisk: the release notes of the running version.
    pub(super) fn whats_new(&mut self, cx: &mut Context<Self>) {
        self.status = "Loading the release notes…".into();
        let task = cx.background_executor().spawn(async move { brisk_update::release(CURRENT) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.status.clear();
                match res {
                    Ok(r) => {
                        // A pending update keeps its own release; What's New only borrows the dialog.
                        if this.update.busy() || this.update.phase == Phase::Ready {
                            this.show_toast(
                                format!(
                                    "Brisk {} is being installed; its notes are in the update dialog",
                                    this.update.release.as_ref().map_or("", |r| r.version.as_str())
                                ),
                                false,
                                cx,
                            );
                            this.update.dialog = true;
                        } else {
                            this.update.release = Some(r);
                            this.update.whats_new = true;
                            this.update.dialog = true;
                            this.update.balloon = false;
                        }
                    }
                    Err(e) => this.show_toast(format!("Cannot load the release notes: {e}"), true, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Update: download, swap the files in, then offer the restart. Without a portable
    /// install or a package for this system, the release page opens instead.
    fn update_start(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.update.release.clone() else { return };
        let (Some(asset), Some(dir)) = (r.asset.clone(), brisk_update::install_dir()) else {
            cx.open_url(if r.page.is_empty() { brisk_update::RELEASES_PAGE } else { &r.page });
            return;
        };
        self.update.phase = Phase::Downloading;
        self.update.got.store(0, Ordering::Relaxed);
        self.update.cancel.store(false, Ordering::Relaxed);
        let (got, cancel) = (self.update.got.clone(), self.update.cancel.clone());
        let zip = dir.join(".brisk-download").join(&asset.name);
        let bg = cx.background_executor().clone();
        let task = bg.spawn(async move {
            brisk_update::download(&asset, &zip, &got, &cancel)?;
            let res = brisk_update::install(&zip, &dir);
            let _ = std::fs::remove_dir_all(zip.parent().unwrap_or(&zip));
            res
        });
        // The progress bar: a redraw every 150 ms while the download runs.
        cx.spawn(async move |this, cx| loop {
            bg.timer(Duration::from_millis(150)).await;
            let going = this
                .update(cx, |this, cx| {
                    if this.update.phase == Phase::Downloading
                        && this.update.got.load(Ordering::Relaxed) >= this.update.release.as_ref().and_then(|r| r.asset.as_ref()).map_or(u64::MAX, |a| a.size)
                    {
                        this.update.phase = Phase::Installing;
                    }
                    cx.notify();
                    matches!(this.update.phase, Phase::Downloading | Phase::Installing)
                })
                .unwrap_or(false);
            if !going {
                break;
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.update.phase = match res {
                    Ok(_) => Phase::Ready,
                    Err(e) if e == "cancelled" => Phase::Idle,
                    Err(e) => Phase::Failed(e),
                };
                if this.update.phase == Phase::Ready && !this.update.dialog {
                    this.update.balloon = true;
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn update_skip(&mut self, cx: &mut Context<Self>) {
        if let Some(r) = &self.update.release {
            let mut state = brisk_update::State::load(&state_path());
            state.skipped = Some(r.version.clone());
            let _ = state.save(&state_path());
        }
        self.update_close(cx);
    }

    /// Later / Close / Esc. A download keeps going; the balloon shows it.
    fn update_close(&mut self, cx: &mut Context<Self>) {
        let keep = self.update.busy() || self.update.phase == Phase::Ready;
        self.update.dialog = false;
        self.update.balloon = keep && !self.update.whats_new;
        if !keep {
            self.update.release = None;
            self.update.phase = Phase::Idle;
            self.update.whats_new = false;
        }
        cx.notify();
    }

    /// Restart: save everything, start the new brisk.exe on this project, quit.
    fn update_restart(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = brisk_update::install_dir() else { return };
        if !self.save_all_for_quit(cx) {
            return;
        }
        let exe = dir.join(if cfg!(windows) { "brisk.exe" } else { "brisk" });
        match std::process::Command::new(&exe).arg(&self.root).current_dir(&dir).spawn() {
            Ok(_) => cx.quit(),
            Err(e) => self.show_toast(format!("Cannot start {}: {e}. Start Brisk again yourself.", exe.display()), true, cx),
        }
    }

    pub(super) fn update_dialog_key(&mut self, e: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        match e.keystroke.key.as_str() {
            "escape" => self.update_close(cx),
            "enter" if !self.update.whats_new && matches!(self.update.phase, Phase::Idle | Phase::Failed(_)) => self.update_start(cx),
            "enter" if self.update.phase == Phase::Ready => self.update_restart(cx),
            "enter" if self.update.whats_new => self.update_close(cx),
            _ => {}
        }
    }

    /// `12.3 of 37.4 MB`.
    fn update_progress_text(&self) -> (f32, String) {
        let got = self.update.got.load(Ordering::Relaxed);
        let total = self.update.release.as_ref().and_then(|r| r.asset.as_ref()).map_or(0, |a| a.size);
        if total == 0 {
            return (0., brisk_update::megabytes(got));
        }
        (got as f32 / total as f32, format!("{} of {}", brisk_update::megabytes(got).trim_end_matches(" MB"), brisk_update::megabytes(total)))
    }

    /// The buttons for the current phase; shared by the balloon and the dialog.
    fn update_actions(&self, dialog: bool, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let mut v = Vec::new();
        let in_place = brisk_update::install_dir().is_some() && self.update.release.as_ref().is_some_and(|r| r.asset.is_some());
        match &self.update.phase {
            Phase::Downloading | Phase::Installing => {
                if self.update.phase == Phase::Downloading {
                    v.push(
                        button(if dialog { "upd-d-cancel" } else { "upd-b-cancel" }, "Cancel", false)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update.cancel.store(true, Ordering::Relaxed);
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
            Phase::Ready => {
                v.push(
                    button(if dialog { "upd-d-later" } else { "upd-b-later" }, "Later", false)
                        .on_click(cx.listener(|this, _, _, cx| this.update_close(cx)))
                        .into_any_element(),
                );
                v.push(
                    button(if dialog { "upd-d-restart" } else { "upd-b-restart" }, "Restart Now", true)
                        .on_click(cx.listener(|this, _, _, cx| this.update_restart(cx)))
                        .into_any_element(),
                );
            }
            _ if self.update.whats_new => {
                v.push(button("upd-d-close", "Close", true).on_click(cx.listener(|this, _, _, cx| this.update_close(cx))).into_any_element());
            }
            _ => {
                if dialog {
                    v.push(button("upd-d-skip", "Skip This Version", false).on_click(cx.listener(|this, _, _, cx| this.update_skip(cx))).into_any_element());
                    v.push(button("upd-d-later", "Later", false).on_click(cx.listener(|this, _, _, cx| this.update_close(cx))).into_any_element());
                } else {
                    v.push(
                        button("upd-b-notes", "What's New", false)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update.dialog = true;
                                this.update.balloon = false;
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
                let label = match (&self.update.phase, in_place) {
                    (Phase::Failed(_), true) => "Try Again",
                    (_, true) if dialog => "Update and Restart",
                    (_, true) => "Update",
                    (_, false) => "Download",
                };
                v.push(
                    button(if dialog { "upd-d-go" } else { "upd-b-go" }, label, true)
                        .on_click(cx.listener(|this, _, _, cx| this.update_start(cx)))
                        .into_any_element(),
                );
            }
        }
        v
    }

    /// One line under the title: what is going on.
    fn update_line(&self) -> Option<(String, bool)> {
        let r = self.update.release.as_ref()?;
        Some(match &self.update.phase {
            Phase::Downloading => (format!("Downloading… {}", self.update_progress_text().1), false),
            Phase::Installing => ("Installing…".into(), false),
            Phase::Ready => (format!("Brisk {} is installed. Restart to use it.", r.version), false),
            Phase::Failed(e) => (format!("The update failed: {e}"), true),
            _ if self.update.whats_new => {
                let mut s = "This is the version you have".to_string();
                if !r.date.is_empty() {
                    s.push_str(&format!(" · released {}", r.date));
                }
                (s, false)
            }
            _ => {
                let mut parts = vec![format!("You have {CURRENT}")];
                if !r.date.is_empty() {
                    parts.push(format!("released {}", r.date));
                }
                if let Some(a) = r.asset.as_ref().filter(|a| a.size > 0) {
                    parts.push(brisk_update::megabytes(a.size));
                }
                (parts.join(" · "), false)
            }
        })
    }

    pub(super) fn render_update_balloon(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.update.balloon || self.update.dialog {
            return None;
        }
        let r = self.update.release.as_ref()?;
        let title = match self.update.phase {
            Phase::Ready => "Update installed".to_string(),
            _ => format!("Brisk {} is available", r.version),
        };
        let (line, bad) = self.update_line().unwrap_or_default();
        let downloading = matches!(self.update.phase, Phase::Downloading | Phase::Installing);
        let part = self.update_progress_text().0;
        Some(
            div()
                .id("update-balloon")
                .occlude()
                .absolute()
                .bottom(ui_px(28.) + px(60.))
                .right(super::toast_right())
                .w(ui_px(380.))
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
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(crate::icons::ui_icon(
                            if self.update.phase == Phase::Ready { "circle-check" } else { "arrow-down-to-line" },
                            15.,
                            Theme::accent(),
                        ))
                        .child(div().flex_1().font_weight(gpui::FontWeight::SEMIBOLD).child(title))
                        .when(!downloading, |d| {
                            d.child(
                                div()
                                    .id("update-balloon-x")
                                    .cursor_pointer()
                                    .rounded(px(4.))
                                    .p(px(2.))
                                    .hover(|s| s.bg(Theme::hover_row()))
                                    .child(crate::icons::ui_icon("x", 13., Theme::text_dim()))
                                    .on_click(cx.listener(|this, _, _, cx| this.update_close(cx))),
                            )
                        }),
                )
                .child(div().text_size(ui_px(12.5)).text_color(if bad { Theme::error() } else { Theme::text_muted() }).child(line))
                .when(self.update.phase == Phase::Downloading, |d| d.child(progress(part)))
                .child(div().flex().justify_end().gap(px(8.)).children(self.update_actions(false, cx)))
                .into_any_element(),
        )
    }

    pub(super) fn render_update_dialog(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.update.dialog {
            return None;
        }
        let r = self.update.release.as_ref()?;
        let (line, bad) = self.update_line().unwrap_or_default();
        let title = if self.update.whats_new { format!("What's New in Brisk {}", r.version) } else { r.title.clone() };
        let mut body: Vec<gpui::AnyElement> = Vec::new();
        for (i, n) in notes(&r.notes).into_iter().enumerate() {
            let id = ElementId::NamedInteger("update-note".into(), i as u64);
            body.push(match n {
                Note::Heading(h) => div()
                    .pt(px(if i == 0 { 0. } else { 12. }))
                    .pb(px(2.))
                    .text_size(ui_px(14.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(crate::editor::doc_view::inline_markdown(&h, id))
                    .into_any_element(),
                Note::Bullet(level, t) => div()
                    .flex()
                    .gap(px(8.))
                    .pl(px(4. + 16. * level as f32))
                    .child(div().flex_shrink_0().text_color(Theme::text_dim()).child(if level == 0 { "•" } else { "◦" }))
                    .child(div().flex_1().min_w_0().child(crate::editor::doc_view::inline_markdown(&t, id)))
                    .into_any_element(),
                Note::Para(t) => div().py(px(2.)).child(crate::editor::doc_view::inline_markdown(&t, id)).into_any_element(),
            });
        }
        if body.is_empty() {
            body.push(div().text_color(Theme::text_dim()).child("This release has no notes.").into_any_element());
        }
        let page = if r.page.is_empty() { brisk_update::RELEASES_PAGE.to_string() } else { r.page.clone() };
        let part = self.update_progress_text().0;
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(70.))
                .occlude()
                .child(
                    div()
                        .id("update-dialog")
                        .w(ui_px(640.))
                        .flex()
                        .flex_col()
                        .bg(Theme::panel())
                        .border_1()
                        .border_color(Theme::divider())
                        .rounded(px(10.))
                        .map(crate::ui_kit::floating)
                        .text_size(ui_px(13.))
                        .text_color(Theme::text())
                        .child(
                            div()
                                .flex()
                                .gap(px(12.))
                                .items_center()
                                .px(px(18.))
                                .pt(px(16.))
                                .pb(px(12.))
                                .child(div().size(ui_px(40.)).flex_shrink_0().flex().items_center().justify_center().rounded(px(9.)).bg(Theme::accent()).child(
                                    crate::icons::ui_icon(if self.update.whats_new { "wand-sparkles" } else { "arrow-down-to-line" }, 20., gpui::white()),
                                ))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.))
                                        .min_w_0()
                                        .child(div().text_size(ui_px(17.)).font_weight(gpui::FontWeight::SEMIBOLD).child(title))
                                        .child(div().text_size(ui_px(12.5)).text_color(if bad { Theme::error() } else { Theme::text_dim() }).child(line)),
                                ),
                        )
                        .when(matches!(self.update.phase, Phase::Downloading), |d| d.child(div().px(px(18.)).pb(px(10.)).child(progress(part))))
                        .child(div().h(px(1.)).bg(Theme::divider()))
                        .child(
                            div()
                                .id("update-notes")
                                .max_h(ui_px(380.))
                                .overflow_y_scroll()
                                .px(px(18.))
                                .py(px(12.))
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .bg(Theme::editor_bg())
                                .children(body),
                        )
                        .child(div().h(px(1.)).bg(Theme::divider()))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .px(px(18.))
                                .py(px(12.))
                                .child(link("update-page", "View on GitHub").on_click(move |_, _, cx| cx.open_url(&page)))
                                .child(div().flex_1())
                                .children(self.update_actions(true, cx)),
                        ),
                )
                .into_any_element(),
        )
    }

    // --- Smoke --------------------------------------------------------------------------

    /// BRISK_SMOKE_UPDATE=1 with BRISK_UPDATE_FEED pointing at a release JSON whose asset
    /// is a `file://` zip, from a copy of a portable install: the startup check shows the
    /// balloon, What's New opens the dialog with the notes, Update downloads and swaps the
    /// files in, and the dialog offers the restart. Exits 1 on a failure.
    #[cfg(feature = "smoke")]
    pub(super) fn update_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hold = std::env::var("BRISK_SMOKE_UPDATE").unwrap_or_default();
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let pause = |ms| bg.timer(Duration::from_millis(ms));
            let mut failed = false;
            let mut check = |ok: bool, what: String| {
                eprintln!("smoke-update: {what}: {}", if ok { "ok" } else { "FAIL" });
                failed |= !ok;
            };
            let mut balloon = false;
            for _ in 0..100 {
                pause(100).await;
                balloon = this.update(cx, |this, _| this.update.balloon).unwrap_or(false);
                if balloon {
                    break;
                }
            }
            let version = this.update(cx, |this, _| this.update.release.as_ref().map(|r| r.version.clone())).ok().flatten();
            check(balloon, format!("balloon for {version:?}"));
            if hold == "balloon" {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                this.update.dialog = true;
                this.update.balloon = false;
                cx.notify();
            });
            pause(300).await;
            let (n, line) = this
                .update(cx, |this, _| (this.update.release.as_ref().map_or(0, |r| notes(&r.notes).len()), this.update_line().map(|l| l.0).unwrap_or_default()))
                .unwrap_or_default();
            check(n > 0, format!("dialog: {n} note blocks, '{line}'"));
            if hold == "dialog" {
                return;
            }
            let _ = this.update(cx, |this, cx| this.update_start(cx));
            let mut phase = Phase::Idle;
            for _ in 0..300 {
                pause(100).await;
                phase = this.update(cx, |this, _| this.update.phase.clone()).unwrap_or(Phase::Idle);
                if matches!(phase, Phase::Ready | Phase::Failed(_) | Phase::Idle) {
                    break;
                }
            }
            check(phase == Phase::Ready, format!("after Update: {phase:?}"));
            let dir = brisk_update::install_dir();
            let marker = dir.as_ref().and_then(|d| std::fs::read_to_string(d.join("UPDATED.txt")).ok());
            check(marker.is_some(), format!("new file in {dir:?}: {marker:?}"));
            let retired = dir.as_ref().is_some_and(|d| d.join(format!("brisk.exe{}", brisk_update::OLD_SUFFIX)).is_file());
            check(retired, "the running exe was renamed aside".into());
            if hold == "ready" {
                return;
            }
            eprintln!("smoke-update: done");
            let _ = cx.update(|_, cx| {
                if failed {
                    std::process::exit(1);
                }
                cx.quit()
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_notes_blocks() {
        let md = "Intro\nline.\n\n## Install\n\nDownload `x.zip`, unpack it\nanywhere.\n\n## Highlights\n\n- One\n  continued\n- Two:\n  - nested\n* Three\n";
        let n = notes(md);
        let shown: Vec<String> = n
            .iter()
            .map(|b| match b {
                Note::Heading(h) => format!("H {h}"),
                Note::Bullet(l, t) => format!("B{l} {t}"),
                Note::Para(t) => format!("P {t}"),
            })
            .collect();
        assert_eq!(shown, ["P Intro line.", "H Highlights", "B0 One continued", "B0 Two:", "B1 nested", "B0 Three"]);
    }
}
