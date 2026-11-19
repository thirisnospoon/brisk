//! Code › Generate (Alt+Insert), Override Methods (Ctrl+O) and Implement Methods (Ctrl+I) for
//! Python classes: what a class has (fields, methods, bases) and the text of the new members.
//! Generated text has no indent; the caller indents it to the class body.

use tree_sitter::{Node, Tree};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub annotation: Option<String>,
    pub default: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Method {
    pub name: String,
    /// The text between the parentheses of the `def`.
    pub params: String,
    pub returns: Option<String>,
    pub decorators: Vec<String>,
    pub is_async: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassInfo {
    pub name: String,
    /// Base class names (last dotted part) and the byte of each, for ty's go to definition.
    pub bases: Vec<(String, usize)>,
    /// The body's indent.
    pub indent: String,
    /// New members go after this row; a lone `pass` body (`pass_row`) is replaced instead.
    pub insert_after: usize,
    pub pass_row: Option<usize>,
    /// Class-level annotated or assigned names, in order.
    pub class_fields: Vec<Field>,
    /// `self.x` assigned in `__init__`, in order.
    pub self_fields: Vec<String>,
    pub methods: Vec<Method>,
}

fn text_of(n: Node, text: &str) -> String {
    text.get(n.byte_range()).unwrap_or("").to_string()
}

fn kids<'t>(n: Node<'t>) -> Vec<Node<'t>> {
    (0..n.named_child_count()).filter_map(|i| n.named_child(i as _)).collect()
}

/// A `def` (possibly decorated) as a [`Method`].
fn method(n: Node, text: &str) -> Option<Method> {
    let (def, decorators) = if n.kind() == "decorated_definition" {
        let decs = kids(n).into_iter().filter(|c| c.kind() == "decorator").map(|d| text_of(d, text)).collect();
        (n.child_by_field_name("definition")?, decs)
    } else {
        (n, Vec::new())
    };
    if def.kind() != "function_definition" {
        return None;
    }
    let params = def.child_by_field_name("parameters").map(|p| text_of(p, text)).unwrap_or_default();
    let params = params.strip_prefix('(').and_then(|p| p.strip_suffix(')')).unwrap_or(&params).to_string();
    Some(Method {
        name: text_of(def.child_by_field_name("name")?, text),
        params,
        returns: def.child_by_field_name("return_type").map(|r| text_of(r, text)),
        decorators,
        is_async: text.get(def.start_byte()..).is_some_and(|t| t.starts_with("async")),
    })
}

fn info(class: Node, text: &str, caret_row: Option<usize>) -> Option<ClassInfo> {
    let body = class.child_by_field_name("body")?;
    let members = kids(body);
    let lines: Vec<&str> = text.split('\n').collect();
    let first = members.first()?;
    let indent = lines.get(first.start_position().row).map(|l| l[..l.len() - l.trim_start().len()].to_string()).unwrap_or_default();
    let bases = class
        .child_by_field_name("superclasses")
        .map(|s| {
            kids(s)
                .into_iter()
                .filter(|b| matches!(b.kind(), "identifier" | "attribute"))
                .map(|b| {
                    let last = if b.kind() == "attribute" { b.child_by_field_name("attribute").unwrap_or(b) } else { b };
                    (text_of(last, text), last.start_byte())
                })
                .collect()
        })
        .unwrap_or_default();
    let mut c = ClassInfo {
        name: text_of(class.child_by_field_name("name")?, text),
        bases,
        indent,
        insert_after: members.last()?.end_position().row,
        pass_row: (members.len() == 1 && first.kind() == "pass_statement").then(|| first.start_position().row),
        class_fields: Vec::new(),
        self_fields: Vec::new(),
        methods: Vec::new(),
    };
    for m in &members {
        // After the method under the caret (fields stay together: else at the end).
        let is_def = matches!(m.kind(), "function_definition" | "decorated_definition");
        if let Some(row) = caret_row.filter(|&r| is_def && r >= m.start_position().row && r <= m.end_position().row) {
            c.insert_after = m.end_position().row.max(row);
        }
        if let Some(def) = method(*m, text) {
            if def.name == "__init__" {
                let def_node = if m.kind() == "decorated_definition" { m.child_by_field_name("definition") } else { Some(*m) };
                if let Some(b) = def_node.and_then(|d| d.child_by_field_name("body")) {
                    self_assignments(b, text, &mut c.self_fields);
                }
            }
            c.methods.push(def);
            continue;
        }
        let Some(a) = (m.kind() == "expression_statement").then(|| m.named_child(0)).flatten().filter(|a| a.kind() == "assignment") else { continue };
        let Some(left) = a.child_by_field_name("left").filter(|l| l.kind() == "identifier") else { continue };
        let name = text_of(left, text);
        if name.starts_with("__") || c.class_fields.iter().any(|f| f.name == name) {
            continue;
        }
        c.class_fields.push(Field { name, annotation: a.child_by_field_name("type").map(|t| text_of(t, text)), default: a.child_by_field_name("right").map(|r| text_of(r, text)) });
    }
    Some(c)
}

