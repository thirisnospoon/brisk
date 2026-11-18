//! Main-thread stall watchdog (debug builds, `BRISK_WATCHDOG=1`, and `BRISK_SMOKE_HANG`).
//!
//! The UI thread bumps a heartbeat every [`TICK`]; a watcher thread notices when the heartbeat
//! stops for longer than [`REPORT`] and, once it moves again, logs the stall to stderr with
//! the last [`note`] (the action or phase the UI thread was in when it stopped). Off by
//! default in release builds: the tick would wake an idle editor 20 times a second.

use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(50);
/// Stalls shorter than this are not logged (a 4K repaint or a font cache fill can take ~100 ms).
pub const REPORT: Duration = Duration::from_millis(200);

static NOTE: Mutex<Cow<'static, str>> = Mutex::new(Cow::Borrowed("startup"));
static HEARTBEAT_MS: AtomicU64 = AtomicU64::new(0);
static MAX_MS: AtomicU64 = AtomicU64::new(0);
static MAX_NOTE: Mutex<String> = Mutex::new(String::new());
static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn now_ms() -> u64 {
    ORIGIN.get_or_init(Instant::now).elapsed().as_millis() as u64
}

pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| cfg!(debug_assertions) || std::env::var_os("BRISK_WATCHDOG").is_some() || std::env::var_os("BRISK_SMOKE_HANG").is_some())
}

/// What the UI thread is about to do. Cheap; call it before anything that might be slow.
pub fn note(what: impl Into<Cow<'static, str>>) {
    if let Ok(mut n) = NOTE.lock() {
        *n = what.into();
    }
}

/// Longest stall since the last [`reset`], with its note.
pub fn worst() -> (u64, String) {
    (MAX_MS.load(Ordering::Relaxed), MAX_NOTE.lock().map(|s| s.clone()).unwrap_or_default())
}

pub fn reset() {
    MAX_MS.store(0, Ordering::Relaxed);
    if let Ok(mut s) = MAX_NOTE.lock() {
        s.clear();
    }
}

pub fn start(cx: &mut gpui::App) {
    if !enabled() {
        return;
    }
    HEARTBEAT_MS.store(now_ms(), Ordering::Relaxed);
    let bg = cx.background_executor().clone();
    cx.spawn(async move |_| loop {
        bg.timer(TICK).await;
        HEARTBEAT_MS.store(now_ms(), Ordering::Relaxed);
    })
    .detach();
    let _ = std::thread::Builder::new().name("stall-watchdog".into()).spawn(|| {
        let mut seen = HEARTBEAT_MS.load(Ordering::Relaxed);
        let mut stalled_on: Option<String> = None;
        loop {
            std::thread::sleep(Duration::from_millis(20));
            let hb = HEARTBEAT_MS.load(Ordering::Relaxed);
            let now = now_ms();
            if hb == seen {
                // Capture the note while the thread is still stuck in it.
                if stalled_on.is_none() && now.saturating_sub(hb) > REPORT.as_millis() as u64 {
                    stalled_on = Some(NOTE.lock().map(|n| n.to_string()).unwrap_or_default());
                }
                continue;
            }
            let gap = hb.saturating_sub(seen).saturating_sub(TICK.as_millis() as u64);
            seen = hb;
            if let Some(what) = stalled_on.take() {
                if gap > REPORT.as_millis() as u64 {
                    eprintln!("stall: main thread blocked {gap} ms during '{what}'");
                    if gap > MAX_MS.load(Ordering::Relaxed) {
                        MAX_MS.store(gap, Ordering::Relaxed);
                        if let Ok(mut s) = MAX_NOTE.lock() {
                            *s = what;
                        }
                    }
                }
            }
        }
    });
}
