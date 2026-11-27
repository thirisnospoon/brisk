//! The smoke tests and screenshot helpers of `docs/testing.md`: the environment names one, and
//! it starts once the window is up. Compiled only with the `smoke` feature.

use super::*;

mod actions;
mod ansi;
mod bars;
mod big;
mod complete;
mod console_find;
mod dataview;
mod edit;
mod edit2;
mod flood;
mod git;
mod gutter;
mod hang;
mod intel;
mod keymap;
mod latency;
mod marks;
mod membudget;
mod motion;
mod mouse;
mod multi;
mod node;
mod open;
mod packages;
mod picker;
mod pin;
mod pycon;
mod pyproj;
mod reformat;
mod run_term;
mod run_tpl;
mod scope;
mod scroll;
mod space;
mod squiggle;
mod ssh;
mod stripe;
mod test_runners;
mod test_view;
mod tour;
mod ts;
mod vcs;
mod w7;
mod w7_2;
mod web;
mod webnav;
mod whitespace;
pub(super) mod win_input;

/// Starts every smoke or helper the environment asks for. Each runs deferred, after the
/// workspace's own start-up work has been queued.
pub(super) fn schedule(window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(f) = std::env::var_os("BRISK_SMOKE_LINT") {
        let f = PathBuf::from(f);
        cx.defer_in(window, move |this: &mut Workspace, window, cx| this.lint_smoke(f, window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_TEMPLATES").is_some() {
        cx.defer_in(window, move |this: &mut Workspace, window, cx| this.templates_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_BARS").is_some() {
        cx.defer_in(window, move |this: &mut Workspace, window, cx| this.bars_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_GUTTER").is_some() {
        cx.defer_in(window, move |this: &mut Workspace, window, cx| this.gutter_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_MOUSE").is_some() {
        cx.defer_in(window, move |this: &mut Workspace, window, cx| this.mouse_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_W7").is_some() {
        cx.defer_in(window, move |this: &mut Workspace, window, cx| this.w7_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_SCOPE").is_some() {
        cx.defer_in(window, move |this: &mut Workspace, window, cx| this.scope_smoke(window, cx));
    }
    if let Some(f) = std::env::var_os("BRISK_SMOKE_BIG") {
        let f = PathBuf::from(f);
        cx.defer_in(window, move |this: &mut Workspace, window, cx| this.big_smoke(f, window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_COMPLETE").is_some() {
        cx.defer_in(window, move |this: &mut Workspace, window, cx| this.complete_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_INTEL").is_some() {
        cx.defer_in(window, move |this: &mut Workspace, window, cx| this.intel_smoke(window, cx));
    }
    if let Some(f) = std::env::var_os("BRISK_SMOKE_LSP") {
        let f = PathBuf::from(f);
        cx.defer_in(window, move |this: &mut Workspace, window, cx| this.lsp_smoke(f, window, cx));
    }
    if let Some(f) = std::env::var_os("BRISK_SMOKE_EDIT") {
        let f = PathBuf::from(f);
        cx.defer_in(window, move |this: &mut Workspace, window, cx| this.edit_smoke(f, window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_EDIT2").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.edit2_smoke(window, cx));
    }
    // Screenshot helpers: open files (a `;`-separated list) and/or the Settings dialog at start.
    if let Some(f) = std::env::var_os("BRISK_OPEN_FILE") {
        let files: Vec<PathBuf> = std::env::split_paths(&f).collect();
        cx.defer_in(window, move |this: &mut Workspace, window, cx| {
            for f in &files {
                this.open_file(f, window, cx);
            }
        });
    }
    // Screenshot helper: the Structure window, with the caret on line N (1-based) of the file.
    if let Some(line) = std::env::var("BRISK_OPEN_STRUCTURE").ok().and_then(|l| l.parse::<usize>().ok()) {
        cx.defer_in(window, move |this: &mut Workspace, window, cx| {
            this.toggle_left(LeftTool::Structure, window, cx);
            if let Some(ed) = this.active_editor().cloned() {
                ed.update(cx, |e, cx| e.go_to_line(line.saturating_sub(1), cx));
            }
        });
    }
    if std::env::var_os("BRISK_OPEN_SETTINGS").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| {
            this.open_settings(window);
            cx.notify();
        });
    }
    if let Some(f) = std::env::var_os("BRISK_SMOKE_DATAVIEW") {
        let f = PathBuf::from(f);
        cx.defer_in(window, move |this: &mut Workspace, window, cx| this.dataview_smoke(f, window, cx));
    }
    if let Some(f) = std::env::var_os("BRISK_SMOKE_DEBUG") {
        let f = PathBuf::from(f);
        cx.defer_in(window, move |this: &mut Workspace, window, cx| this.debug_smoke(f, window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_SETTINGS").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.settings_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_ACTIONS").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.actions_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_DISK").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.disk_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_UI").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.ui_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_RUNCFG").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.run_dialog_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_RUNMARKS").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.run_marks_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_RUNTPL").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.run_tpl_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_TESTRUNNERS").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.test_runners_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_STICKY").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.sticky_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_WHITESPACE").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.whitespace_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_SQUIGGLE").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.squiggle_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_STRIPE").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.stripe_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_PALETTE").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.palette_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_MOTION").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.motion_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_FLOOD").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.flood_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_LATENCY").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.latency_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_ANSI").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.ansi_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_TESTVIEW").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.test_view_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_RUNTERM").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.run_term_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_PYCON").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.pycon_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_MULTI").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.multi_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_PIN").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.pin_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_SERVICES").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.services_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_CONSOLEFIND").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.console_find_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_WEB").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.web_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_BEFORE").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.before_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_PROFILE").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.profile_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_COVERAGE").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.coverage_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_DEBUGPY_OFFER").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.debugpy_offer_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_COMPOUND").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.compound_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_ENVFILE").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.envfile_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_TERMVENV").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.terminal_env_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_RUNANYTHING").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.run_anything_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_SPACE").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.space_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_UPDATE").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.update_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_NODE").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.node_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_REFORMAT").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.reformat_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_TS").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.ts_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_PICKER").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.picker_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_TOUR").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.tour_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_KEYMAP").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.keymap_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_MARKS").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.marks_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_SSH").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.ssh_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_WEBNAV").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.webnav_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_VENV").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.venv_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_PYCONSOLE").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.python_console_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_PKGQUEUE").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.python_packages_queue_smoke(window, cx));
    }
    if let Ok(which) = std::env::var("BRISK_SMOKE_PYPROJ") {
        cx.defer_in(window, move |this: &mut Workspace, window, cx| this.pyproj_smoke(which, window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_PACKAGES").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.python_packages_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_TODO").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.todo_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_OPEN").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.open_smoke(window, cx));
    } else if crate::stall::frames_enabled() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.list_scroll_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_GITX").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.gitx_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_VCS").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.vcs_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_RUNWIN").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.run_window_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_HANG").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.hang_smoke(window, cx));
    }
    if std::env::var_os("BRISK_SMOKE_MEMBUDGET").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.membudget_smoke(window, cx));
    }
    if let Some(f) = std::env::var_os("BRISK_SMOKE_RUN") {
        let f = PathBuf::from(f);
        cx.defer_in(window, move |this: &mut Workspace, window, cx| this.run_smoke(f, window, cx));
    }
    if let Some(sql) = std::env::var_os("BRISK_SMOKE_DB") {
        let sql = sql.to_string_lossy().into_owned();
        cx.defer_in(window, move |this: &mut Workspace, window, cx| this.db_smoke(sql, window, cx));
    }
    if std::env::var_os("BRISK_OPEN_TERMINAL").is_some() {
        cx.defer_in(window, |this: &mut Workspace, window, cx| this.toggle_terminal(window, cx));
    }
    // Smoke test hook: Find in Files for a query, then show results in the Find window.
    if let Some(q) = std::env::var_os("BRISK_SMOKE_FIND") {
        let q = q.to_string_lossy().into_owned();
        cx.defer_in(window, move |this: &mut Workspace, window, cx| {
            this.find.set_query_for_smoke(&q);
            this.open_find(false, window, cx);
            let bg = cx.background_executor().clone();
            let t0 = Instant::now();
            cx.spawn_in(window, async move |this, cx| {
                for _ in 0..120 {
                    bg.timer(std::time::Duration::from_millis(250)).await;
                    let done = this.update(cx, |this, _| this.files.is_some() && this.find.idle()).unwrap_or(true);
                    if done {
                        break;
                    }
                }
                let _ = this.update_in(cx, |this, window, cx| {
                    eprintln!(
                        "smoke-find: {} (since start {:.1?}, files indexed {})",
                        this.find.summary(),
                        t0.elapsed(),
                        this.files.as_ref().map_or(0, |f| f.files.len())
                    );
                    this.dispatch_find_window_for_smoke(window, cx);
                });
            })
            .detach();
        });
    }
    // Smoke test hook: open a file and exercise the git windows (used by scripts, no UI driving).
    if let Some(f) = std::env::var_os("BRISK_SMOKE_GIT") {
        let f = PathBuf::from(f);
        cx.defer_in(window, move |this: &mut Workspace, window, cx| {
            this.open_file(&f, window, cx);
            this.dispatch(Action::ToggleCommitWindow, window, cx);
            this.dispatch(Action::ToggleGitWindow, window, cx);
            this.focus_editor(window, cx);
            this.dispatch(Action::ShowDiff, window, cx);
            this.dispatch(Action::Branches, window, cx);
        });
    }
}