/// `self.x = ...` targets anywhere in `n` (not in nested functions or classes).
fn self_assignments(n: Node, text: &str, out: &mut Vec<String>) {
    for c in kids(n) {
        if matches!(c.kind(), "function_definition" | "class_definition" | "lambda") {
            continue;
        }
        if c.kind() == "assignment" || c.kind() == "augmented_assignment" {
            if let Some(l) = c.child_by_field_name("left").filter(|l| l.kind() == "attribute") {
                let obj = l.child_by_field_name("object").map(|o| text_of(o, text));
                if let (Some("self"), Some(attr)) = (obj.as_deref(), l.child_by_field_name("attribute")) {
                    let name = text_of(attr, text);
                    if !out.contains(&name) {
                        out.push(name);
                    }
                }
            }
        }
        self_assignments(c, text, out);
    }
}

/// The class around `byte`, with new members going after the member under the caret. A
/// caret at the end of the class's last line (just past its node) counts as inside.
pub fn class_at(tree: &Tree, text: &str, byte: usize) -> Option<ClassInfo> {
    let found = class_from(tree, text, byte);
    found.or_else(|| class_from(tree, text, byte.checked_sub(1)?))
}

fn class_from(tree: &Tree, text: &str, byte: usize) -> Option<ClassInfo> {
    let mut n = tree.root_node().descendant_for_byte_range(byte, byte);
    while let Some(c) = n {
        if c.kind() == "class_definition" {
            return info(c, text, Some(text[..byte.min(text.len())].matches('\n').count()));
        }
        n = c.parent();
    }
    None
}

/// The class defined at `row` (its `class` line or a decorator line), or named `name`.
pub fn class_named(tree: &Tree, text: &str, name: &str, row: Option<usize>) -> Option<ClassInfo> {
    let mut stack = vec![tree.root_node()];
    while let Some(n) = stack.pop() {
        if n.kind() == "class_definition" {
            let hit = match row {
                Some(r) => (n.start_position().row..=n.child_by_field_name("name").map(|x| x.start_position().row).unwrap_or(0)).contains(&r),
                None => n.child_by_field_name("name").is_some_and(|x| text_of(x, text) == name),
            };
            if hit {
                return info(n, text, None);
            }
        }
        if !matches!(n.kind(), "function_definition") {
            stack.extend(kids(n).into_iter().rev());
        }
    }
    None
}

/// Fields shown by `__repr__` / compared by `__eq__`: class fields, then `self.x` of `__init__`.
pub fn data_fields(c: &ClassInfo) -> Vec<String> {
    let mut v: Vec<String> = c.class_fields.iter().map(|f| f.name.clone()).collect();
    for f in &c.self_fields {
        if !v.contains(f) {
            v.push(f.clone());
        }
    }
    v
}

