//! Brisk's own folder browser for Open… and New Project…, like PyCharm's file chooser: places
//! on the left (home, Desktop, Documents, Downloads, the drives, recent projects), the current
//! folder's subfolders on the right, an editable path above them and, for a new project, the
//! new folder's name below. Folders are listed in the background, so a slow drive never holds
//! the window.
//!
//! What Open / Create acts on is the selected folder, or the folder shown when nothing is
//! selected. Keys: arrows move, `→` or a double click steps into a folder, `←` / `Backspace`
//! goes up, `Enter` is OK, `Tab` moves between the path, the list and the name.

use super::Workspace;
use crate::theme::{ui_px, Theme};
use crate::ui_kit;
use brisk_project::folders::{self, Folder};
use gpui::{div, prelude::*, px, Context, ElementId, KeyDownEvent, ScrollHandle, SharedString, Window};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum Purpose {
    /// Open an existing folder.
    #[default]
    Open,
    /// Create a folder for a new project and open it.
    New,
}

/// Where typing goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Path,
    List,
    Name,
}

struct Place {
    label: String,
    icon: &'static str,
    path: PathBuf,
    /// The heading it goes under.
    group: &'static str,
}

pub(super) struct FolderPicker {
    pub purpose: Purpose,
    /// The folder shown.
    pub dir: PathBuf,
    /// Its subfolders; `None` while they are listed.
    pub listing: Option<Result<Vec<Folder>, String>>,
    /// Index into [`FolderPicker::visible`].
    pub selected: Option<usize>,
    pub field: Field,
    /// The path field's text.
    pub path: String,
    /// The new project's folder name.
    pub name: String,
    /// The focused field's text is selected: typing replaces it.
    select_all: bool,
    pub show_hidden: bool,
    /// Why the last OK or path did not work.
    pub error: Option<String>,
    places: Vec<Place>,
    /// Bumped by every listing, so a late one for a folder left since is dropped.
    serial: u64,
    scroll: ScrollHandle,
}

impl FolderPicker {
    /// The subfolders shown: hidden and system ones only when asked for.
    pub fn visible(&self) -> Vec<&Folder> {
        match &self.listing {
            Some(Ok(all)) if self.show_hidden => all.iter().collect(),
            Some(Ok(all)) => folders::matching(all, ""),
            _ => Vec::new(),
        }
    }

    /// What OK acts on: the selected folder, else the one shown.
    pub fn target(&self) -> PathBuf {
        self.selected.and_then(|i| self.visible().get(i).map(|f| f.path.clone())).unwrap_or_else(|| self.dir.clone())
    }

    #[cfg(feature = "smoke")]
    pub fn place_count(&self) -> usize {
        self.places.len()
    }

    fn text_mut(&mut self) -> Option<&mut String> {
        match self.field {
            Field::Path => Some(&mut self.path),
            Field::Name => Some(&mut self.name),
            Field::List => None,
        }
    }
}

fn places() -> Vec<Place> {
    let mut v = Vec::new();
    let home = brisk_platform::dirs::home();
    if let Some(h) = &home {
        v.push(Place { label: "Home".into(), icon: "house", path: h.clone(), group: "Places" });
        // Windows may keep these under OneDrive.
        for (name, icon) in [("Desktop", "monitor"), ("Documents", "file-text"), ("Downloads", "download")] {
            if let Some(p) = [h.join(name), h.join("OneDrive").join(name)].into_iter().find(|p| p.is_dir()) {
                v.push(Place { label: name.into(), icon, path: p, group: "Places" });
            }
        }
    }
    for r in brisk_platform::dirs::roots() {
        let label = r.to_string_lossy().trim_end_matches(['\\', '/']).to_string();
        v.push(Place { label: if label.is_empty() { "/".into() } else { label }, icon: "hard-drive", path: r, group: "Drives" });
    }
    for p in super::project_ui::recent_projects().into_iter().take(8) {
        v.push(Place { label: folder_name(&p), icon: "history", path: p, group: "Recent" });
    }
    v
}

