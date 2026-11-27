//! Where a JS / TS / HTML / CSS reference points, without a TypeScript server: the string
//! under the caret in an `import` / `require` / `export … from`, an HTML `src` / `href`, a CSS
//! `@import` / `url()`, resolved the way Node and bundlers do (relative paths with the usual
//! extensions and `index` files, `compilerOptions.paths` aliases of `tsconfig.json` /
//! `jsconfig.json`, packages in `node_modules` by their `package.json`). Also which module an
//! imported name comes from, for Ctrl+B on the name.

use std::path::{Path, PathBuf};

/// Extensions tried after a path without one, in order.
const EXTS: &[&str] = &["ts", "tsx", "d.ts", "js", "jsx", "mjs", "cjs", "mts", "cts", "vue", "svelte", "json", "css", "scss"];

/// The quoted string around `byte` on its line: its text and byte range (quotes excluded).
pub fn string_at(text: &str, byte: usize) -> Option<(String, std::ops::Range<usize>)> {
    let byte = byte.min(text.len());
    let start = text[..byte].rfind('\n').map_or(0, |i| i + 1);
    let end = text[byte..].find('\n').map_or(text.len(), |i| byte + i);
    let line = &text[start..end];
    let mut open: Option<(usize, char)> = None;
    for (i, c) in line.char_indices() {
        match open {
            None if matches!(c, '"' | '\'' | '`') => open = Some((i, c)),
            Some((o, q)) if c == q && !line[..i].ends_with('\\') => {
                let (a, b) = (start + o + 1, start + i);
                if a <= byte && byte <= b {
                    return Some((text[a..b].to_string(), a..b));
                }
                open = None;
            }
            _ => {}
        }
    }
    // `url(img/x.png)` without quotes.
    let before = &line[..byte - start];
    let u = before.rfind("url(")?;
    let a = start + u + 4;
    let b = start + line[u + 4..].find(')').map(|i| u + 4 + i)?;
    (a <= byte && byte <= b).then(|| (text[a..b].trim().to_string(), a..b))
}

/// The string at `byte` is a path the file refers to: an import / require / dynamic import /
/// re-export, an HTML `src` / `href` / `action`, a CSS `@import` / `url()`.
pub fn reference_at(text: &str, byte: usize) -> Option<String> {
    reference_range_at(text, byte).map(|r| r.0)
}

/// [`reference_at`] with the string's byte range (quotes excluded).
pub fn reference_range_at(text: &str, byte: usize) -> Option<(String, std::ops::Range<usize>)> {
    let (s, r) = string_at(text, byte)?;
    if s.contains("://") || s.starts_with('#') || s.starts_with("data:") || s.starts_with("mailto:") {
        return None;
    }
    let line_start = text[..r.start].rfind('\n').map_or(0, |i| i + 1);
    let before = text[line_start..r.start].trim_end_matches(['"', '\'', '`']).trim_end();
    let ok = before.ends_with("from")
        || before.ends_with("import")
        || before.ends_with("require(")
        || before.ends_with("import(")
        || before.ends_with("url(")
        || before.ends_with("@import")
        || ["src=", "href=", "action=", "src =", "href ="].iter().any(|k| before.ends_with(k))
        // A multi-line import: `} from "x"` after the line's start.
        || before.starts_with('}') && before.ends_with("from");
    ok.then_some((s, r))
}

