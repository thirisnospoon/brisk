//! Child processes. Every long-lived child Brisk starts (ty, ruff, debugpy, runs, terminals,
//! consoles, git) is adopted into a group that dies with Brisk, and none of them opens a
//! console window.
//!
//! On Windows the group is one Job Object created with `KILL_ON_JOB_CLOSE`. Only Brisk holds
//! the job handle, so when Brisk exits for any reason (quit, crash, `taskkill`, project
//! switch) Windows kills the whole job, grandchildren included. Brisk itself is not in the
//! job, so a browser or a new Brisk window it opens is unaffected.
//!
//! Elsewhere `adopt` is a no-op: Unix children already get their own process group and are
//! signalled on stop.

use std::process::{Child, Command};

#[cfg_attr(windows, path = "windows.rs")]
#[cfg_attr(not(windows), path = "unix.rs")]
mod sys;

/// A `Command` for `program` that opens no console window.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(program);
    no_window(&mut c);
    c
}

/// Keeps `c` from opening a console window (`CREATE_NO_WINDOW` on Windows).
pub fn no_window(c: &mut Command) -> &mut Command {
    sys::no_window(c)
}

/// Put a freshly spawned child into the group.
pub fn adopt(child: &Child) {
    sys::adopt(child)
}

/// Same for a child known only by pid (the ConPTY shell of a terminal).
pub fn adopt_pid(pid: u32) {
    sys::adopt_pid(pid)
}

/// Kill a child and every process it started (`Child::kill` stops only the child, leaving
/// e.g. debugpy's launcher or a console's subprocesses running). The caller holds the
/// child's handle, so its pid cannot have been reused yet.
pub fn kill_tree(child: &mut Child) {
    sys::kill_tree(child);
    let _ = child.kill();
}

/// Number of live processes in the group (for the memory/handle smoke). `None` where it is
/// not known, or before any child was adopted.
pub fn live_children() -> Option<u32> {
    sys::live_children()
}

/// This process's working set and private bytes, in bytes.
pub fn memory() -> Option<(u64, u64)> {
    sys::memory()
}

#[cfg(test)]
mod tests {
    #[test]
    fn adopted_child_is_counted() {
        #[cfg(windows)]
        {
            let mut c = std::process::Command::new("cmd").args(["/C", "ping -n 3 127.0.0.1 >NUL"]).spawn().unwrap();
            super::adopt(&c);
            assert!(super::live_children().unwrap() >= 1);
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// The grandchild (ping under cmd) dies too: it holds the stdout pipe, so EOF comes only
    /// once it is gone.
    #[test]
    fn kill_tree_takes_grandchildren() {
        #[cfg(windows)]
        {
            use std::io::Read;
            let mut c = std::process::Command::new("cmd").args(["/C", "ping -n 30 127.0.0.1"]).stdout(std::process::Stdio::piped()).spawn().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(500));
            let mut out = c.stdout.take().unwrap();
            let t = std::time::Instant::now();
            super::kill_tree(&mut c);
            let _ = c.wait();
            let mut sink = Vec::new();
            let _ = out.read_to_end(&mut sink);
            assert!(t.elapsed() < std::time::Duration::from_secs(10), "grandchild survived");
        }
    }

    #[test]
    fn command_runs() {
        let program = if cfg!(windows) { "cmd" } else { "sh" };
        let arg = if cfg!(windows) { "/C" } else { "-c" };
        let out = super::command(program).args([arg, "echo hi"]).output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("hi"));
    }
}
