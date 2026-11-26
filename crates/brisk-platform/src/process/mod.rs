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

/// A process on this machine, as the size and memory budget samples it (`cargo xtask budget`).
#[derive(Clone, Debug)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent: u32,
    /// The executable's file name (`ty.exe`, `ruff`).
    pub name: String,
    /// Bytes; 0 when the process cannot be opened.
    pub working_set: u64,
}

/// Every process the current user can see, with its parent.
pub fn processes() -> Vec<ProcessInfo> {
    sys::processes()
}

/// `pid` and every process it started, directly or not (`pid` itself left out).
pub fn descendants(all: &[ProcessInfo], pid: u32) -> Vec<ProcessInfo> {
    let mut ids = vec![pid];
    let mut out: Vec<ProcessInfo> = Vec::new();
    loop {
        let more: Vec<ProcessInfo> = all.iter().filter(|p| ids.contains(&p.parent) && !ids.contains(&p.pid)).cloned().collect();
        if more.is_empty() {
            return out;
        }
        ids.extend(more.iter().map(|p| p.pid));
        out.extend(more);
    }
}

/// Working set and private bytes of another process, in bytes.
pub fn memory_of(pid: u32) -> Option<(u64, u64)> {
    sys::memory_of(pid)
}

/// When `child` was created, in milliseconds since the Unix epoch (as the system recorded it,
/// so the time spent in the spawn call itself does not count). `None` where it is not known.
pub fn start_unix_ms(child: &Child) -> Option<u64> {
    sys::start_unix_ms(child)
}

/// Whether `pid` has a visible top-level window of its own (`MainWindowHandle` in .NET).
/// Always false where windows are not tracked per process.
pub fn has_window(pid: u32) -> bool {
    sys::has_window(pid)
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

    #[test]
    fn descendants_follow_the_tree() {
        let p = |pid, parent| super::ProcessInfo { pid, parent, name: String::new(), working_set: 0 };
        let all = [p(1, 0), p(2, 1), p(3, 2), p(4, 1), p(5, 9)];
        let mut d: Vec<u32> = super::descendants(&all, 1).iter().map(|p| p.pid).collect();
        d.sort();
        assert_eq!(d, [2, 3, 4]);
        assert!(super::descendants(&all, 5).is_empty());
    }

    #[test]
    fn a_child_is_listed_with_its_memory_and_start() {
        #[cfg(windows)]
        {
            let before = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
            let mut c = std::process::Command::new("cmd").args(["/C", "ping -n 3 127.0.0.1 >NUL"]).spawn().unwrap();
            let all = super::processes();
            let me = std::process::id();
            let kids = super::descendants(&all, me);
            assert!(kids.iter().any(|k| k.pid == c.id() && k.name.eq_ignore_ascii_case("cmd.exe")), "{kids:?}");
            assert!(super::memory_of(c.id()).is_some_and(|(ws, _)| ws > 0));
            let start = super::start_unix_ms(&c).unwrap();
            assert!(start + 1000 >= before && start <= before + 10_000, "{start} vs {before}");
            assert!(!super::has_window(c.id()));
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}