/// The folder whose entries complete the path typed so far (`typed` ends where the caret
/// is): its part up to the last `/` resolved like [`resolve`] resolves a folder. `None` for a
/// bare package name being typed (the caller lists `node_modules`).
pub fn completion_dir(typed: &str, from: &Path, root: &Path) -> Option<PathBuf> {
    let dir = from.parent().unwrap_or(root);
    let Some(slash) = typed.rfind('/') else {
        let plain = from.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e, "css" | "scss" | "less" | "html" | "htm"));
        return plain.then(|| dir.to_path_buf());
    };
    let head = &typed[..=slash];
    let p = if head.starts_with("./") || head.starts_with("../") {
        dir.join(head)
    } else if let Some(rest) = head.strip_prefix('/') {
        return ["", "public", "static"].iter().map(|d| root.join(d).join(rest)).find(|p| p.is_dir());
    } else if let Some(rest) = head.strip_prefix("@/").or_else(|| head.strip_prefix("~/")) {
        root.join("src").join(rest)
    } else {
        let plain = from.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e, "css" | "scss" | "less" | "html" | "htm"));
        if plain {
            dir.join(head)
        } else {
            // `pkg/sub/`: inside the package.
            let in_pkg = dir.ancestors().map(|d| d.join("node_modules").join(head)).find(|p| p.is_dir());
            return in_pkg;
        }
    };
    p.is_dir().then_some(p)
}

/// The packages in the nearest `node_modules` folders (`@scope/name` for scoped ones).
pub fn packages(from: &Path, root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for d in from.parent().unwrap_or(root).ancestors() {
        let nm = d.join("node_modules");
        let Ok(rd) = std::fs::read_dir(&nm) else {
            if d == root {
                break;
            }
            continue;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            if name.starts_with('@') {
                for s in std::fs::read_dir(e.path()).into_iter().flatten().flatten() {
                    out.push(format!("{name}/{}", s.file_name().to_string_lossy()));
                }
            } else {
                out.push(name);
            }
        }
        if d == root {
            break;
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The file `spec` names from the file `from` in the project `root`.
pub fn resolve(spec: &str, from: &Path, root: &Path) -> Option<PathBuf> {
    let spec = spec.split(['?', '#']).next().unwrap_or(spec).trim();
    if spec.is_empty() {
        return None;
    }
    let dir = from.parent().unwrap_or(root);
    if spec.starts_with("./") || spec.starts_with("../") || spec == "." || spec == ".." {
        return file_or_dir(&dir.join(spec));
    }
    if let Some(rest) = spec.strip_prefix('/') {
        // A site-absolute path: the project root, or a public / static folder.
        return ["", "public", "static", "src"].iter().find_map(|d| file_or_dir(&root.join(d).join(rest)));
    }
    // A CSS / HTML path without `./` is relative too.
    let plain = from.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e, "css" | "scss" | "less" | "html" | "htm"));
    if plain {
        if let Some(p) = file_or_dir(&dir.join(spec)) {
            return Some(p);
        }
    }
    if let Some(p) = alias(spec, dir, root) {
        return Some(p);
    }
    package(spec, dir, root)
}

fn file_or_dir(p: &Path) -> Option<PathBuf> {
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    let name = p.file_name()?.to_string_lossy().into_owned();
    // `./util.js` written for TypeScript sources that are `util.ts`.
    for (js, ts) in [(".js", ".ts"), (".js", ".tsx"), (".mjs", ".mts"), (".cjs", ".cts"), (".jsx", ".tsx")] {
        if let Some(stem) = name.strip_suffix(js) {
            let q = p.with_file_name(format!("{stem}{ts}"));
            if q.is_file() {
                return Some(q);
            }
        }
    }
    for e in EXTS {
        let q = p.with_file_name(format!("{name}.{e}"));
        if q.is_file() {
            return Some(q);
        }
    }
    if p.is_dir() {
        if let Some(entry) = package_entry(p) {
            return Some(entry);
        }
        for e in EXTS {
            let q = p.join(format!("index.{e}"));
            if q.is_file() {
                return Some(q);
            }
        }
    }
    None
}

/// A package folder's entry file from its `package.json`: types, module, main, or the `.`
/// export.
fn package_entry(dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let mut candidates: Vec<String> = Vec::new();
    for k in ["types", "typings", "module", "main"] {
        if let Some(s) = v.get(k).and_then(|x| x.as_str()) {
            candidates.push(s.to_string());
        }
    }
    if let Some(e) = v.get("exports") {
        let dot = e.get(".").unwrap_or(e);
        collect_exports(dot, &mut candidates);
    }
    candidates.iter().find_map(|c| {
        let p = dir.join(c);
        if p.is_file() {
            Some(p)
        } else {
            file_or_dir_no_pkg(&p)
        }
    })
}

/// [`file_or_dir`] without reading `package.json` again (a `main` pointing at a folder).
fn file_or_dir_no_pkg(p: &Path) -> Option<PathBuf> {
    let name = p.file_name()?.to_string_lossy().into_owned();
    EXTS.iter().map(|e| p.with_file_name(format!("{name}.{e}"))).chain(EXTS.iter().map(|e| p.join(format!("index.{e}")))).find(|q| q.is_file())
}

fn collect_exports(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Object(m) => {
            for k in ["types", "import", "module", "default", "require", "node"] {
                if let Some(x) = m.get(k) {
                    collect_exports(x, out);
                }
            }
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect_exports(x, out)),
        _ => {}
    }
}

