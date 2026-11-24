//! Folders marked by the user (PyCharm's Mark Directory as): source roots and test source
//! roots go on the import path of runs and ty, excluded folders are left out of search,
//! the file index and ty. Stored in `.brisk/structure.json`; on first use the marks are
//! taken from PyCharm's `.idea/*.iml` when there is one.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Sources,
    Tests,
    Excluded,
}

/// Paths are relative to the project root with '/' separators.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Structure {
    #[serde(default, rename = "sourceRoots", skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    #[serde(default, rename = "testRoots", skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded: Vec<String>,
}

/// `path` relative to `root` with '/' separators, `""` for the root itself.
pub fn rel_path(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    Some(rel.to_string_lossy().replace('\\', "/").trim_matches('/').to_string())
}

fn under(rel: &str, dir: &str) -> bool {
    rel == dir || (rel.len() > dir.len() && rel.starts_with(dir) && rel.as_bytes()[dir.len()] == b'/')
}

impl Structure {
    pub fn file(root: &Path) -> PathBuf {
        root.join(".brisk").join("structure.json")
    }

    /// The saved marks, else PyCharm's, else none.
    pub fn load(root: &Path) -> Self {
        match std::fs::read_to_string(Self::file(root)) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Self::from_idea(root).unwrap_or_default(),
        }
    }

    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(root.join(".brisk"))?;
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(Self::file(root), text + "\n")
    }

    /// `sourceFolder` and `excludeFolder` entries of the project's `.idea/*.iml` files.
    pub fn from_idea(root: &Path) -> Option<Self> {
        let rd = std::fs::read_dir(root.join(".idea")).ok()?;
        let mut s = Structure::default();
        let mut found = false;
        for e in rd.flatten() {
            if e.path().extension().is_none_or(|x| x != "iml") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            found = true;
            for tag in text.split('<').skip(1) {
                let (name, rest) = tag.split_once(char::is_whitespace).unwrap_or((tag, ""));
                let Some(url) = attr(rest, "url") else { continue };
                let Some(rel) = url.strip_prefix("file://$MODULE_DIR$").map(|r| r.trim_matches('/').to_string()) else { continue };
                if rel.is_empty() || rel.contains("..") {
                    continue;
                }
                match name {
                    "sourceFolder" if attr(rest, "isTestSource").as_deref() == Some("true") => s.set(&rel, Some(Mark::Tests)),
                    "sourceFolder" => s.set(&rel, Some(Mark::Sources)),
                    "excludeFolder" => s.set(&rel, Some(Mark::Excluded)),
                    _ => {}
                }
            }
        }
        found.then_some(s)
    }

    /// The folder's own mark (not an inherited one).
    pub fn mark_of(&self, rel: &str) -> Option<Mark> {
        if self.excluded.iter().any(|d| d == rel) {
            Some(Mark::Excluded)
        } else if self.tests.iter().any(|d| d == rel) {
            Some(Mark::Tests)
        } else if self.sources.iter().any(|d| d == rel) {
            Some(Mark::Sources)
        } else {
            None
        }
    }

    /// Marks a folder (`None` unmarks it). One mark per folder.
    pub fn set(&mut self, rel: &str, mark: Option<Mark>) {
        for v in [&mut self.sources, &mut self.tests, &mut self.excluded] {
            v.retain(|d| d != rel);
        }
        let v = match mark {
            Some(Mark::Sources) => &mut self.sources,
            Some(Mark::Tests) => &mut self.tests,
            Some(Mark::Excluded) => &mut self.excluded,
            None => return,
        };
        v.push(rel.to_string());
        v.sort();
    }

    /// The path or one of its parents is marked excluded.
    pub fn is_excluded(&self, rel: &str) -> bool {
        self.excluded.iter().any(|d| under(rel, d))
    }

    /// Source and test roots that exist, for `PYTHONPATH` and ty's search paths.
    pub fn import_roots(&self, root: &Path) -> Vec<PathBuf> {
        self.sources.iter().chain(&self.tests).filter(|d| !d.is_empty()).map(|d| root.join(d)).filter(|p| p.is_dir()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty() && self.tests.is_empty() && self.excluded.is_empty()
    }
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let i = tag.find(&format!("{name}=\""))? + name.len() + 2;
    let end = tag[i..].find('"')?;
    Some(tag[i..i + end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_and_inheritance() {
        let mut s = Structure::default();
        s.set("src", Some(Mark::Sources));
        s.set("data", Some(Mark::Excluded));
        s.set("src", Some(Mark::Tests));
        assert_eq!(s.mark_of("src"), Some(Mark::Tests));
        assert!(s.sources.is_empty());
        assert!(s.is_excluded("data") && s.is_excluded("data/raw/x.csv"));
        assert!(!s.is_excluded("database") && !s.is_excluded("src"));
        s.set("data", None);
        assert!(!s.is_excluded("data"));
    }

    #[test]
    fn reads_pycharm_iml() {
        let d = std::env::temp_dir().join(format!("brisk-structure-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".idea")).unwrap();
        std::fs::write(
            d.join(".idea/proj.iml"),
            r#"<module type="PYTHON_MODULE" version="4">
  <component name="NewModuleRootManager">
    <content url="file://$MODULE_DIR$">
      <sourceFolder url="file://$MODULE_DIR$/backend/src" isTestSource="false" />
      <sourceFolder url="file://$MODULE_DIR$/backend/tests" isTestSource="true" />
      <excludeFolder url="file://$MODULE_DIR$/.venv" />
      <excludeFolder url="file://$MODULE_DIR$/media" />
    </content>
  </component>
</module>"#,
        )
        .unwrap();
        let s = Structure::load(&d);
        assert_eq!(s.sources, ["backend/src"]);
        assert_eq!(s.tests, ["backend/tests"]);
        assert_eq!(s.excluded, [".venv", "media"]);
        s.save(&d).unwrap();
        assert_eq!(Structure::load(&d), s);
        let _ = std::fs::remove_dir_all(&d);
    }
}
