//! What production code needs to know about the smoke tests of `docs/testing.md`. Without the
//! `smoke` feature there are none, and every answer here is no. The smokes themselves are in
//! `workspace::smoke`.

/// Whether the smoke variable `name` (`BRISK_SMOKE_...`) is set.
#[cfg(feature = "smoke")]
pub fn on(name: &str) -> bool {
    std::env::var_os(name).is_some()
}

#[cfg(not(feature = "smoke"))]
#[inline(always)]
pub fn on(_name: &str) -> bool {
    false
}

/// Whether a smoke drives this process: pip stays offline and no browser is opened.
#[cfg(feature = "smoke")]
pub fn active() -> bool {
    std::env::vars_os().any(|(k, _)| k.to_string_lossy().starts_with("BRISK_SMOKE_"))
}

#[cfg(not(feature = "smoke"))]
#[inline(always)]
pub fn active() -> bool {
    false
}