/// `pkg`, `pkg/sub`, `@scope/pkg/sub` from the nearest `node_modules` up to the root.
fn package(spec: &str, dir: &Path, root: &Path) -> Option<PathBuf> {
    let parts: Vec<&str> = spec.split('/').collect();
    let n = if spec.starts_with('@') { 2 } else { 1 };
    if parts.len() < n {
        return None;
    }
    let (name, sub) = (parts[..n].join("/"), parts[n..].join("/"));
    for d in dir.ancestors() {
        let pkg = d.join("node_modules").join(&name);
        if pkg.is_dir() {
            return if sub.is_empty() { file_or_dir(&pkg) } else { file_or_dir(&pkg.join(&sub)) };
        }
        if d == root || !d.starts_with(root) && d.parent().is_none() {
            break;
        }
    }
    // A workspace package: `packages/<name>`.
    None
}

/// `compilerOptions.paths` of the nearest `tsconfig.json` / `jsconfig.json`: `@/*` → `src/*`;
/// without paths, `baseUrl` makes `components/x` mean `<baseUrl>/components/x`.
fn alias(spec: &str, dir: &Path, root: &Path) -> Option<PathBuf> {
    for d in dir.ancestors() {
        for name in ["tsconfig.json", "jsconfig.json"] {
            let Some(cfg) = std::fs::read_to_string(d.join(name)).ok().and_then(|t| parse_jsonc(&t)) else { continue };
            let opts = cfg.get("compilerOptions");
            let base = d.join(opts.and_then(|o| o.get("baseUrl")).and_then(|b| b.as_str()).unwrap_or("."));
            if let Some(paths) = opts.and_then(|o| o.get("paths")).and_then(|p| p.as_object()) {
                for (pat, targets) in paths {
                    let Some(rest) = match_pattern(pat, spec) else { continue };
                    for t in targets.as_array().into_iter().flatten().filter_map(|t| t.as_str()) {
                        if let Some(p) = file_or_dir(&base.join(t.replace('*', &rest))) {
                            return Some(p);
                        }
                    }
                }
            }
            if opts.and_then(|o| o.get("baseUrl")).is_some() {
                if let Some(p) = file_or_dir(&base.join(spec)) {
                    return Some(p);
                }
            }
            // The nearest config decides.
            return None;
        }
        if d == root {
            break;
        }
    }
    // Vite and Nuxt projects use `@/` for `src/` even without a tsconfig path.
    spec.strip_prefix("@/").or_else(|| spec.strip_prefix("~/")).and_then(|r| file_or_dir(&root.join("src").join(r)))
}

/// `@/*` against `@/components/x`: `components/x`; an exact pattern gives "".
fn match_pattern(pat: &str, spec: &str) -> Option<String> {
    match pat.split_once('*') {
        Some((pre, post)) => spec.strip_prefix(pre).and_then(|r| r.strip_suffix(post)).map(str::to_string),
        None => (pat == spec).then(String::new),
    }
}

