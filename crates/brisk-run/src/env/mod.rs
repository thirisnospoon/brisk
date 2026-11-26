//! The project's Python environment: creating a venv and installing its requirements
//! (`venv`), uv and Poetry (`manager`), the packages of an interpreter (`packages`) and the
//! Terminal's activation of it (`terminal`). Calls that start a process block: run them in the
//! background.

pub mod manager;
pub mod packages;
pub mod terminal;
pub mod venv;

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

static OFFLINE: AtomicBool = AtomicBool::new(false);

/// Keep pip and the PyPI name list off the network: pip gets no index unless `PIP_INDEX_URL`
/// names one, and only a `file://` `BRISK_PYPI_INDEX` is read. The smokes turn it on.
pub fn set_offline(offline: bool) {
    OFFLINE.store(offline, Ordering::Relaxed);
}

fn offline() -> bool {
    OFFLINE.load(Ordering::Relaxed)
}

/// `PIP_NO_INDEX=1` on `c` while offline and no `PIP_INDEX_URL` is set. Every pip call made
/// here goes through it; a caller that starts pip itself can use it too.
pub fn pip_offline(c: &mut Command) {
    if offline() && std::env::var_os("PIP_INDEX_URL").is_none() {
        c.env("PIP_NO_INDEX", "1");
    }
}

/// `program` (an interpreter or uv) with UTF-8 output, pip without prompts or version checks,
/// and no console window.
fn pip_command(program: &Path) -> Command {
    let mut c = Command::new(program);
    c.env("PYTHONIOENCODING", "utf-8").env("PIP_DISABLE_PIP_VERSION_CHECK", "1").env("PIP_NO_INPUT", "1");
    pip_offline(&mut c);
    brisk_platform::process::no_window(&mut c);
    c
}
