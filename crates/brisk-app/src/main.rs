//! Brisk — a lightweight IDE with PyCharm's layout, keymap and behavior.
//!
//! Usage: `brisk [project-dir]` (defaults to the current directory).

#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod crash;
mod stall;
mod diff_view;
mod merge_view;
mod grid_view;
mod data_view;
mod icons;
mod ui_kit;
mod editor;
mod editorconfig;
mod keys;
mod local_history;
mod py_console;
mod py_packages;
mod py_manager;
mod py_venv;
mod run_anything;
mod terminal_env;
mod coverage;
mod profile;
mod todo_scan;
mod terminal_view;
mod scrollbar;
mod settings;
mod theme;
mod workspace;

use gpui::{px, size, App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions};
use std::path::PathBuf;
use workspace::Workspace;

fn main() {
    workspace::budget_probe::mark("main");
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
    workspace::budget_probe::mark("settings");
    // Local History: old revisions and the over-budget oldest go, once per start, off the UI.
    std::thread::Builder::new().name("local-history-cleanup".into()).spawn(|| {
        local_history::cleanup();
    }).ok();

    Application::new().with_assets(icons::Assets).run(move |cx: &mut App| {
        workspace::budget_probe::mark("app_run");
        // Fonts shipped next to the exe (or in BRISK_FONT_DIR): .ttf / .otf files are added to
        // the text system, so the UI does not depend on what Windows has installed.
        for dir in fonts_dirs() {
            let fonts: Vec<std::borrow::Cow<'static, [u8]>> = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ttf") || x.eq_ignore_ascii_case("otf")))
                .filter_map(|p| std::fs::read(p).ok())
                .map(std::borrow::Cow::Owned)
                .collect();
            if !fonts.is_empty() {
                let _ = cx.text_system().add_fonts(fonts);
            }
        }
        workspace::budget_probe::mark("fonts");
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
        workspace::budget_probe::mark("window_opened");
        cx.activate(true);
        stall::start(cx);
    });
}

/// Folders whose fonts Brisk loads at start: `BRISK_FONT_DIR`, then `fonts` next to the exe.
fn fonts_dirs() -> Vec<std::path::PathBuf> {
    let mut v: Vec<std::path::PathBuf> = std::env::var_os("BRISK_FONT_DIR").map(Into::into).into_iter().collect();
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join("fonts"))) {
        v.push(d);
    }
    v
}
