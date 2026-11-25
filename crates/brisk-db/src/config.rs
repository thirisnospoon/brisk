//! Data source storage: `.brisk/dataSources.json` in the project (shareable, no passwords).
//! On first open, data sources from PyCharm's `.idea/dataSources.xml` are imported.

use crate::{DataSource, Driver};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const CONFIG_FILE: &str = ".brisk/dataSources.json";

#[derive(Serialize, Deserialize, Default)]
struct File {
    #[serde(rename = "dataSources", default)]
    data_sources: Vec<DataSource>,
}

/// Brisk's settings folder (kept here for the callers that reach it through `brisk_db`).
pub use brisk_platform::dirs::config_dir;

/// Load data sources. Returns `(sources, imported_from_pycharm)`.
pub fn load(root: &Path) -> (Vec<DataSource>, bool) {
    let path = root.join(CONFIG_FILE);
    if let Ok(s) = std::fs::read_to_string(&path) {
        match serde_json::from_str::<File>(&s) {
            Ok(f) => return (f.data_sources, false),
            // Broken by a hand edit: keep it before the next save replaces the data sources.
            Err(_) => {
                let _ = std::fs::copy(&path, path.with_extension("json.bak"));
            }
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
        let (inner, next) = if open.ends_with('/') {
            // `<data-source …/>`: nothing inside; the next element starts right after it (its
            // `</data-source>` is not this one's).
            ("", gt + 1)
        } else {
            const CLOSE: &str = "</data-source>";
            let end = after.find(CLOSE).unwrap_or(after.len());
            (&after[gt + 1..end.max(gt + 1)], (end + CLOSE.len()).min(after.len()))
        };
        out.push((attr(open, "uuid").unwrap_or_default(), attr(open, "name").unwrap_or_default(), inner.to_string()));
        rest = &after[next..];
    }
    out
}

#[cfg(test)]
#[test]
fn self_closing_data_source_does_not_hide_the_next() {
    let xml = r#"<component><data-source source="LOCAL" name="empty" uuid="1"/>
<data-source source="LOCAL" name="pg" uuid="2"><jdbc-url>jdbc:postgresql://h/db</jdbc-url></data-source>
<data-source name="third" uuid="3"><jdbc-url>x</jdbc-url></data-source></component>"#;
    let v = data_sources(xml);
    assert_eq!(v.iter().map(|(id, n, _)| (id.as_str(), n.as_str())).collect::<Vec<_>>(), [("1", "empty"), ("2", "pg"), ("3", "third")]);
    assert_eq!(v[0].2, "");
    assert_eq!(tag(&v[1].2, "jdbc-url").as_deref(), Some("jdbc:postgresql://h/db"));
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