/// `__init__` taking the class fields (those without a default first).
pub fn gen_init(c: &ClassInfo) -> String {
    let mut fields: Vec<&Field> = c.class_fields.iter().collect();
    fields.sort_by_key(|f| f.default.is_some());
    let mut params = vec!["self".to_string()];
    for f in &fields {
        params.push(match (&f.annotation, &f.default) {
            (Some(a), Some(d)) => format!("{}: {a} = {d}", f.name),
            (Some(a), None) => format!("{}: {a}", f.name),
            (None, Some(d)) => format!("{}={d}", f.name),
            (None, None) => f.name.clone(),
        });
    }
    let body: Vec<String> = fields.iter().map(|f| format!("    self.{0} = {0}", f.name)).collect();
    let body = if body.is_empty() { "    pass".to_string() } else { body.join("\n") };
    format!("def __init__({}):\n{body}", params.join(", "))
}

pub fn gen_repr(c: &ClassInfo) -> String {
    let parts: Vec<String> = data_fields(c).iter().map(|f| format!("{f}={{self.{f}!r}}")).collect();
    format!("def __repr__(self):\n    return f\"{}({})\"", c.name, parts.join(", "))
}

pub fn gen_eq(c: &ClassInfo) -> String {
    let f = data_fields(c);
    let cmp = match f.len() {
        0 => "True".to_string(),
        1 => format!("self.{0} == other.{0}", f[0]),
        _ => {
            let side = |o: &str| f.iter().map(|x| format!("{o}.{x}")).collect::<Vec<_>>().join(", ");
            format!("({}) == ({})", side("self"), side("other"))
        }
    };
    format!("def __eq__(self, other):\n    if not isinstance(other, {}):\n        return NotImplemented\n    return {cmp}", c.name)
}

/// A getter and setter for the private attribute `field` (`_x` -> `x`).
pub fn gen_property(field: &str) -> String {
    let p = field.trim_start_matches('_');
    format!("@property\ndef {p}(self):\n    return self.{field}\n\n@{p}.setter\ndef {p}(self, value):\n    self.{field} = value")
}

/// Private `self._x` attributes that have no property yet.
pub fn property_candidates(c: &ClassInfo) -> Vec<String> {
    c.self_fields.iter().filter(|f| f.starts_with('_') && !f.starts_with("__") && !c.methods.iter().any(|m| m.name == f.trim_start_matches('_'))).cloned().collect()
}

/// Split parameters at top-level commas.
fn split_params(params: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth, mut quote) = (Vec::new(), String::new(), 0i32, None::<char>);
    for ch in params.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(ch),
            (None, '(' | '[' | '{') => depth += 1,
            (None, ')' | ']' | '}') => depth -= 1,
            (None, ',') if depth == 0 => {
                out.push(std::mem::take(&mut cur).trim().to_string());
                continue;
            }
            _ => {}
        }
        cur.push(ch);
    }
    out.push(cur.trim().to_string());
    out.retain(|p| !p.is_empty());
    out
}

/// An override of `m` from class `base` calling the base version, as PyCharm writes it.
pub fn gen_override(m: &Method, base: &str) -> String {
    let is_static = m.decorators.iter().any(|d| d.contains("staticmethod"));
    let mut args = Vec::new();
    let mut kw_only = false;
    for (i, p) in split_params(&m.params).into_iter().enumerate() {
        let name = p.split([':', '=']).next().unwrap_or("").trim().to_string();
        // `__new__` passes `cls` on: `super().__new__(cls, ...)`.
        if i == 0 && !is_static && (name == "self" || name == "cls") && m.name != "__new__" || name == "/" {
            continue;
        }
        if name == "*" {
            kw_only = true;
            continue;
        }
        if name.starts_with('*') {
            kw_only = true;
            args.push(name);
        } else if kw_only {
            args.push(format!("{name}={name}"));
        } else {
            args.push(name);
        }
    }
    let target = if is_static { base.to_string() } else { "super()".to_string() };
    let call = format!("{}{target}.{}({})", if m.is_async { "await " } else { "" }, m.name, args.join(", "));
    let abstract_ = m.decorators.iter().any(|d| d.contains("abstractmethod"));
    let body = if abstract_ {
        "pass".to_string()
    } else if m.name == "__init__" || m.returns.as_deref() == Some("None") {
        call
    } else {
        format!("return {call}")
    };
    let decorators: String = m.decorators.iter().filter(|d| !d.contains("abstractmethod")).map(|d| format!("{d}\n")).collect();
    let returns = m.returns.as_ref().map(|r| format!(" -> {r}")).unwrap_or_default();
    format!("{decorators}{}def {}({}){returns}:\n    {body}", if m.is_async { "async " } else { "" }, m.name, m.params)
}

