//! Scratch files, as in PyCharm: File › New › Scratch File (Ctrl+Alt+Shift+Insert) asks for a
//! language and creates `scratch.py`, `scratch_1.py`, … in `<config>/scratches` (outside the
//! project), filled with the editor's selection if there is one. The folder is the
//! "Scratches and Consoles" node under the project in the Project view.
//!
//! Also New › Python File's template: `<config>/fileTemplates/Python Script.py`, if present, with
//! `${NAME}`, `${DATE}`, `${TIME}`, `${USER}`, `${PROJECT_NAME}` filled in (no template engine).

use super::{PopupItem, PopupKind, Target, Workspace};
use gpui::{Context, Window};
use std::path::{Path, PathBuf};

/// (title, extension) in the order of PyCharm's "New Scratch File" list (the current file's
/// language goes first).
const LANGS: &[(&str, &str)] = &[
    ("Python", "py"),
    ("Plain text", "txt"),
    ("JSON", "json"),
    ("Markdown", "md"),
    ("SQL", "sql"),
    ("YAML", "yaml"),
    ("TOML", "toml"),
    ("HTML", "html"),
    ("CSS", "css"),
    ("JavaScript", "js"),
    ("TypeScript", "ts"),
    ("XML", "xml"),
    ("Shell Script", "sh"),
    ("PowerShell", "ps1"),
    ("Batch", "bat"),
    ("Dockerfile", "dockerfile"),
    ("INI", "ini"),
];

pub(super) const ROOT_NAME: &str = "Scratches and Consoles";

pub(super) fn scratch_dir() -> PathBuf {
    brisk_db::config_dir().join("scratches")
}

/// `scratch.<ext>`, else `scratch_1.<ext>`, … : the first free name.
fn free_name(dir: &Path, ext: &str) -> PathBuf {
    let ext = ext.trim();
    let name = |i: usize| {
        let stem = if i == 0 { "scratch".to_string() } else { format!("scratch_{i}") };
        if ext == "dockerfile" {
            dir.join(format!("{stem}.Dockerfile"))
        } else {
            dir.join(format!("{stem}.{ext}"))
        }
    };
    (0..).map(name).find(|p| !p.exists()).unwrap()
}

/// Fill `${NAME}`-style variables, PyCharm's names. Unknown ones stay as typed.
pub(super) fn fill_template(text: &str, name: &str, project: &str) -> String {
    let now = chrono_like_now();
    let user = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default();
    text.replace("${NAME}", name).replace("${DATE}", &now.0).replace("${TIME}", &now.1).replace("${USER}", &user).replace("${PROJECT_NAME}", project)
}

/// Local date `dd.mm.yyyy` and time `HH:MM` (PyCharm's defaults for ${DATE}/${TIME}), from the
/// system clock and the local UTC offset, without a date crate.
fn chrono_like_now() -> (String, String) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) + local_offset_secs();
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (format!("{d:02}.{m:02}.{y}"), format!("{:02}:{:02}", rem / 3600, rem % 3600 / 60))
}

#[cfg(windows)]
fn local_offset_secs() -> i64 {
    #[repr(C)]
    struct Tzi {
        bias: i32,
        _rest: [u8; 168],
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetTimeZoneInformation(tz: *mut Tzi) -> u32;
    }
    let mut tz = Tzi { bias: 0, _rest: [0; 168] };
    // 2 = daylight time in effect: the daylight bias (at offset 168) adds to the bias.
    let r = unsafe { GetTimeZoneInformation(&mut tz) };
    let daylight = if r == 2 { i32::from_le_bytes(tz._rest[164..168].try_into().unwrap()) } else { 0 };
    -i64::from(tz.bias + daylight) * 60
}

#[cfg(not(windows))]
fn local_offset_secs() -> i64 {
    0
}

impl Workspace {
    pub(super) fn start_new_scratch(&mut self, cx: &mut Context<Self>) {
        // Filtered by `structure_items` as the query changes.
        self.structure = self.new_scratch_items(cx);
        self.open_popup(PopupKind::NewScratch);
        cx.notify();
    }

    fn new_scratch_items(&self, cx: &gpui::App) -> Vec<PopupItem> {
        // The current file's language first, as PyCharm does.
        let cur = self.active_editor().and_then(|e| e.read(cx).path.as_ref().and_then(|p| p.extension()).map(|x| x.to_string_lossy().to_lowercase()));
        let mut langs: Vec<&(&str, &str)> = LANGS.iter().collect();
        if let Some(c) = cur {
            if let Some(i) = langs.iter().position(|(_, x)| x.trim() == c) {
                let l = langs.remove(i);
                langs.insert(0, l);
            }
        }
        langs
            .into_iter()
            .map(|&(t, x)| PopupItem { label: t.to_string(), detail: format!(".{}", x.trim()), hint: String::new(), target: Target::NewScratch(x) })
            .collect()
    }

    pub(super) fn create_scratch(&mut self, ext: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let dir = scratch_dir();
        let path = free_name(&dir, ext);
        // The editor's selection becomes the scratch's text.
        let text = self.active_editor().map(|e| e.read(cx).buffer.selected_text()).unwrap_or_default();
        let r = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, text));
        if let Err(e) = r {
            self.status = format!("Cannot create a scratch file: {e}");
            return cx.notify();
        }
        let root = self.tree.add_root(ROOT_NAME, &dir);
        self.tree.reload(root);
        if let Some(id) = self.tree.reveal(&path) {
            self.tree_selected = Some(id);
        }
        self.open_file(&path, window, cx);
        self.status = format!("Created scratch file {}", path.file_name().unwrap_or_default().to_string_lossy());
        cx.notify();
    }

    /// The text of a new Python file: the user's "Python Script" template, if any.
    pub(super) fn python_template(&self, file: &Path) -> String {
        let t = brisk_db::config_dir().join("fileTemplates").join("Python Script.py");
        let Ok(text) = std::fs::read_to_string(t) else { return String::new() };
        let name = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let project = self.root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        fill_template(&text, &name, &project)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_names_count_up() {
        let d = std::env::temp_dir().join(format!("brisk-scratch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(free_name(&d, "py"), d.join("scratch.py"));
        std::fs::write(d.join("scratch.py"), "").unwrap();
        assert_eq!(free_name(&d, "py"), d.join("scratch_1.py"));
        assert_eq!(free_name(&d, "dockerfile"), d.join("scratch.Dockerfile"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn template_fills_names() {
        let t = fill_template("# ${NAME} in ${PROJECT_NAME} ${UNKNOWN}", "util", "demo");
        assert_eq!(t, "# util in demo ${UNKNOWN}");
        let (date, time) = chrono_like_now();
        assert_eq!(date.len(), 10);
        assert_eq!(time.len(), 5);
    }
}
