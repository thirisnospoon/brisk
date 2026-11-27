//! Brisk's setup for Windows. `Brisk-<version>-setup.exe` carries the release zip and
//! installs or updates Brisk for the current user; the same exe without the zip, copied to
//! the install folder as `uninstall.exe`, removes it (or, with `--modify`, changes the
//! options). `--silent` does either without a window, with the defaults or the current
//! options (an uninstall keeps the settings).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ops;

mod ui;

use gpui::{px, size, App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions};
use std::path::PathBuf;
use ui::{Mode, Setup};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let value = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
    let exe = std::env::current_exe().ok();
    let carries = exe.as_deref().and_then(brisk_platform::install::payload::find).is_some();
    let mode = if flag("--modify") {
        Mode::Modify
    } else if carries && !flag("--uninstall") {
        Mode::Install
    } else {
        Mode::Uninstall
    };
    let installed = brisk_platform::install::installed();

    // The uninstaller cannot delete the folder it runs from: it runs from a temp copy.
    if mode == Mode::Uninstall {
        let Some(dir) = installed.as_ref().map(|i| i.dir.clone()) else {
            if flag("--silent") {
                return;
            }
            return show(Mode::Uninstall);
        };
        if let Some(copy) = ops::relaunch_copy(&dir) {
            let mut c = std::process::Command::new(&copy);
            c.args(&args);
            if c.spawn().is_ok() {
                return;
            }
        }
        // Running as the temp copy: Windows deletes it at the next restart.
        if let Some(e) = &exe {
            if e.parent().is_some_and(|p| p == std::env::temp_dir().as_path() || p.starts_with(std::env::temp_dir())) {
                brisk_platform::install::delete_on_reboot(e);
            }
        }
    }

    if flag("--silent") {
        let quiet = |_: f32, _: &str| {};
        let res = match mode {
            Mode::Install => {
                let dir = value("--dir").map(PathBuf::from).or_else(|| installed.as_ref().map(|i| i.dir.clone())).or_else(brisk_platform::install::default_dir);
                let options = installed.as_ref().map(|i| i.options.clone()).unwrap_or_default();
                match dir {
                    Some(d) => ops::install(&d, &options, &quiet),
                    None => Err("no install folder".into()),
                }
            }
            Mode::Modify => Ok(()),
            Mode::Uninstall => match &installed {
                Some(i) => ops::uninstall(&i.dir, flag("--delete-settings"), &quiet),
                None => Ok(()),
            },
        };
        std::process::exit(if res.is_ok() { 0 } else { 1 });
    }
    show(mode);
}

fn show(mode: Mode) {
    Application::new().run(move |cx: &mut App| {
        // 880 x 560, or smaller when the display's work area is: the layout goes compact.
        let (mut w, mut h) = (880f32, 560f32);
        if let Some(d) = cx.primary_display() {
            let b = d.bounds().size;
            w = w.min(f32::from(b.width) - 40.).max(640.);
            h = h.min(f32::from(b.height) - 90.).max(440.);
        }
        let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(if mode == Mode::Uninstall { "Uninstall Brisk".into() } else { "Brisk Setup".into() }),
                appears_transparent: true,
                ..Default::default()
            }),
            is_resizable: false,
            ..Default::default()
        };
        cx.open_window(options, |_, cx| cx.new(|cx| Setup::new(mode, cx))).expect("cannot open the setup window");
        cx.on_window_closed(|cx| cx.quit()).detach();
        cx.activate(true);
    });
}