/// `object`'s methods, offered by Override Methods for every class (after the bases' own).
pub fn object_methods() -> Vec<Method> {
    [
        ("__init__", "self", None),
        ("__new__", "cls, *args, **kwargs", None),
        ("__repr__", "self", Some("str")),
        ("__str__", "self", Some("str")),
        ("__eq__", "self, other", Some("bool")),
        ("__ne__", "self, other", Some("bool")),
        ("__hash__", "self", Some("int")),
        ("__format__", "self, format_spec", Some("str")),
        ("__getattribute__", "self, name", None),
        ("__setattr__", "self, name, value", Some("None")),
        ("__delattr__", "self, name", Some("None")),
        ("__dir__", "self", None),
        ("__sizeof__", "self", Some("int")),
        ("__reduce__", "self", None),
    ]
    .into_iter()
    .map(|(name, params, returns)| Method { name: name.into(), params: params.into(), returns: returns.map(Into::into), decorators: Vec::new(), is_async: false })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "class Point(Base, mod.Mixin):\n    x: int\n    y: int = 0\n    label = \"p\"\n\n    def __init__(self, x, y):\n        self.x = x\n        self._hidden = 1\n\n    def area(self) -> float:\n        return 0\n";

    fn class() -> ClassInfo {
        let t = crate::parse_full(crate::Lang::Python, SRC, None).unwrap();
        class_at(&t, SRC, SRC.find("return 0").unwrap()).unwrap()
    }

    #[test]
    fn reads_a_class() {
        let c = class();
        assert_eq!(c.name, "Point");
        assert_eq!(c.bases.iter().map(|b| b.0.as_str()).collect::<Vec<_>>(), ["Base", "Mixin"]);
        assert_eq!((c.indent.as_str(), c.insert_after, c.pass_row), ("    ", 10, None));
        assert_eq!(c.class_fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["x", "y", "label"]);
        assert_eq!(c.self_fields, ["x", "_hidden"]);
        assert_eq!(c.methods.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["__init__", "area"]);
        assert_eq!(property_candidates(&c), ["_hidden"]);
    }

    #[test]
    fn generated_members() {
        let c = class();
        assert_eq!(gen_init(&c), "def __init__(self, x: int, y: int = 0, label=\"p\"):\n    self.x = x\n    self.y = y\n    self.label = label");
        assert_eq!(gen_repr(&c), "def __repr__(self):\n    return f\"Point(x={self.x!r}, y={self.y!r}, label={self.label!r}, _hidden={self._hidden!r})\"");
        assert!(gen_eq(&c).ends_with("return (self.x, self.y, self.label, self._hidden) == (other.x, other.y, other.label, other._hidden)"));
        let m = Method { name: "run".into(), params: "self, a: int, *args, key=None, **kw".into(), returns: Some("str".into()), decorators: vec![], is_async: false };
        assert_eq!(gen_override(&m, "Base"), "def run(self, a: int, *args, key=None, **kw) -> str:\n    return super().run(a, *args, key=key, **kw)");
        let m = Method { name: "step".into(), params: "self, x".into(), returns: None, decorators: vec!["@abstractmethod".into()], is_async: true };
        assert_eq!(gen_override(&m, "Base"), "async def step(self, x):\n    pass");
        let new = object_methods().into_iter().find(|m| m.name == "__new__").unwrap();
        assert_eq!(gen_override(&new, "object"), "def __new__(cls, *args, **kwargs):\n    return super().__new__(cls, *args, **kwargs)");
    }
}
