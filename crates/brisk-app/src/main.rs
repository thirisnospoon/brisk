//! Brisk — a lightweight IDE with PyCharm's layout, keymap and behavior.
//!
//! Usage: `brisk [project-dir]` (defaults to the current directory).

#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod crash;
mod diff_view;
mod grid_view;
mod editor;
mod editorconfig;
mod keys;
mod local_history;
mod py_console;
mod py_packages;
mod py_venv;
mod run_anything;
mod terminal_env;
mod coverage;
mod profile;
mod todo_scan;
mod terminal_view;
mod settings;
mod theme;
mod workspace;

use gpui::{px, size, App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions};
use std::path::PathBuf;
use workspace::Workspace;

fn main() {
    crash::install();
    // Without an argument, reopen the last project like PyCharm; the current directory otherwise.
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| workspace::project_ui::recent_projects().into_iter().next())
        .unwrap_or_else(|| std::env::current_dir().expect("no current dir"));
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    // Strip the Windows verbatim prefix so paths display normally.
    let root = PathBuf::from(root.to_string_lossy().trim_start_matches(r"\\?\").to_string());

    workspace::project_ui::remember_project(&root);
    settings::load();

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);
        let title = format!("{} – Brisk", root.file_name().map(|n| n.to_string_lossy()).unwrap_or_default());
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions { title: Some(title.into()), ..Default::default() }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Workspace::new(root.clone(), window, cx)),
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
