// macOS: the system font and Menlo. Elsewhere the fonts most distributions install.
pub const UI: &str = if cfg!(target_os = "macos") { ".SystemUIFont" } else { "Noto Sans" };
pub const MONO: &str = if cfg!(target_os = "macos") { "Menlo" } else { "DejaVu Sans Mono" };
