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
