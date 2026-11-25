//! Everything that differs between operating systems, behind one API per concern
//! (docs/architecture.md, `brisk-platform`). Each module keeps its Windows code in a
//! `windows` submodule selected by one `#[cfg]` on the `mod` line; callers never need a
//! `cfg` of their own.

pub mod process;
