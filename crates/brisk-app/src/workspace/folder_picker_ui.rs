//! Brisk's own folder picker for Open… and New Project… (instead of the system dialog): a
//! path typed in the popup's field, with its folder's subfolders below it. Enter on a folder
//! steps into it, Tab completes the selected one, and the first row opens (or, for a new
//! project, creates) the folder the path names. Folders are listed in the background and
//! kept until the picker closes, so a slow drive never holds the window.

use super::{PopupItem, PopupKind, Target, Workspace};
use brisk_project::folders::{self, Folder};
use gpui::Context;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum Purpose {
    /// Open an existing folder.
    #[default]
    Open,
    /// Create a folder for a new project and open it.
    New,
}

#[derive(Clone, Debug)]
pub(super) enum Pick {
    /// Show this folder's subfolders.
    Into(PathBuf),
    /// Open this folder as the project.
    Open(PathBuf),
    /// Create this folder, then open it.
    Create(PathBuf),
}

#[derive(Default)]
pub(super) struct FolderPicker {
    purpose: Purpose,
    /// Folder listings: subfolders, or why the folder cannot be read.
    listed: HashMap<PathBuf, Result<Vec<Folder>, String>>,
    /// The folder the current path needs and nobody is listing yet.
    wanted: Option<PathBuf>,
    /// Bumped when the picker opens: a listing from an earlier picker is dropped.
    generation: u64,
}

impl Workspace {
    /// Opens the picker at the folder that holds the current project.
    pub(super) fn open_folder_picker(&mut self, purpose: Purpose, cx: &mut Context<Self>) {
        let start = self.root.parent().map(Path::to_path_buf).or_else(brisk_platform::dirs::home).unwrap_or_else(|| self.root.clone());
        let picker = &mut self.folder_picker;
        picker.purpose = purpose;
        picker.listed.clear();
        picker.wanted = None;
        picker.generation += 1;
        self.open_popup(PopupKind::PickFolder);
        self.set_picker_query(folders::as_query(&start), cx);
    }

    pub(super) fn picker_title(&self) -> (&'static str, &'static str) {
        match self.folder_picker.purpose {
            Purpose::Open => ("Open Folder", "Folder path · Enter steps in · Tab completes"),
            Purpose::New => ("New Project", "Where, then the new folder's name"),
        }
    }

    /// Puts `q` in the picker's field and shows what it names.
    pub(super) fn set_picker_query(&mut self, q: String, cx: &mut Context<Self>) {
        if let Some(p) = self.popup.as_mut().filter(|p| p.kind == PopupKind::PickFolder) {
            p.query = q;
            p.selected = 0;
        }
        self.refresh_popup();
        self.load_picker_folder(cx);
    }

