//! Brisk — a lightweight IDE with PyCharm's layout, keymap and behavior.
//!
//! Usage: `brisk [project-dir | file]`: a file opens in the window of its project when one
//! runs, else in a new window of its project folder; nothing reopens the last project.

#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod crash;
mod data_view;
mod diff_view;
mod editor;
mod grid_view;
mod icons;
mod keymap_store;
mod keys;
mod merge_view;
mod scrollbar;
mod smoke;
mod stall;
mod terminal_view;
mod theme;
mod ui_kit;
mod window_state;
mod workspace;

use gpui::{App, AppContext, Application, TitlebarOptions, WindowOptions};
use std::path::PathBuf;
use workspace::Workspace;

fn main() {
    workspace::budget_probe::mark("main");
    crash::install();
    // Strip the Windows verbatim prefix so paths display normally.
    let clean = |p: PathBuf| {
        let p = std::fs::canonicalize(&p).unwrap_or(p);
        PathBuf::from(p.to_string_lossy().trim_start_matches(r"\\?\").to_string())
    };
    let arg = std::env::args().nth(1).map(PathBuf::from).map(clean);
    // "Edit with Brisk" and `brisk file.py`: a window that already has the project takes it.
    if let Some(a) = arg.as_ref().filter(|_| !smoke::active()) {
        if brisk_platform::instance::hand_over(a) {
            return;
        }
    }
    let (root, initial_file) = match arg {
        Some(f) if f.is_file() => (brisk_platform::instance::project_root_of(&f), Some(f)),
        Some(d) => (d, None),
        // Without an argument, reopen the last project like PyCharm; the current directory otherwise.
        None => (clean(workspace::project_ui::recent_projects().into_iter().next().unwrap_or_else(|| std::env::current_dir().expect("no current dir"))), None),
    };

    workspace::project_ui::remember_project(&root);
    // Smokes never reach the network: pip gets no index of its own, PyPI names only from a file.
    brisk_run::env::set_offline(smoke::active());
    brisk_settings::load();
    workspace::budget_probe::mark("settings");
    // Local History: old revisions and the over-budget oldest go, once per start, off the UI.
    std::thread::Builder::new()
        .name("local-history-cleanup".into())
        .spawn(|| {
            brisk_history::cleanup();
        })
        .ok();

    Application::new().with_assets(icons::Assets).run(move |cx: &mut App| {
        workspace::budget_probe::mark("app_run");
        // Fonts shipped next to the exe (or in BRISK_FONT_DIR): .ttf / .otf files are added to
        // the text system, so the UI does not depend on what Windows has installed.
        for dir in brisk_platform::fonts::extra_dirs() {
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
        let window_bounds = window_state::initial(cx);
        let title = format!("{} – Brisk", root.file_name().map(|n| n.to_string_lossy()).unwrap_or_default());
        let options = WindowOptions {
            window_bounds: Some(window_bounds),
            // On Windows the toolbar is the title bar, as in PyCharm's new UI: it drags the window
            // and carries the minimize, maximize and close buttons (`Workspace::render_toolbar`).
            titlebar: Some(TitlebarOptions { title: Some(title.into()), appears_transparent: cfg!(windows), ..Default::default() }),
            ..Default::default()
        };
        #[cfg(feature = "smoke")]
        let options = smoke::window_options(options);
        let handle = cx.open_window(options, |window, cx| cx.new(|cx| Workspace::new(root.clone(), window, cx))).expect("failed to open window");
        if let Some(f) = initial_file.clone() {
            let _ = handle.update(cx, |ws, window, cx| ws.open_file(&f, window, cx));
        }
        // Files and folders handed over by later launches (see brisk_platform::instance).
        if !smoke::active() {
            let (tx, mut rx) = futures::channel::mpsc::unbounded();
            brisk_platform::instance::serve(&root, move |r| {
                let _ = tx.unbounded_send(r);
            });
            cx.spawn(async move |cx| {
                use futures::StreamExt;
                while let Some(r) = rx.next().await {
                    let _ = handle.update(cx, |ws, window, cx| {
                        if let brisk_platform::instance::Request::Open(p) = &r {
                            if p.is_file() {
                                ws.open_file(p, window, cx);
                            }
                        }
                        window.activate_window();
                    });
                }
            })
            .detach();
        }
        #[cfg(feature = "smoke")]
        smoke::window_opened();
        workspace::budget_probe::mark("window_opened");
        cx.activate(true);
        stall::start(cx);
    });
}