impl Workspace {
    /// Opens the browser at the folder that holds the current project.
    pub(super) fn open_folder_picker(&mut self, purpose: Purpose, cx: &mut Context<Self>) {
        let start = self.root.parent().map(Path::to_path_buf).or_else(brisk_platform::dirs::home).unwrap_or_else(|| self.root.clone());
        self.popup = None;
        self.folder_picker = Some(FolderPicker {
            purpose,
            dir: start.clone(),
            listing: None,
            selected: None,
            field: if purpose == Purpose::New { Field::Name } else { Field::List },
            path: String::new(),
            name: String::new(),
            select_all: false,
            show_hidden: false,
            error: None,
            places: places(),
            serial: 0,
            scroll: ScrollHandle::new(),
        });
        self.picker_go(start, None, cx);
    }

    /// Shows `dir`, selecting the subfolder `select` once listed.
    pub(super) fn picker_go(&mut self, dir: PathBuf, select: Option<PathBuf>, cx: &mut Context<Self>) {
        let Some(b) = self.folder_picker.as_mut() else { return };
        b.dir = dir.clone();
        b.path = dir.to_string_lossy().into_owned();
        b.listing = None;
        b.selected = None;
        b.error = None;
        b.serial += 1;
        b.scroll.set_offset(gpui::point(px(0.), px(0.)));
        let serial = b.serial;
        let task = cx.background_executor().spawn({
            let dir = dir.clone();
            async move { folders::list_dirs(&dir).map_err(|e| e.to_string()) }
        });
        cx.spawn(async move |this, cx| {
            let listing = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(b) = this.folder_picker.as_mut().filter(|b| b.serial == serial) else { return };
                b.listing = Some(listing);
                if let Some(s) = &select {
                    b.selected = b.visible().iter().position(|f| &f.path == s);
                    if let Some(i) = b.selected {
                        b.scroll.scroll_to_item(i);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn picker_up(&mut self, cx: &mut Context<Self>) {
        let Some(b) = &self.folder_picker else { return };
        if let Some(parent) = b.dir.parent().map(Path::to_path_buf) {
            let from = b.dir.clone();
            self.picker_go(parent, Some(from), cx);
        }
    }

    fn picker_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.folder_picker = None;
        self.focus_editor(window, cx);
        cx.notify();
    }

    /// The path field's Enter: show the typed folder.
    fn picker_go_typed(&mut self, cx: &mut Context<Self>) {
        let Some(b) = self.folder_picker.as_mut() else { return };
        let home = brisk_platform::dirs::home();
        let typed = b.path.trim().trim_matches('"').to_string();
        let (dir, rest) = folders::split_query(&folders::as_query(Path::new(&typed)), home.as_deref());
        let dir = dir.filter(|_| rest.is_empty()).unwrap_or_else(|| PathBuf::from(&typed));
        if dir.is_dir() {
            b.field = Field::List;
            self.picker_go(dir, None, cx);
        } else {
            b.error = Some(format!("No folder {typed}"));
            cx.notify();
        }
    }

    /// OK: open the target folder, or create the new project's folder in it.
    pub(super) fn picker_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.folder_picker.as_mut() else { return };
        if b.field == Field::Path && Path::new(b.path.trim()) != b.dir {
            return self.picker_go_typed(cx);
        }
        let target = b.target();
        match b.purpose {
            Purpose::Open => {
                self.picker_close(window, cx);
                self.ask_open_where(target, cx);
            }
            Purpose::New => {
                let name = b.name.trim().to_string();
                if name.is_empty() {
                    b.field = Field::Name;
                    b.error = Some("Type the new project's folder name".into());
                } else if !folders::valid_new_name(&name) {
                    b.field = Field::Name;
                    b.error = Some(format!("'{name}' cannot be a folder name"));
                } else {
                    let path = target.join(&name);
                    if path.exists() {
                        b.error = Some(format!("{} already exists", path.display()));
                    } else {
                        match std::fs::create_dir(&path) {
                            Ok(()) => {
                                self.picker_close(window, cx);
                                self.ask_open_where(path, cx);
                                return;
                            }
                            Err(e) => b.error = Some(format!("Cannot create {}: {e}", path.display())),
                        }
                    }
                }
                cx.notify();
            }
        }
    }

    fn picker_select(&mut self, i: Option<usize>, cx: &mut Context<Self>) {
        if let Some(b) = self.folder_picker.as_mut() {
            b.selected = i;
            if let Some(i) = i {
                b.scroll.scroll_to_item(i);
            }
            cx.notify();
        }
    }

    /// Steps into the selected folder.
    fn picker_enter_selected(&mut self, cx: &mut Context<Self>) {
        let Some(b) = &self.folder_picker else { return };
        if let Some(f) = b.selected.and_then(|i| b.visible().get(i).map(|f| f.path.clone())) {
            self.picker_go(f, None, cx);
        }
    }

    pub(super) fn picker_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let m = k.modifiers;
        let Some(b) = self.folder_picker.as_mut() else { return };
        let n = b.visible().len();
        let plain = !(m.control || m.alt || m.platform);
        match k.key.as_str() {
            "escape" => return self.picker_close(window, cx),
            "enter" => return self.picker_ok(window, cx),
            "tab" => {
                let order: &[Field] = if b.purpose == Purpose::New { &[Field::Path, Field::List, Field::Name] } else { &[Field::Path, Field::List] };
                let pos = order.iter().position(|f| *f == b.field).unwrap_or(0);
                b.field = order[if m.shift { (pos + order.len() - 1) % order.len() } else { (pos + 1) % order.len() }];
                b.select_all = b.field != Field::List;
            }
            "up" if m.alt => return self.picker_up(cx),
            "f5" => {
                let (dir, sel) = (b.dir.clone(), b.selected.and_then(|i| b.visible().get(i).map(|f| f.path.clone())));
                return self.picker_go(dir, sel, cx);
            }
            "h" if m.control => b.show_hidden = !b.show_hidden,
            _ if b.field == Field::List => match k.key.as_str() {
                "up" | "down" | "home" | "end" | "pageup" | "pagedown" if n > 0 => {
                    let cur = b.selected;
                    let i = match k.key.as_str() {
                        "up" => cur.map_or(n - 1, |i| i.saturating_sub(1)),
                        "down" => cur.map_or(0, |i| (i + 1).min(n - 1)),
                        "pageup" => cur.map_or(0, |i| i.saturating_sub(10)),
                        "pagedown" => cur.map_or(0, |i| (i + 10).min(n - 1)),
                        "home" => 0,
                        _ => n - 1,
                    };
                    return self.picker_select(Some(i), cx);
                }
                "right" => return self.picker_enter_selected(cx),
                "left" | "backspace" => return self.picker_up(cx),
                _ if plain => {
                    // Type to jump: the next folder starting with the letter.
                    if let Some(t) = crate::keys::typed_text(k).map(str::to_lowercase) {
                        let names: Vec<String> = b.visible().iter().map(|f| f.name.to_lowercase()).collect();
                        let from = b.selected.map_or(0, |i| i + 1);
                        let hit = (0..n).map(|d| (from + d) % n.max(1)).find(|&i| names.get(i).is_some_and(|s| s.starts_with(&t)));
                        if hit.is_some() {
                            return self.picker_select(hit, cx);
                        }
                    }
                }
                _ => {}
            },
            "backspace" => {
                let all = b.select_all || m.control;
                b.select_all = false;
                if let Some(t) = b.text_mut() {
                    if all {
                        t.clear();
                    } else {
                        t.pop();
                    }
                }
                b.error = None;
            }
            "a" if m.control => b.select_all = true,
            "v" if m.control => {
                if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    let all = std::mem::take(&mut b.select_all);
                    if let Some(s) = b.text_mut() {
                        if all {
                            s.clear();
                        }
                        s.push_str(t.trim().lines().next().unwrap_or(""));
                    }
                    b.error = None;
                }
            }
            _ if plain => {
                if let Some(ch) = crate::keys::typed_text(k).map(str::to_string) {
                    let all = std::mem::take(&mut b.select_all);
                    if let Some(s) = b.text_mut() {
                        if all {
                            s.clear();
                        }
                        s.push_str(&ch);
                    }
                    b.error = None;
                }
            }
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn render_folder_picker(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let b = self.folder_picker.as_ref()?;
        let new = b.purpose == Purpose::New;
        let title = if new { "New Project" } else { "Open Folder or Project" };
        let visible = b.visible();
        let field = |id: &'static str, f: Field, text: &str, placeholder: &'static str, cx: &mut Context<Self>| {
            let focused = b.field == f;
            let empty = text.is_empty();
            ui_kit::input_frame(focused, false, ui_kit::CONTROL_H)
                .id(id)
                .flex_1()
                .min_w_0()
                .px(px(8.))
                .text_size(ui_px(ui_kit::FIELD_TEXT))
                .cursor_text()
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(b) = this.folder_picker.as_mut() {
                        b.field = f;
                        b.select_all = false;
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .when(empty && !focused, |d| d.text_color(Theme::text_dim()).child(placeholder))
                        .when(!empty, |d| d.child(div().when(focused && b.select_all, |d| d.bg(Theme::selection())).child(tail(text, 84))))
                        .when(focused && !b.select_all, |d| d.child(div().w(px(1.5)).h(ui_px(15.)).bg(Theme::caret()))),
                )
                .when(focused, |d| d.child(ui_kit::focus_ring(ui_kit::RADIUS, 0.)))
        };
        let tool = |id: &'static str, icon: &'static str, text: &'static str| ui_kit::icon_button(id, icon, true).tooltip(tip(text));

        // Places, grouped under their headings.
        let mut side: Vec<gpui::AnyElement> = Vec::new();
        let mut group = "";
        for (i, p) in b.places.iter().enumerate() {
            if p.group != group {
                group = p.group;
                side.push(
                    div()
                        .px(px(10.))
                        .pt(px(if i == 0 { 6. } else { 12. }))
                        .pb(px(4.))
                        .text_size(ui_px(11.))
                        .text_color(Theme::text_dim())
                        .child(group)
                        .into_any_element(),
                );
            }
            let here = b.dir == p.path;
            let path = p.path.clone();
            side.push(
                div()
                    .id(ElementId::NamedInteger("picker-place".into(), i as u64))
                    .mx(px(4.))
                    .px(px(6.))
                    .h(ui_px(26.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .when(here, |d| d.bg(Theme::selection_row()))
                    .when(!here, |d| d.hover(|s| s.bg(Theme::hover_row())))
                    .tooltip(tip(p.path.to_string_lossy().into_owned()))
                    .on_click(cx.listener(move |this, _, _, cx| this.picker_go(path.clone(), None, cx)))
                    .child(crate::icons::ui_icon(p.icon, 15., Theme::text_muted()))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(p.label.clone()))
                    .into_any_element(),
            );
        }

        // The folder's subfolders.
        let list: Vec<gpui::AnyElement> = match &b.listing {
            None => vec![div().p(px(12.)).text_color(Theme::text_dim()).child("Loading…").into_any_element()],
            Some(Err(e)) => vec![div().p(px(12.)).text_color(Theme::error()).child(format!("Cannot read this folder: {e}")).into_any_element()],
            Some(Ok(_)) if visible.is_empty() => vec![div().p(px(12.)).text_color(Theme::text_dim()).child("No folders here").into_any_element()],
            Some(Ok(_)) => visible
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    let sel = b.selected == Some(i);
                    let path = f.path.clone();
                    div()
                        .id(ElementId::NamedInteger("picker-row".into(), i as u64))
                        .h(ui_px(26.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .flex_shrink_0()
                        .when(sel, |d| d.bg(if b.field == Field::List { Theme::selection_row() } else { Theme::hover_row() }))
                        .when(!sel, |d| d.hover(|s| s.bg(Theme::hover_row())))
                        .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                            if e.click_count() >= 2 {
                                this.picker_go(path.clone(), None, cx);
                            } else if let Some(b) = this.folder_picker.as_mut() {
                                b.field = if b.field == Field::Name { Field::Name } else { Field::List };
                                b.selected = Some(i);
                                b.error = None;
                                cx.notify();
                            }
                        }))
                        .child(crate::icons::ui_icon("folder", 15., if f.project { Theme::accent() } else { Theme::text_muted() }))
                        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(f.name.clone()))
                        .when(f.project, |d| d.child(div().text_size(ui_px(11.5)).text_color(Theme::text_dim()).child("project")))
                        .into_any_element()
                })
                .collect(),
        };

        let target = b.target();
        let status: SharedString = match (&b.error, new) {
            (Some(e), _) => e.clone().into(),
            (None, true) => {
                let name = b.name.trim();
                if name.is_empty() {
                    format!("In {}", tail(&target.to_string_lossy(), 90)).into()
                } else {
                    format!("Creates {}", tail(&target.join(name).to_string_lossy(), 90)).into()
                }
            }
            (None, false) => format!("Opens {}", tail(&target.to_string_lossy(), 90)).into(),
        };
        let ok = if new { "Create" } else { "Open" };
        let can_up = b.dir.parent().is_some();

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(60.))
                .occlude()
                .child(
                    div()
                        .id("folder-picker")
                        .w(ui_px(760.))
                        .max_w(gpui::relative(0.95))
                        .flex()
                        .flex_col()
                        .bg(Theme::panel())
                        .border_1()
                        .border_color(Theme::divider())
                        .rounded(px(10.))
                        .map(ui_kit::floating)
                        .text_size(ui_px(13.))
                        .text_color(Theme::text())
                        .child(div().px(px(16.)).pt(px(14.)).pb(px(10.)).font_weight(gpui::FontWeight::SEMIBOLD).text_size(ui_px(15.)).child(title))
                        .child(
                            div()
                                .px(px(16.))
                                .pb(px(10.))
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(
                                    ui_kit::icon_button("picker-up", "arrow-up", can_up)
                                        .tooltip(tip("Up (Backspace)"))
                                        .on_click(cx.listener(|this, _, _, cx| this.picker_up(cx))),
                                )
                                .child(tool("picker-refresh", "rotate-cw", "Refresh (F5)").on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(dir) = this.folder_picker.as_ref().map(|b| b.dir.clone()) {
                                        this.picker_go(dir, None, cx);
                                    }
                                })))
                                .child(field("picker-path", Field::Path, &b.path, "Folder path", cx))
                                .child(
                                    ui_kit::button("picker-hidden", "Hidden", true, false)
                                        .when(b.show_hidden, |d| d.bg(Theme::selection_row()))
                                        .tooltip(tip("Show hidden folders (Ctrl+H)"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if let Some(b) = this.folder_picker.as_mut() {
                                                b.show_hidden = !b.show_hidden;
                                                b.selected = None;
                                            }
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(div().h(px(1.)).bg(Theme::divider()))
                        .child(
                            div()
                                .h(ui_px(360.))
                                .flex()
                                .child(
                                    div()
                                        .id("picker-places")
                                        .w(ui_px(190.))
                                        .flex_shrink_0()
                                        .h_full()
                                        .overflow_y_scroll()
                                        .pb(px(6.))
                                        .flex()
                                        .flex_col()
                                        .children(side),
                                )
                                .child(div().w(px(1.)).h_full().bg(Theme::divider()))
                                .child(
                                    div()
                                        .id("picker-list")
                                        .flex_1()
                                        .min_w_0()
                                        .h_full()
                                        .overflow_y_scroll()
                                        .track_scroll(&b.scroll)
                                        .py(px(4.))
                                        .bg(Theme::editor_bg())
                                        .flex()
                                        .flex_col()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if let Some(b) = this.folder_picker.as_mut().filter(|b| b.field == Field::Path) {
                                                b.field = Field::List;
                                                cx.notify();
                                            }
                                        }))
                                        .children(list),
                                ),
                        )
                        .child(div().h(px(1.)).bg(Theme::divider()))
                        .when(new, |d| {
                            d.child(
                                div()
                                    .px(px(16.))
                                    .pt(px(12.))
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(div().w(ui_px(44.)).text_color(Theme::text_muted()).child("Name"))
                                    .child(field("picker-name", Field::Name, &b.name, "new project folder", cx)),
                            )
                        })
                        .child(
                            div()
                                .px(px(16.))
                                .py(px(12.))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_size(ui_px(12.))
                                        .text_color(if b.error.is_some() { Theme::error() } else { Theme::text_dim() })
                                        .child(status),
                                )
                                .child(
                                    ui_kit::button("picker-cancel", "Cancel", true, false)
                                        .on_click(cx.listener(|this, _, window, cx| this.picker_close(window, cx))),
                                )
                                .child(ui_kit::button("picker-ok", ok, true, true).on_click(cx.listener(|this, _, window, cx| this.picker_ok(window, cx)))),
                        ),
                )
                .into_any_element(),
        )
    }
}

fn folder_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned())
}

fn tip(t: impl Into<SharedString>) -> impl Fn(&mut Window, &mut gpui::App) -> gpui::AnyView + 'static {
    let t = t.into();
    move |_, cx| cx.new(|_| super::Tip(t.clone())).into()
}

/// The end of a long path, which says the most: `…/work/brisk`.
fn tail(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        s.to_string()
    } else {
        format!("…{}", s.chars().skip(n - max + 1).collect::<String>())
    }
}
