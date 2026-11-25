//! Where Brisk keeps its own files, and the per-user folders other tools use.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[cfg_attr(windows, path = "windows.rs")]
#[cfg_attr(not(windows), path = "unix.rs")]
mod sys;

/// Brisk's settings folder: the portable `data` folder if there is one, else `%APPDATA%\Brisk`,
/// `~/Library/Application Support/Brisk` or `$XDG_CONFIG_HOME/brisk` (`~/.config/brisk`).
pub fn config_dir() -> PathBuf {
    config_dir_from(portable(), &|k| std::env::var_os(k))
}

/// The settings folder of a portable install: `<exe dir>/data` when a file named `portable`
/// sits next to the executable.
pub fn portable() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    portable_in(exe.parent()?, |p| p.is_file())
}

/// The per-user roaming application data folder (`%APPDATA%`), where Windows tools such as
/// PyCharm and Poetry keep theirs. `None` on other systems.
pub fn roaming_app_data() -> Option<PathBuf> {
    sys::roaming_app_data(&|k| std::env::var_os(k))
}

fn portable_in(exe_dir: &Path, is_file: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    is_file(&exe_dir.join("portable")).then(|| exe_dir.join("data"))
}

type Env<'a> = &'a dyn Fn(&str) -> Option<OsString>;

fn config_dir_from(portable: Option<PathBuf>, env: Env) -> PathBuf {
    if let Some(p) = portable {
        return p;
    }
    if let Some(d) = sys::config_dir(env) {
        return d;
    }
    if let Some(x) = env("XDG_CONFIG_HOME") {
        return PathBuf::from(x).join("brisk");
    }
    if let Some(h) = env("HOME").or_else(|| env("USERPROFILE")) {
        return PathBuf::from(h).join(".config").join("brisk");
    }
    std::env::temp_dir().join("brisk")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| OsString::from(v))
    }

    #[test]
    fn portable_only_with_the_marker_file() {
        let exe_dir = Path::new("brisk-install");
        assert_eq!(portable_in(exe_dir, |p| p == exe_dir.join("portable")), Some(exe_dir.join("data")));
        assert_eq!(portable_in(exe_dir, |_| false), None);
    }

    #[test]
    fn portable_wins_over_the_user_folder() {
        let data = Path::new("brisk-install").join("data");
        let e = env(&[("APPDATA", "roaming"), ("XDG_CONFIG_HOME", "xdg"), ("HOME", "home")]);
        assert_eq!(config_dir_from(Some(data.clone()), &e), data);
    }

    #[cfg(windows)]
    #[test]
    fn installed_uses_appdata() {
        let e = env(&[("APPDATA", r"C:\Users\u\AppData\Roaming"), ("XDG_CONFIG_HOME", "xdg"), ("HOME", "home")]);
        assert_eq!(config_dir_from(None, &e).as_os_str(), r"C:\Users\u\AppData\Roaming\Brisk");
        assert_eq!(sys::roaming_app_data(&e), Some(PathBuf::from(r"C:\Users\u\AppData\Roaming")));
    }

    #[cfg(not(windows))]
    #[test]
    fn installed_uses_the_user_config_folder() {
        let home = env(&[("APPDATA", "roaming"), ("HOME", "/home/u")]);
        let expected = if cfg!(target_os = "macos") {
            Path::new("/home/u").join("Library").join("Application Support").join("Brisk")
        } else {
            Path::new("/home/u").join(".config").join("brisk")
        };
        assert_eq!(config_dir_from(None, &home), expected);
        assert_eq!(sys::roaming_app_data(&home), None);
    }

    #[test]
    fn fallbacks_without_the_system_folder() {
        assert_eq!(config_dir_from(None, &env(&[("XDG_CONFIG_HOME", "xdg")])), Path::new("xdg").join("brisk"));
        assert_eq!(config_dir_from(None, &env(&[])), std::env::temp_dir().join("brisk"));
    }
}
