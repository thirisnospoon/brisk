//! Position units. Brisk counts columns in characters (UTF-32); a server that only speaks
//! UTF-16 (TypeScript) gets its positions converted on the way out and back, for the whole
//! message at once: every `{line, character}` object is a position, in the file named by the
//! nearest `uri` / `targetUri` / `textDocument.uri` above it (or a `changes` map key).

use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf32,
    Utf16,
}

/// The texts of the documents open in the server (what positions in them refer to); other
/// files are read from disk when a reply points into them.
pub type Docs = Arc<Mutex<HashMap<PathBuf, String>>>;

/// Line texts per file for one message.
struct Lines<'a> {
    docs: &'a HashMap<PathBuf, String>,
    cache: HashMap<String, Option<Vec<String>>>,
}

impl Lines<'_> {
    fn line(&mut self, uri: &str, line: usize) -> Option<&str> {
        if !self.cache.contains_key(uri) {
            let text = super::uri_to_path(uri).and_then(|p| self.docs.get(&p).cloned().or_else(|| std::fs::read_to_string(&p).ok()));
            self.cache.insert(uri.to_string(), text.map(|t| t.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect()));
        }
        self.cache.get(uri)?.as_ref()?.get(line).map(String::as_str)
    }
}

/// UTF-16 units → characters in `line`.
fn chars_of(line: &str, units: usize) -> usize {
    let mut u = 0;
    for (i, c) in line.chars().enumerate() {
        if u >= units {
            return i;
        }
        u += c.len_utf16();
    }
    line.chars().count() + units.saturating_sub(u)
}

/// Characters → UTF-16 units in `line`.
fn units_of(line: &str, chars: usize) -> usize {
    let n = line.chars().count();
    line.chars().take(chars).map(char::len_utf16).sum::<usize>() + chars.saturating_sub(n)
}

fn walk(v: &mut Value, uri: &str, to_wire: bool, lines: &mut Lines) {
    match v {
        Value::Array(a) => a.iter_mut().for_each(|x| walk(x, uri, to_wire, lines)),
        Value::Object(m) => {
            if m.len() == 2 {
                if let (Some(l), Some(c)) = (m.get("line").and_then(Value::as_u64), m.get("character").and_then(Value::as_u64)) {
                    if let Some(text) = lines.line(uri, l as usize).filter(|t| !t.is_ascii()) {
                        let c = if to_wire { units_of(text, c as usize) } else { chars_of(text, c as usize) };
                        m.insert("character".into(), c.into());
                    }
                    return;
                }
            }
            let own = ["uri", "targetUri"]
                .iter()
                .find_map(|k| m.get(*k).and_then(Value::as_str))
                .or_else(|| m.get("textDocument").and_then(|t| t.get("uri")).and_then(Value::as_str))
                .map(str::to_string);
            let here = own.as_deref().unwrap_or(uri).to_string();
            for (k, x) in m.iter_mut() {
                match (k.as_str(), x) {
                    ("changes", Value::Object(files)) => {
                        for (file, edits) in files.iter_mut() {
                            walk(edits, file, to_wire, lines);
                        }
                    }
                    (_, x) => walk(x, &here, to_wire, lines),
                }
            }
        }
        _ => {}
    }
}

/// Converts the positions of a message in place: `to_wire` from characters to UTF-16, else
/// back. `uri` is the document of positions with no file of their own.
pub fn convert(v: &mut Value, uri: &str, to_wire: bool, docs: &Docs) {
    let docs = docs.lock().unwrap_or_else(|e| e.into_inner());
    let mut lines = Lines { docs: &docs, cache: HashMap::new() };
    walk(v, uri, to_wire, &mut lines);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn utf16_positions_round_trip() {
        let path = std::env::temp_dir().join("brisk-enc-test.ts");
        let uri = super::super::path_to_uri(&path);
        let docs: Docs = Arc::default();
        // "😀" is one character, two UTF-16 units.
        docs.lock().unwrap().insert(path.clone(), "ok\nconst s = '😀'; x\n".into());
        let mut out = json!({"textDocument": {"uri": uri}, "position": {"line": 1, "character": 15}});
        convert(&mut out, "", true, &docs);
        assert_eq!(out["position"]["character"], 16);
        let mut back = json!([{"uri": uri, "range": {"start": {"line": 1, "character": 16}, "end": {"line": 0, "character": 1}}}]);
        convert(&mut back, "", false, &docs);
        assert_eq!(back[0]["range"]["start"]["character"], 15);
        assert_eq!(back[0]["range"]["end"]["character"], 1);
        let mut edit =
            json!({"changes": {uri.clone(): [{"range": {"start": {"line": 1, "character": 18}, "end": {"line": 1, "character": 19}}, "newText": "y"}]}});
        convert(&mut edit, "", false, &docs);
        assert_eq!(edit["changes"][&uri][0]["range"]["start"]["character"], 17);
    }
}
