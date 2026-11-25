//! Everything that differs between operating systems, behind one API per concern
//! (docs/architecture.md, `brisk-platform`). Each module keeps its Windows code in a
//! `windows` submodule selected by one `#[cfg]` on the `mod` line, or, where the difference
//! is a value rather than an API (a venv layout), in plain code tested on every OS. Callers
//! never need a `cfg` of their own.

pub mod dirs;
pub mod fonts;
pub mod fs;
pub mod process;
pub mod python;
pub mod shell;
