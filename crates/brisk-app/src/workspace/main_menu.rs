//! The main menu (☰ in the title bar, as in PyCharm's new UI): File, Edit, View, Navigate,
//! Code, Refactor, Run, Git, Tools, Window. File is laid out by hand; the others list the
//! actions of their keymap groups, so every action is reachable with the mouse.

use super::menu_ui::Entry;
use super::Workspace;
use brisk_keymap::{Action, ALL_ACTIONS};
use gpui::{Context, PathPromptOptions, Window};
use std::rc::Rc;

impl Workspace {
    pub(super) fn main_menu_entries(&self) -> Vec<Entry> {
        use Action as A;
        let a = |label: &str, act: Action| self.action_entry(label, act, true, false);
        let file = vec![
            a("New Project…", A::NewProject),
            a("New…", A::NewElement),
            Entry::Separator,
            a("Open…", A::OpenProject),
            a("Open File…", A::OpenFile),
            a("Recent Projects", A::RecentProjects),
            Entry::Separator,
            a("Close Tab", A::CloseTab),
            a("Reopen Closed Tab", A::ReopenClosedTab),
            Entry::Separator,
            a("Settings…", A::Settings),
            Entry::Separator,
            a("Save All", A::SaveAll),
            a("Reload All from Disk", A::ReloadFromDisk),
            Entry::Separator,
            Entry::Item { label: "Exit".into(), shortcut: String::new(), enabled: true, run: Rc::new(|this, _, cx| {
                this.save_all(cx);
                cx.quit();
            }) },
        ];
        // Editor groups give the editor the focus first, so the action applies to it.
        let group = |groups: &[&str], editor: bool| -> Vec<Entry> {
            let mut v = Vec::new();
            for (n, g) in groups.iter().enumerate() {
                if n > 0 {
                    v.push(Entry::Separator);
                }
                v.extend(ALL_ACTIONS.iter().filter(|x| x.info().group == *g).map(|x| self.action_entry(x.info().title, *x, true, editor)));
            }
            v
        };
        let sub = |label: &str, entries: Vec<Entry>| Entry::Submenu { label: label.into(), entries };
        vec![
            sub("File", file),
            sub("Edit", group(&["Edit", "Find"], true)),
            sub("View", {
                // View › Active Editor, as in PyCharm (Show Whitespaces).
                let mut v = group(&["View"], false);
                v.push(sub("Active Editor", group(&["Active Editor"], true)));
                v.push(Entry::Separator);
                v.extend(group(&["Tool Windows"], false));
                v
            }),
            sub("Navigate", group(&["Navigate"], false)),
            sub("Code", group(&["Code"], true)),
            sub("Refactor", group(&["Refactor"], true)),
            sub("Run", group(&["Run"], false)),
            sub("Git", group(&["Git"], false)),
            sub("Tools", group(&["Database"], false)),
            sub("Window", group(&["Window"], false)),
        ]
    }

    /// File > Open File…: files open in tabs of this project; a picked folder is a project.
    pub(super) fn open_file_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Open".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    for p in paths {
                        if p.is_dir() {
                            this.project_cmd(super::project_ui::ProjectCmd::Go(p, true), window, cx);
                        } else {
                            this.open_file(&p, window, cx);
                        }
                    }
                });
            }
        })
        .detach();
    }
}