/// JSON with comments and trailing commas, as tsconfig allows.
fn parse_jsonc(text: &str) -> Option<serde_json::Value> {
    let mut out = String::with_capacity(text.len());
    let mut it = text.chars().peekable();
    let mut in_str = false;
    while let Some(c) = it.next() {
        if in_str {
            out.push(c);
            if c == '\\' {
                if let Some(n) = it.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match (c, it.peek()) {
            ('"', _) => {
                in_str = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for n in it.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                it.next();
                let mut prev = ' ';
                for n in it.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    // Trailing commas before `}` / `]`.
    let mut clean = String::with_capacity(out.len());
    let chars: Vec<char> = out.chars().collect();
    let mut in_str = false;
    for (i, &c) in chars.iter().enumerate() {
        if c == '"' && (i == 0 || chars[i - 1] != '\\') {
            in_str = !in_str;
        }
        if !in_str && c == ',' && chars[i + 1..].iter().find(|c| !c.is_whitespace()).is_some_and(|n| matches!(n, '}' | ']')) {
            continue;
        }
        clean.push(c);
    }
    serde_json::from_str(&clean).ok()
}

/// Where the name `name` used in `text` is imported from: the module and the name it has
/// there (`default` for a default import, `*` for a namespace). Handles `import a from`,
/// `import { b as c }`, `import * as ns`, `const { d } = require(…)` and `export { e } from`.
pub fn import_of(text: &str, name: &str) -> Option<(String, String)> {
    for stmt in statements(text) {
        let s = stmt.trim();
        let (clause, module) = if let Some(rest) = s.strip_prefix("import ").or_else(|| s.strip_prefix("export ")) {
            let (c, m) = rest.rsplit_once(" from ")?;
            (c.trim().trim_start_matches("type ").to_string(), unquote(m))
        } else if let Some(i) = s.find("require(") {
            let decl = s[..i].trim_end().trim_end_matches('=').trim();
            let decl = decl.trim_start_matches("const ").trim_start_matches("let ").trim_start_matches("var ");
            (decl.to_string(), unquote(&s[i + 8..s[i..].find(')').map_or(s.len(), |j| i + j)]))
        } else {
            continue;
        };
        let Some(module) = module else { continue };
        if let Some(found) = binding(&clause, name) {
            return Some((module, found));
        }
    }
    None
}

/// Statements that can be imports, joined across lines up to the module string.
fn statements(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in text.lines() {
        let t = line.trim();
        if cur.is_empty() && !(t.starts_with("import ") || t.starts_with("export ") || t.contains("require(")) {
            continue;
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(t);
        let done = cur.contains(" from ") && (cur.ends_with(';') || cur.ends_with('\'') || cur.ends_with('"'))
            || cur.contains("require(") && cur.contains(')')
            || !cur.contains('{') && !cur.starts_with("import ") && !cur.starts_with("export ");
        if done || cur.len() > 2000 {
            out.push(std::mem::take(&mut cur));
        }
    }
    out
}

fn unquote(s: &str) -> Option<String> {
    let s = s.trim().trim_end_matches(';').trim();
    let q = s.chars().next().filter(|c| matches!(c, '"' | '\'' | '`'))?;
    s[1..].find(q).map(|e| s[1..1 + e].to_string())
}

/// The name `name` takes in an import clause, as the exported name.
fn binding(clause: &str, name: &str) -> Option<String> {
    let clause = clause.trim();
    let (head, braces) = match (clause.find('{'), clause.rfind('}')) {
        (Some(a), Some(b)) if b > a => (format!("{}{}", &clause[..a], &clause[b + 1..]), Some(&clause[a + 1..b])),
        _ => (clause.to_string(), None),
    };
    if let Some(inner) = braces {
        for part in inner.split(',') {
            let part = part.trim().trim_start_matches("type ");
            let (orig, local) = match part.split_once(" as ").or_else(|| part.split_once(':')) {
                Some((o, l)) => (o.trim(), l.trim()),
                None => (part, part),
            };
            if local == name {
                return Some(orig.to_string());
            }
        }
    }
    for part in head.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        if let Some(ns) = part.strip_prefix("* as ") {
            if ns.trim() == name {
                return Some("*".into());
            }
        } else if part == name {
            return Some("default".into());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("brisk-jsmod-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn references_under_caret() {
        let t = "import { a } from './util';\nconst x = require(\"../lib/x\");\n<img src=\"/logo.png\">\nbody { background: url(img/bg.png) }\nconst s = 'just text';\n";
        let at = |needle: &str| t.find(needle).unwrap() + 1;
        assert_eq!(reference_at(t, at("./util")).as_deref(), Some("./util"));
        assert_eq!(reference_at(t, at("../lib")).as_deref(), Some("../lib/x"));
        assert_eq!(reference_at(t, at("/logo")).as_deref(), Some("/logo.png"));
        assert_eq!(reference_at(t, at("img/bg")).as_deref(), Some("img/bg.png"));
        assert_eq!(reference_at(t, at("just")), None);
    }

    #[test]
    fn resolves_relative_alias_and_package() {
        let d = tmp("resolve");
        touch(&d.join("src/util.ts"), "");
        touch(&d.join("src/comp/index.tsx"), "");
        touch(&d.join("src/app.ts"), "");
        touch(&d.join("tsconfig.json"), "{\n // comment\n \"compilerOptions\": { \"baseUrl\": \".\", \"paths\": { \"@/*\": [\"src/*\"], }, },\n}");
        touch(&d.join("node_modules/lodash/package.json"), r#"{"main": "lodash.js"}"#);
        touch(&d.join("node_modules/lodash/lodash.js"), "");
        touch(&d.join("node_modules/@scope/pkg/package.json"), r#"{"exports": {".": {"import": "./esm/index.mjs"}}}"#);
        touch(&d.join("node_modules/@scope/pkg/esm/index.mjs"), "");
        let from = d.join("src/app.ts");
        assert_eq!(resolve("./util", &from, &d), Some(d.join("src/util.ts")));
        assert_eq!(resolve("./util.js", &from, &d), Some(d.join("src/util.ts")));
        assert_eq!(resolve("./comp", &from, &d), Some(d.join("src/comp/index.tsx")));
        assert_eq!(resolve("@/util", &from, &d), Some(d.join("src/util.ts")));
        assert_eq!(resolve("lodash", &from, &d), Some(d.join("node_modules/lodash/lodash.js")));
        assert_eq!(resolve("@scope/pkg", &from, &d), Some(d.join("node_modules/@scope/pkg/esm/index.mjs")));
        assert_eq!(resolve("./missing", &from, &d), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn imported_names() {
        let t = "import React, { useState as useS, type FC } from 'react';\nimport * as api from \"./api\";\nimport {\n  helper,\n  other,\n} from './util';\nconst { join } = require('path');\nexport { x as y } from './x';\n";
        assert_eq!(import_of(t, "React"), Some(("react".into(), "default".into())));
        assert_eq!(import_of(t, "useS"), Some(("react".into(), "useState".into())));
        assert_eq!(import_of(t, "FC"), Some(("react".into(), "FC".into())));
        assert_eq!(import_of(t, "api"), Some(("./api".into(), "*".into())));
        assert_eq!(import_of(t, "other"), Some(("./util".into(), "other".into())));
        assert_eq!(import_of(t, "join"), Some(("path".into(), "join".into())));
        assert_eq!(import_of(t, "y"), Some(("./x".into(), "x".into())));
        assert_eq!(import_of(t, "nothing"), None);
    }
}