    /// Lists the folder the typed path needs, if it is not listed yet.
    pub(super) fn load_picker_folder(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.folder_picker.wanted.take() else { return };
        // Listed as loading, so typing on does not start it again.
        self.folder_picker.listed.insert(dir.clone(), Ok(Vec::new()));
        let generation = self.folder_picker.generation;
        let task = cx.background_executor().spawn({
            let dir = dir.clone();
            async move { folders::list_dirs(&dir).map_err(|e| e.to_string()) }
        });
        cx.spawn(async move |this, cx| {
            let listing = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.folder_picker.generation != generation {
                    return;
                }
                this.folder_picker.listed.insert(dir, listing);
                if this.popup.as_ref().is_some_and(|p| p.kind == PopupKind::PickFolder) {
                    this.refresh_popup();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The picker's rows for the typed path.
    pub(super) fn picker_items(&mut self, q: &str) -> Vec<PopupItem> {
        let home = brisk_platform::dirs::home();
        let (dir, typed) = folders::split_query(q, home.as_deref());
        let row = |label: String, detail: String, hint: &str, pick: Pick| PopupItem { label, detail, hint: hint.into(), target: Target::PickFolder(pick) };
        let Some(dir) = dir else {
            // Nothing that names a folder yet: where to start.
            let typed = typed.to_lowercase();
            let mut starts: Vec<PathBuf> = home.into_iter().chain(brisk_platform::dirs::roots()).collect();
            starts.extend(super::project_ui::recent_projects());
            return starts
                .into_iter()
                .filter(|p| p.to_string_lossy().to_lowercase().contains(&typed))
                .map(|p| row(p.to_string_lossy().into_owned(), String::new(), "", Pick::Into(p)))
                .collect();
        };
        let listing = match self.folder_picker.listed.get(&dir) {
            Some(l) => l,
            None => {
                self.folder_picker.wanted = Some(dir.clone());
                return vec![PopupItem { label: "Loading…".into(), detail: String::new(), hint: String::new(), target: Target::Note }];
            }
        };
        let subfolders = match listing {
            Ok(v) => v,
            Err(e) => return vec![PopupItem { label: format!("Cannot read {}", dir.display()), detail: e.clone(), hint: String::new(), target: Target::Note }],
        };
        let mut items = Vec::new();
        let exact = subfolders.iter().find(|f| f.name.eq_ignore_ascii_case(&typed));
        match (self.folder_picker.purpose, typed.is_empty(), exact) {
            // `C:\code\`: this folder.
            (Purpose::Open, true, _) => {
                items.push(row(format!("Open {}", folder_name(&dir)), dir.to_string_lossy().into_owned(), "Enter", Pick::Open(dir.clone())))
            }
            (Purpose::New, true, _) => {}
            (Purpose::New, false, None) => {
                let path = dir.join(&typed);
                items.push(if folders::valid_new_name(&typed) {
                    row(format!("Create {typed}"), path.to_string_lossy().into_owned(), "Enter", Pick::Create(path))
                } else {
                    PopupItem { label: format!("'{typed}' cannot be a folder name"), detail: String::new(), hint: String::new(), target: Target::Note }
                });
            }
            // A folder with the typed name: open it straight away.
            (_, false, Some(f)) => items.push(row(format!("Open {}", f.name), f.path.to_string_lossy().into_owned(), "", Pick::Open(f.path.clone()))),
            (Purpose::Open, false, None) => {}
        }
        items.extend(
            folders::matching(subfolders, &typed)
                .into_iter()
                .map(|f| row(f.name.clone(), String::new(), if f.project { "project" } else { "" }, Pick::Into(f.path.clone()))),
        );
        if items.is_empty() {
            items.push(PopupItem { label: "No folders here".into(), detail: String::new(), hint: String::new(), target: Target::Note });
        }
        // Typing a name aims at a subfolder: select the first one, so Enter steps into it.
        if let Some(p) = self.popup.as_mut().filter(|p| p.kind == PopupKind::PickFolder && p.selected == 0) {
            if !typed.is_empty() && exact.is_none() {
                p.selected = items.iter().position(|i| matches!(i.target, Target::PickFolder(Pick::Into(_)))).unwrap_or(0);
            }
        }
        items
    }

    /// Enter (or a click) on a picker row. `false`: the popup stays open.
    pub(super) fn picker_pick(&mut self, pick: Pick, cx: &mut Context<Self>) -> bool {
        match pick {
            Pick::Into(dir) => {
                self.set_picker_query(folders::as_query(&dir), cx);
                false
            }
            Pick::Open(dir) => {
                self.popup = None;
                self.ask_open_where(dir, cx);
                true
            }
            Pick::Create(dir) => match std::fs::create_dir(&dir) {
                Ok(()) => {
                    self.popup = None;
                    self.ask_open_where(dir, cx);
                    true
                }
                Err(e) => {
                    self.status = format!("Cannot create {}: {e}", dir.display());
                    cx.notify();
                    false
                }
            },
        }
    }

    /// Tab: the selected subfolder's path in the field.
    pub(super) fn picker_complete(&mut self, cx: &mut Context<Self>) {
        let selected = self.popup.as_ref().and_then(|p| p.items.get(p.selected)).map(|i| i.target.clone());
        if let Some(Target::PickFolder(Pick::Into(dir) | Pick::Open(dir))) = selected {
            self.set_picker_query(folders::as_query(&dir), cx);
        }
    }
}

fn folder_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned())
}
