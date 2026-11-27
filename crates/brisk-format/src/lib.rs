//! Reformat Code (`Ctrl+Alt+L`) for files that have no formatter in the project: JSON and
//! JSONC, CSS / SCSS / Less, HTML / XML / SVG, SQL and TOML. Small, dependency-free
//! formatters in PyCharm's default style; Python goes to ruff and JS / TS to Prettier or
//! TypeScript's server in the app. A text that does not parse is an error, never mangled.

mod css;
mod json;
mod markup;
mod sql;
mod toml;

use std::path::Path;

/// What a file is formatted as, from its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Json,
    Css,
    Html,
    Xml,
    Sql,
    Toml,
}

impl Kind {
    pub fn of(path: &Path) -> Option<Kind> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        let name = path.file_name()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "json" | "jsonc" | "json5" | "babelrc" | "eslintrc" | "prettierrc" => Kind::Json,
            "css" | "scss" | "less" | "pcss" | "postcss" => Kind::Css,
            "html" | "htm" | "xhtml" | "vue" | "svelte" => Kind::Html,
            "xml" | "svg" | "xsd" | "xsl" | "xslt" | "plist" | "csproj" | "props" | "targets" | "config" | "iml" | "wsdl" | "pom" => Kind::Xml,
            "sql" | "ddl" | "pgsql" | "mysql" => Kind::Sql,
            "toml" => Kind::Toml,
            _ if matches!(name.as_str(), ".babelrc" | ".eslintrc" | ".prettierrc") => Kind::Json,
            _ => return None,
        })
    }
}

/// Formats `text` as `kind`, indenting with `indent` (the file's unit, `"    "` or `"\t"`).
pub fn format(kind: Kind, text: &str, indent: &str) -> Result<String, String> {
    let out = match kind {
        Kind::Json => json::format(text, indent)?,
        Kind::Css => css::format(text, indent)?,
        Kind::Html => markup::format(text, indent, true)?,
        Kind::Xml => markup::format(text, indent, false)?,
        Kind::Sql => sql::format(text, indent)?,
        Kind::Toml => toml::format(text, indent)?,
    };
    // An empty file stays empty.
    Ok(if text.trim().is_empty() { text.to_string() } else { out })
}

/// The output being built: lines at a depth, no trailing spaces.
pub(crate) struct Out<'a> {
    pub s: String,
    pub depth: usize,
    unit: &'a str,
}

impl<'a> Out<'a> {
    pub fn new(unit: &'a str) -> Self {
        Out { s: String::new(), depth: 0, unit }
    }

    /// Nothing but indentation on the current line.
    pub fn at_line_start(&self) -> bool {
        self.s.rsplit('\n').next().is_none_or(|l| l.trim().is_empty())
    }

    /// Ends the line (dropping its trailing spaces) and indents the next one.
    pub fn newline(&mut self) {
        let t = self.s.trim_end_matches([' ', '\t']).len();
        self.s.truncate(t);
        if self.s.is_empty() {
            return;
        }
        self.s.push('\n');
        for _ in 0..self.depth {
            self.s.push_str(self.unit);
        }
    }

    pub fn push_str(&mut self, t: &str) {
        self.s.push_str(t);
    }

    /// The text with one final line break.
    pub fn finish(self) -> String {
        let mut s = self.s.trim_end().to_string();
        s.push('\n');
        s
    }
}
