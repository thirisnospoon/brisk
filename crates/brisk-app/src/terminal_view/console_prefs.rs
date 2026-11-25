//! Soft-Wrap and Scroll to End of the Run console, remembered per run configuration (by name)
//! in `.brisk/run-console.json`: a configuration opens with the toggles it was left with.

use super::TerminalPanel;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct ConsolePrefs {
    /// Configuration name → (soft wrap, scroll to end); absent means both on.
    map: BTreeMap<String, (bool, bool)>,
    loaded: bool,
    /// The tab whose toggles the panel currently shows.
    synced: Option<u64>,
}

fn file(root: &Path) -> PathBuf {
    root.join(".brisk").join("run-console.json")
}

impl ConsolePrefs {
    fn load(&mut self, root: &Path) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let Ok(text) = std::fs::read_to_string(file(root)) else { return };
        let Ok(serde_json::Value::Object(o)) = serde_json::from_str::<serde_json::Value>(&text) else { return };
        for (name, v) in o {
            let flag = |k: &str| v.get(k).and_then(|b| b.as_bool()).unwrap_or(true);
            self.map.insert(name, (flag("softWrap"), flag("scrollToEnd")));
        }
    }

    fn save(&self, root: &Path) {
        let o: serde_json::Map<String, serde_json::Value> =
            self.map.iter().map(|(k, &(w, f))| (k.clone(), serde_json::json!({ "softWrap": w, "scrollToEnd": f }))).collect();
        let _ = std::fs::create_dir_all(root.join(".brisk"));
        let _ = crate::editor::write_atomic(&file(root), serde_json::to_string_pretty(&o).unwrap_or_default().as_bytes());
    }
}

impl TerminalPanel {
    fn active_run_name(&self) -> Option<(u64, String)> {
        self.tabs.get(self.active).and_then(|t| t.run.as_ref().map(|r| (t.id, r.name.clone())))
    }

    /// Show the toggles of the active tab's configuration (called on every render; cheap
    /// unless the active tab changed).
    pub(super) fn sync_console_prefs(&mut self) {
        let Some((id, name)) = self.active_run_name() else { return };
        if self.prefs.synced == Some(id) {
            return;
        }
        self.prefs.synced = Some(id);
        self.prefs.load(&self.root);
        let (wrap, follow) = self.prefs.map.get(&name).copied().unwrap_or((true, true));
        if wrap != self.soft_wrap {
            self.h_scroll = 0;
        }
        self.soft_wrap = wrap;
        self.scroll_to_end = follow;
    }

    /// A toggle was clicked (`wrap`: Soft-Wrap, else Scroll to End): remember that one for the
    /// active configuration. Scrolling up stops following without changing the setting.
    pub(super) fn store_console_prefs(&mut self, wrap: bool) {
        let Some((id, name)) = self.active_run_name() else { return };
        self.prefs.load(&self.root);
        self.prefs.synced = Some(id);
        let mut v = self.prefs.map.get(&name).copied().unwrap_or((true, true));
        if wrap {
            v.0 = self.soft_wrap;
        } else {
            v.1 = self.scroll_to_end;
        }
        if v == (true, true) {
            self.prefs.map.remove(&name);
        } else {
            self.prefs.map.insert(name, v);
        }
        self.prefs.save(&self.root);
    }
}
