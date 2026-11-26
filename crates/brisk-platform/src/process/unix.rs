use std::process::{Child, Command};

pub fn no_window(c: &mut Command) -> &mut Command {
    c
}

pub fn adopt(_: &Child) {}

pub fn adopt_pid(_: u32) {}

/// `Child::kill`, which the caller does next, is all there is for now.
pub fn kill_tree(_: &mut Child) {}

pub fn memory() -> Option<(u64, u64)> {
    None
}

pub fn live_children() -> Option<u32> {
    None
}

/// From `/proc` on Linux; empty elsewhere.
pub fn processes() -> Vec<super::ProcessInfo> {
    let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
    dir.flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(e.path().join("stat")).ok()?;
            // `pid (comm) state ppid …`; comm may hold spaces and parentheses.
            let (head, rest) = stat.rsplit_once(')')?;
            let name = head.split_once('(')?.1.to_string();
            let parent = rest.split_whitespace().nth(1)?.parse().ok()?;
            Some(super::ProcessInfo { pid, parent, name, working_set: memory_of(pid).map_or(0, |m| m.0) })
        })
        .collect()
}

/// `VmRSS` and `RssAnon` of `/proc/<pid>/status`.
pub fn memory_of(pid: u32) -> Option<(u64, u64)> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let kb = |key: &str| status.lines().find_map(|l| l.strip_prefix(key)?.trim().strip_suffix("kB")?.trim().parse::<u64>().ok()).map(|k| k * 1024);
    Some((kb("VmRSS:")?, kb("RssAnon:").unwrap_or(0)))
}

pub fn start_unix_ms(_: &Child) -> Option<u64> {
    None
}

pub fn has_window(_: u32) -> bool {
    false
}
