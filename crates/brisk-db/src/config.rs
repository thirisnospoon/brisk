//! Data source storage: `.brisk/dataSources.json` in the project (shareable, no passwords).
//! On first open, data sources from PyCharm's `.idea/dataSources.xml` are imported.

use crate::{DataSource, Driver};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = ".brisk/dataSources.json";

#[derive(Serialize, Deserialize, Default)]
struct File {
    #[serde(rename = "dataSources", default)]
    data_sources: Vec<DataSource>,
}

/// Per-user config dir: `%APPDATA%\Brisk`, `~/.config/brisk`, `~/Library/Application Support/Brisk`.
pub fn config_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(a) = std::env::var_os("APPDATA") {
            return PathBuf::from(a).join("Brisk");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(h) = std::env::var_os("HOME") {
            return PathBuf::from(h).join("Library/Application Support/Brisk");
        }
    }
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(x).join("brisk");
    }
    if let Some(h) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        return PathBuf::from(h).join(".config/brisk");
    }
    std::env::temp_dir().join("brisk")
}

/// Load data sources. Returns `(sources, imported_from_pycharm)`.
pub fn load(root: &Path) -> (Vec<DataSource>, bool) {
    if let Ok(s) = std::fs::read_to_string(root.join(CONFIG_FILE)) {
        if let Ok(f) = serde_json::from_str::<File>(&s) {
            return (f.data_sources, false);
        }
    }
    let imported = import_idea(root);
    let any = !imported.is_empty();
    (imported, any)
}

pub fn save(root: &Path, sources: &[DataSource]) -> std::io::Result<()> {
    let path = root.join(CONFIG_FILE);
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let json = serde_json::to_string_pretty(&File { data_sources: sources.to_vec() }).map_err(std::io::Error::other)?;
    std::fs::write(path, json + "\n")
}

/// Values of `<tag>…</tag>` inside an XML fragment (no XML dependency for two small files).
fn tag(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let s = xml.find(&open)? + open.len();
    let e = xml[s..].find(&format!("</{name}>"))? + s;
    Some(unescape(&xml[s..e]))
}

fn attr(open_tag: &str, name: &str) -> Option<String> {
    let key = format!(" {name}=\"");
    let s = open_tag.find(&key)? + key.len();
    let e = open_tag[s..].find('"')? + s;
    Some(unescape(&open_tag[s..e]))
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// `(uuid, name, inner xml)` for each `<data-source>` element.
fn data_sources(xml: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<data-source ") {
        let after = &rest[i..];
        let Some(gt) = after.find('>') else { break };
        let open = &after[..gt];
        let end = after.find("</data-source>").unwrap_or(after.len());
        let inner = if open.ends_with('/') { "" } else { &after[gt + 1..end] };
        out.push((attr(open, "uuid").unwrap_or_default(), attr(open, "name").unwrap_or_default(), inner.to_string()));
        rest = &after[end.min(after.len())..];
        if rest.len() == after.len() {
            rest = &after[gt + 1..];
        }
    }
    out
}

/// Import PyCharm/DataGrip data sources: `.idea/dataSources.xml` (URL, driver) and
/// `.idea/dataSources.local.xml` (user name). Passwords live in the JetBrains keychain and
/// are asked for on first connect.
pub fn import_idea(root: &Path) -> Vec<DataSource> {
    let Ok(xml) = std::fs::read_to_string(root.join(".idea/dataSources.xml")) else { return Vec::new() };
    let local = std::fs::read_to_string(root.join(".idea/dataSources.local.xml")).unwrap_or_default();
    let users: Vec<(String, String)> = data_sources(&local).into_iter().filter_map(|(id, _, inner)| tag(&inner, "user-name").map(|u| (id, u))).collect();
    let mut out = Vec::new();
    for (uuid, name, inner) in data_sources(&xml) {
        let Some(url) = tag(&inner, "jdbc-url") else { continue };
        let driver_ref = tag(&inner, "driver-ref").unwrap_or_default().to_lowercase();
        let mut ds = DataSource::new(if driver_ref.contains("sqlite") {
            Driver::Sqlite
        } else if driver_ref.contains("mysql") || driver_ref.contains("maria") {
            Driver::Mysql
        } else {
            Driver::Postgres
        });
        if ds.apply_url(&url).is_none() {
            continue;
        }
        if !uuid.is_empty() {
            ds.id = uuid.clone();
        }
        ds.name = name;
        if let Some((_, u)) = users.iter().find(|(id, _)| *id == uuid) {
            ds.user = u.clone();
        }
        if ds.driver == Driver::Sqlite {
            ds.file = ds.file.replace("$PROJECT_DIR$", "").trim_start_matches(['/', '\\']).to_string();
        }
        out.push(ds);
    }
    out
}
