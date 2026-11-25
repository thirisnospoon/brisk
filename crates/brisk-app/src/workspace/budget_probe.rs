//! `BRISK_BUDGET=<report.json>`, for `scripts\budget.ps1`: records named startup moments and
//! when the first window frame was drawn (wall clock; the script subtracts the process start),
//! then after 5 s idle (`BRISK_BUDGET_IDLE_MS`) the working set, holds a moment so the script can
//! sample the child processes (`BRISK_BUDGET_HOLD`, ms) and exits.
//! `BRISK_BUDGET_STARTUP_ONLY=1` exits right after the first frame.
//! The report is rewritten at each stage (`"stage"`), so the script can follow it.

use super::Workspace;
use gpui::{Context, Window};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

const IDLE_MS: u64 = 5000;

static FIRST_RENDER_MS: AtomicU64 = AtomicU64::new(0);
static FIRST_FRAME_MS: AtomicU64 = AtomicU64::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);

fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn report_path() -> Option<PathBuf> {
    static P: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    P.get_or_init(|| std::env::var_os("BRISK_BUDGET").map(PathBuf::from)).clone()
}

static MARKS: std::sync::Mutex<Vec<(&'static str, u64)>> = std::sync::Mutex::new(Vec::new());

/// A named moment of startup (wall clock), reported with the first frame. No-op without
/// `BRISK_BUDGET`.
pub(crate) fn mark(name: &'static str) {
    if report_path().is_some() {
        if let Ok(mut m) = MARKS.lock() {
            m.push((name, unix_ms()));
        }
    }
}

/// Called at the start of every Workspace render: the first one schedules the first-frame mark.
pub(super) fn on_render(window: &mut Window) {
    if report_path().is_none() || ARMED.swap(true, Ordering::Relaxed) {
        return;
    }
    FIRST_RENDER_MS.store(unix_ms(), Ordering::Relaxed);
    window.on_next_frame(|_, _| FIRST_FRAME_MS.store(unix_ms(), Ordering::Relaxed));
}

fn write(path: &Path, fields: &[(&str, serde_json::Value)]) {
    let mut m = serde_json::Map::new();
    m.insert("pid".into(), std::process::id().into());
    m.insert("first_render_unix_ms".into(), FIRST_RENDER_MS.load(Ordering::Relaxed).into());
    m.insert("first_frame_unix_ms".into(), FIRST_FRAME_MS.load(Ordering::Relaxed).into());
    if let Ok(marks) = MARKS.lock() {
        let marks: serde_json::Map<String, serde_json::Value> = marks.iter().map(|(k, v)| ((*k).to_string(), (*v).into())).collect();
        m.insert("marks".into(), marks.into());
    }
    for (k, v) in fields {
        m.insert((*k).into(), v.clone());
    }
    // Written whole, then renamed: the script never reads half a file.
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, serde_json::Value::Object(m).to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

fn mb(b: u64) -> f64 {
    (b as f64 / 1024.0 / 1024.0 * 10.0).round() / 10.0
}

impl Workspace {
    pub(super) fn budget_probe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = report_path() else { return };
        let startup_only = std::env::var_os("BRISK_BUDGET_STARTUP_ONLY").is_some();
        let env_ms = |k: &str, d: u64| std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d);
        let idle = Duration::from_millis(env_ms("BRISK_BUDGET_IDLE_MS", IDLE_MS));
        let hold = Duration::from_millis(env_ms("BRISK_BUDGET_HOLD", 1500));
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |_, _| {
            let wait = |d: Duration| bg.timer(d);
            while FIRST_FRAME_MS.load(Ordering::Relaxed) == 0 {
                wait(Duration::from_millis(10)).await;
            }
            write(&path, &[("stage", "first_frame".into())]);
            if startup_only {
                wait(Duration::from_millis(300)).await;
                std::process::exit(0);
            }
            wait(idle).await;
            let (ws, private) = brisk_platform::process::memory().unwrap_or_default();
            let idle = [("idle_ws_mb", mb(ws).into()), ("idle_private_mb", mb(private).into())];
            write(&path, &[&idle[..], &[("stage", "idle".into())]].concat());
            wait(hold).await;
            write(&path, &[&idle[..], &[("stage", "done".into())]].concat());
            std::process::exit(0);
        })
        .detach();
    }
}
