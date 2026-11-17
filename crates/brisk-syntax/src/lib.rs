//! Syntax highlighting and structure via tree-sitter.
//!
//! Highlighting is computed only for the visible byte range, so cost does not grow with file size.

mod sql;
pub use sql::highlight_sql;

use std::ops::Range;
use std::path::Path;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor, Tree};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Highlight {
    Keyword,
    Function,
    FunctionBuiltin,
    Method,
    Type,
    String,
    Escape,
    Comment,
    Number,
    Constant,
    ConstantBuiltin,
    Property,
    Decorator,
    Operator,
    Punctuation,
    SelfParam,
}

fn classify(capture: &str) -> Option<Highlight> {
    use Highlight::*;
    Some(match capture {
        c if c.starts_with("keyword") => Keyword,
        // HTML/CSS tags, CSS at-rules, Markdown headings.
        c if c.starts_with("tag") => Keyword,
        "charset" | "import" | "keyframes" | "media" | "namespace" | "supports" => Keyword,
        "text.title" => Keyword,
        "text.literal" => String,
        "text.uri" => Constant,
        "text.reference" => Function,
        "boolean" => ConstantBuiltin,
        // JSON / YAML keys.
        "string.special.key" | "label" => Property,
        "decorator" => Decorator,
        "function.builtin" => FunctionBuiltin,
        "function.method" | "method" => Method,
        c if c.starts_with("function") || c == "constructor" => Function,
        c if c.starts_with("type") => Type,
        "string.escape" | "escape" => Escape,
        c if c.starts_with("string") => String,
        c if c.starts_with("comment") => Comment,
        "number" | "float" => Number,
        "constant.builtin" => ConstantBuiltin,
        c if c.starts_with("constant") => Constant,
        "property" | "attribute" => Property,
        "punctuation.special" | "embedded" => Punctuation,
        "operator" => Operator,
        c if c.starts_with("punctuation") => Punctuation,
        "variable.builtin" => SelfParam,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Python,
    Json,
    Toml,
    Yaml,
    Markdown,
    Html,
    Css,
    JavaScript,
    TypeScript,
    Tsx,
    Shell,
}

impl Lang {
    pub fn from_path(p: &Path) -> Option<Self> {
        let name = p.file_name()?.to_str()?.to_ascii_lowercase();
        let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        Some(match ext {
            "py" | "pyi" | "pyw" => Lang::Python,
            "json" | "jsonc" | "ipynb" | "geojson" | "babelrc" | "eslintrc" => Lang::Json,
            "toml" => Lang::Toml,
            "yaml" | "yml" => Lang::Yaml,
            "md" | "markdown" => Lang::Markdown,
            "html" | "htm" | "xhtml" | "jinja" | "jinja2" | "j2" => Lang::Html,
            "css" => Lang::Css,
            "js" | "mjs" | "cjs" | "jsx" => Lang::JavaScript,
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "sh" | "bash" | "zsh" => Lang::Shell,
            _ => match name.as_str() {
                "pipfile" | "poetry.lock" | "uv.lock" => Lang::Toml,
                ".bashrc" | ".profile" | ".zshrc" | ".envrc" => Lang::Shell,
                _ => return None,
            },
        })
    }

    /// Status bar name.
    pub fn name(self) -> &'static str {
        match self {
            Lang::Python => "Python",
            Lang::Json => "JSON",
            Lang::Toml => "TOML",
            Lang::Yaml => "YAML",
            Lang::Markdown => "Markdown",
            Lang::Html => "HTML",
            Lang::Css => "CSS",
            Lang::JavaScript => "JavaScript",
            Lang::TypeScript => "TypeScript",
            Lang::Tsx => "TypeScript JSX",
            Lang::Shell => "Shell Script",
        }
    }

    /// Line comment prefix for Ctrl+/ (None: block comments only).
    pub fn line_comment(self) -> Option<&'static str> {
        match self {
            Lang::Python | Lang::Toml | Lang::Yaml | Lang::Shell => Some("#"),
            Lang::JavaScript | Lang::TypeScript | Lang::Tsx | Lang::Json => Some("//"),
            Lang::Markdown | Lang::Html | Lang::Css => None,
        }
    }

    fn ts(self) -> Language {
        match self {
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::Json => tree_sitter_json::LANGUAGE.into(),
            Lang::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Lang::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Lang::Markdown => tree_sitter_md::LANGUAGE.into(),
            Lang::Html => tree_sitter_html::LANGUAGE.into(),
            Lang::Css => tree_sitter_css::LANGUAGE.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::Shell => tree_sitter_bash::LANGUAGE.into(),
        }
    }
    fn highlights_query(self) -> String {
        match self {
            Lang::Python => tree_sitter_python::HIGHLIGHTS_QUERY.into(),
            Lang::Json => tree_sitter_json::HIGHLIGHTS_QUERY.into(),
            // The bundled query paints whole pairs as keys.
            Lang::Toml => TOML_QUERY.into(),
            Lang::Yaml => tree_sitter_yaml::HIGHLIGHTS_QUERY.into(),
            Lang::Markdown => tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.into(),
            Lang::Html => tree_sitter_html::HIGHLIGHTS_QUERY.into(),
            Lang::Css => tree_sitter_css::HIGHLIGHTS_QUERY.into(),
            Lang::JavaScript => format!("{}
{}", tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_javascript::JSX_HIGHLIGHT_QUERY),
            // The TypeScript query extends the JavaScript one.
            Lang::TypeScript => format!("{}
{}", tree_sitter_typescript::HIGHLIGHTS_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY),
            Lang::Tsx => format!(
                "{}
{}
{}",
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY
            ),
            Lang::Shell => tree_sitter_bash::HIGHLIGHT_QUERY.into(),
        }
    }
    /// Extra patterns on top of the grammar's query, to look closer to PyCharm.
    fn extra_query(self) -> &'static str {
        match self {
            Lang::Python => {
                r#"
                ((identifier) @variable.builtin (#match? @variable.builtin "^(self|cls)$"))
                (decorator) @decorator
                (decorator (identifier) @decorator)
                (decorator (attribute) @decorator)
                (decorator (call function: (_) @decorator))
                (keyword_argument name: (identifier) @property)
                ["def" "class" "if" "elif" "else" "for" "while" "return" "import" "from" "as"
                 "try" "except" "finally" "with" "pass" "break" "continue" "raise" "lambda"
                 "yield" "await" "async" "global" "nonlocal" "assert" "del" "in" "not" "and" "or" "is"] @keyword
                "#
            }
            Lang::Html => "(doctype) @keyword",
            _ => "",
        }
    }
}

const TOML_QUERY: &str = r#"
(table (bare_key) @type)
(table (dotted_key (bare_key) @type))
(table_array_element (bare_key) @type)
(table_array_element (dotted_key (bare_key) @type))
(pair (bare_key) @property)
(pair (dotted_key (bare_key) @property))
(quoted_key) @string
(boolean) @boolean
(comment) @comment
(string) @string
[(integer) (float)] @number
[(offset_date_time) (local_date_time) (local_date) (local_time)] @string.special
["." ","] @punctuation.delimiter
"=" @operator
["[" "]" "[[" "]]" "{" "}"] @punctuation.bracket
"#;

pub struct SyntaxTree {
    lang: Lang,
    parser: Parser,
    query: Query,
    names: Vec<Option<Highlight>>,
    tree: Option<Tree>,
}

/// A highlighted span, in **byte** offsets of the source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub kind: Highlight,
}

impl SyntaxTree {
    pub fn new(lang: Lang) -> Self {
        let mut parser = Parser::new();
        parser.set_language(&lang.ts()).expect("grammar version mismatch");
        let src = format!("{}\n{}", lang.highlights_query(), lang.extra_query());
        let query = Query::new(&lang.ts(), &src).expect("bad highlight query");
        let names = query.capture_names().iter().map(|n| classify(n)).collect();
        Self { lang, parser, query, names, tree: None }
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// Full reparse. Incremental `edit()` comes with the LSP/indexer work; tree-sitter reparses
    /// a 10k-line file in a few milliseconds anyway.
    pub fn parse(&mut self, text: &str) {
        self.tree = self.parser.parse(text, None);
    }

    pub fn tree(&self) -> Option<&Tree> {
        self.tree.as_ref()
    }

    /// Spans intersecting `bytes`, non-overlapping and sorted. Innermost capture wins.
    pub fn highlights(&self, text: &str, bytes: Range<usize>) -> Vec<Span> {
        let Some(tree) = &self.tree else { return Vec::new() };
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(bytes.clone());
        let mut raw: Vec<(Range<usize>, usize, Highlight)> = Vec::new();
        let mut it = cursor.captures(&self.query, tree.root_node(), text.as_bytes());
        while let Some((m, idx)) = it.next() {
            let cap = m.captures[*idx];
            if let Some(kind) = self.names[cap.index as usize] {
                let r = cap.node.byte_range();
                raw.push((r, m.pattern_index, kind));
            }
        }
        if raw.is_empty() {
            return Vec::new();
        }
        // Paint: larger nodes first, then smaller ones on top. Among equal ranges the earliest
        // pattern wins (painted last), matching tree-sitter-highlight precedence.
        // More specific kinds beat generic ones on the same node regardless of query order.
        // Data languages (JSON, YAML): a key is a string too, and the key color wins.
        let keys_win = self.lang != Lang::Python;
        let specificity = |k: Highlight| -> u8 {
            use Highlight::*;
            match k {
                SelfParam | FunctionBuiltin | ConstantBuiltin | Escape => 2,
                Method | Decorator | Keyword => 1,
                Property if keys_win => 1,
                _ => 0,
            }
        };
        raw.sort_by(|a, b| {
            (b.0.len())
                .cmp(&a.0.len())
                .then_with(|| specificity(a.2).cmp(&specificity(b.2)))
                .then_with(|| b.1.cmp(&a.1))
        });
        let lo = bytes.start;
        let hi = bytes.end.min(text.len());
        if hi <= lo {
            return Vec::new();
        }
        let mut paint: Vec<Option<Highlight>> = vec![None; hi - lo];
        for (r, _, k) in raw {
            let s = r.start.max(lo);
            let e = r.end.min(hi);
            for p in &mut paint[s.saturating_sub(lo)..e.saturating_sub(lo)] {
                *p = Some(k);
            }
        }
        let mut out: Vec<Span> = Vec::new();
        let mut i = 0;
        while i < paint.len() {
            if let Some(k) = paint[i] {
                let st = i;
                while i < paint.len() && paint[i] == Some(k) {
                    i += 1;
                }
                out.push(Span { range: lo + st..lo + i, kind: k });
            } else {
                i += 1;
            }
        }
        out
    }

    /// Top-level and nested classes/functions for the Structure tool window (Alt+7).
    pub fn outline(&self, text: &str) -> Vec<OutlineItem> {
        let mut out = Vec::new();
        if let Some(tree) = &self.tree {
            let mut stack = vec![(tree.root_node(), 0usize)];
            while let Some((node, depth)) = stack.pop() {
                let kind = match node.kind() {
                    "class_definition" => Some(OutlineKind::Class),
                    "function_definition" => Some(if depth > 0 { OutlineKind::Method } else { OutlineKind::Function }),
                    _ => None,
                };
                let child_depth = if kind.is_some() { depth + 1 } else { depth };
                if let (Some(kind), Some(name)) = (kind, node.child_by_field_name("name")) {
                    out.push(OutlineItem {
                        name: text[name.byte_range()].to_string(),
                        kind,
                        depth,
                        byte: name.start_byte(),
                        line: name.start_position().row,
                        end_line: node.end_position().row,
                    });
                }
                let mut c = node.walk();
                let children: Vec<_> = node.children(&mut c).collect();
                for ch in children.into_iter().rev() {
                    stack.push((ch, child_depth));
                }
            }
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineKind {
    Class,
    Function,
    Method,
}

#[derive(Clone, Debug)]
pub struct OutlineItem {
    pub name: String,
    pub kind: OutlineKind,
    pub depth: usize,
    pub byte: usize,
    pub line: usize,
    /// Last line of the whole definition (body included).
    pub end_line: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "import os\n\n@dataclass\nclass User:\n    def name(self) -> str:\n        # hi\n        return \"x\" + str(42)\n";

    #[test]
    fn highlights_python() {
        let mut s = SyntaxTree::new(Lang::Python);
        s.parse(SRC);
        let spans = s.highlights(SRC, 0..SRC.len());
        let kind_of = |needle: &str| {
            let at = SRC.find(needle).unwrap();
            spans.iter().find(|sp| sp.range.contains(&at)).map(|sp| sp.kind)
        };
        assert_eq!(kind_of("import"), Some(Highlight::Keyword));
        assert_eq!(kind_of("class User"), Some(Highlight::Keyword));
        assert_eq!(kind_of("@dataclass"), Some(Highlight::Decorator));
        assert_eq!(kind_of("# hi"), Some(Highlight::Comment));
        assert_eq!(kind_of("\"x\""), Some(Highlight::String));
        assert_eq!(kind_of("42"), Some(Highlight::Number));
        assert_eq!(kind_of("self"), Some(Highlight::SelfParam));
        assert_eq!(kind_of("str(42)"), Some(Highlight::FunctionBuiltin));
        // Non-overlapping and sorted.
        for w in spans.windows(2) {
            assert!(w[0].range.end <= w[1].range.start);
        }
    }

    #[test]
    fn other_languages() {
        let cases: &[(&str, &str, &str, Highlight)] = &[
            ("a.json", "{\"name\": true, \"n\": 1}", "\"name\"", Highlight::Property),
            ("a.json", "{\"name\": true, \"n\": 1}", "true", Highlight::ConstantBuiltin),
            ("pyproject.toml", "[project]
name = \"x\" # c
", "name", Highlight::Property),
            ("pyproject.toml", "[project]
name = \"x\" # c
", "\"x\"", Highlight::String),
            ("ci.yml", "jobs:
  test: 1 # c
", "jobs", Highlight::Property),
            ("ci.yml", "jobs:
  test: 1 # c
", "# c", Highlight::Comment),
            ("README.md", "# Title

```py
x
```
", "Title", Highlight::Keyword),
            ("index.html", "<div class=\"a\">hi</div>", "div", Highlight::Keyword),
            ("index.html", "<div class=\"a\">hi</div>", "class", Highlight::Property),
            ("s.css", ".a { color: red; }", "color", Highlight::Property),
            ("app.js", "const x = 'a'; // c
foo(1);", "const", Highlight::Keyword),
            ("app.js", "const x = 'a'; // c
foo(1);", "'a'", Highlight::String),
            ("app.ts", "interface A { b: number }", "interface", Highlight::Keyword),
            ("app.tsx", "const a = <div>x</div>;", "const", Highlight::Keyword),
            ("run.sh", "echo \"hi\" # c
", "# c", Highlight::Comment),
        ];
        for (file, src, needle, want) in cases {
            let lang = Lang::from_path(Path::new(file)).unwrap();
            let mut s = SyntaxTree::new(lang);
            s.parse(src);
            let spans = s.highlights(src, 0..src.len());
            let at = src.find(needle).unwrap();
            let got = spans.iter().find(|sp| sp.range.contains(&at)).map(|sp| sp.kind);
            assert_eq!(got, Some(*want), "{file}: {needle} in {spans:?}");
        }
    }

    #[test]
    fn outline() {
        let mut s = SyntaxTree::new(Lang::Python);
        s.parse(SRC);
        let o = s.outline(SRC);
        assert_eq!(o.len(), 2);
        assert_eq!((o[0].name.as_str(), o[0].kind), ("User", OutlineKind::Class));
        assert_eq!((o[1].name.as_str(), o[1].kind, o[1].depth), ("name", OutlineKind::Method, 1));
    }
}
