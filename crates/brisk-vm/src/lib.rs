//! Brisk Engine: a small Linux VM on Windows Hypervisor Platform that runs Docker for Brisk,
//! without Docker Desktop or WSL. See `docs/design/containers.md`.
//!
//! This part answers whether the machine can run the VM and, when the Windows feature is off,
//! turns it on through one UAC prompt.

pub mod virt;

pub use virt::{check, enable, EnableOutcome, Virtualization};
