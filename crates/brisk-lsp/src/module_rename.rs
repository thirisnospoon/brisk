//! Import updates for a moved Python module or package done by Brisk itself, for ty
//! versions that answer `workspace/willRenameFiles` with nothing (0.0.104 does so for every
//! folder and for any module moved to another folder).

use brisk_lint::{Edit, Pos};
use std::path::Path;

/// Dotted module name of `path` (a `.py` / `.pyi` file or a package folder) inside the
/// project `root`, as ty resolves first-party code: from the root, or from `src/`.
pub fn module_name(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    if parts.len() > 1 && parts[0] == "src" {
        parts.remove(0);
    }
    let last = parts.pop()?;
    let stem = last.strip_suffix(".pyi").or_else(|| last.strip_suffix(".py")).unwrap_or(&last).to_string();
    if stem != "__init__" {
        parts.push(stem);
    }
    let ok = !parts.is_empty() && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c == '_' || c.is_alphanumeric()));
    ok.then(|| parts.join("."))
}

fn ident(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// Char columns of `name` in `line` from `from`, where it stands as a whole dotted name
/// (or the start of a longer one): not preceded by an identifier char or '.', followed by
/// '.' or a non-identifier char.
fn find_names(line: &[char], name: &[char], from: usize, to: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = from;
    while i + name.len() <= to {
        if line[i..i + name.len()] == *name {
            let before_ok = i == 0 || !(ident(line[i - 1]) || line[i - 1] == '.');
            let after_ok = line.get(i + name.len()).is_none_or(|&c| !ident(c));
            if before_ok && after_ok {
                out.push(i);
                i += name.len();
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Edits renaming module `old` to `new` in one file: in `import` / `from … import` lines,
/// and in qualified uses (`old.attr`) when the file binds the dotted name with a plain
/// `import old…`. `from parent import last` is updated when only the last part changes.
pub fn rename_module_refs(text: &str, old: &str, new: &str) -> Vec<Edit> {
    let oldc: Vec<char> = old.chars().collect();
    let (old_parent, old_last) = old.rsplit_once('.').map_or(("", old), |(p, l)| (p, l));
    let (new_parent, new_last) = new.rsplit_once('.').map_or(("", new), |(p, l)| (p, l));
    let mut edits = Vec::new();
    let mut qualified = false;
    let mut code_lines = Vec::new();
    let edit = |line: usize, col: usize, len: usize, content: &str| Edit { start: Pos { line, col }, end: Pos { line, col: col + len }, content: content.to_string() };
    for (ln, raw) in text.lines().enumerate() {
        let line: Vec<char> = raw.chars().collect();
        let code_end = line.iter().position(|&c| c == '#').unwrap_or(line.len());
        let indent = line.iter().take_while(|c| c.is_whitespace()).count();
        let rest: String = line[indent..code_end].iter().collect();
        if let Some(after) = rest.strip_prefix("from ") {
            let module_start = indent + 5 + (after.len() - after.trim_start().len());
            let module: String = line[module_start..code_end].iter().take_while(|&&c| ident(c) || c == '.').collect();
            if module.starts_with('.') {
                continue;
            }
            if module == old || module.starts_with(&format!("{old}.")) {
                edits.push(edit(ln, module_start, oldc.len(), new));
            } else if !old_parent.is_empty() && module == old_parent && old_parent == new_parent {
                // `from pkg import util` when `pkg.util` becomes `pkg.helpers`.
                let names_from = module_start + module.chars().count();
                for at in find_names(&line, &old_last.chars().collect::<Vec<_>>(), names_from, code_end) {
                    edits.push(edit(ln, at, old_last.chars().count(), new_last));
                }
            }
        } else if rest.starts_with("import ") {
            for at in find_names(&line, &oldc, indent + 7, code_end) {
                edits.push(edit(ln, at, oldc.len(), new));
                // A plain `import old…` (no `as`) binds the dotted path used in code.
                let tail: String = line[at..code_end].iter().collect();
                let item = tail.split(',').next().unwrap_or("");
                if !item.contains(" as ") {
                    qualified = true;
                }
            }
        } else {
            code_lines.push((ln, line, code_end));
        }
    }
    if qualified {
        for (ln, line, end) in code_lines {
            for at in find_names(&line, &oldc, 0, end) {
                // Only `old.attr` is a module reference; a bare `old` may be anything.
                if line.get(at + oldc.len()) == Some(&'.') && !in_string(&line[..at]) {
                    edits.push(edit(ln, at, oldc.len(), new));
                }
            }
        }
    }
    edits.sort_by_key(|e| e.start);
    edits
}

/// Inside a string literal at the end of `before` (odd count of unescaped quotes).
fn in_string(before: &[char]) -> bool {
    let count = |q: char| before.iter().enumerate().filter(|&(i, &c)| c == q && (i == 0 || before[i - 1] != '\\')).count();
    count('"') % 2 == 1 || count('\'') % 2 == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use brisk_lint::apply_edits;
    use std::path::PathBuf;

    #[test]
    fn module_names() {
        let root = PathBuf::from("/p");
        assert_eq!(module_name(&root, &root.join("pkg")).as_deref(), Some("pkg"));
        assert_eq!(module_name(&root, &root.join("pkg").join("util.py")).as_deref(), Some("pkg.util"));
        assert_eq!(module_name(&root, &root.join("src").join("app").join("__init__.py")).as_deref(), Some("app"));
        assert_eq!(module_name(&root, &root.join("my-scripts")), None);
    }

    #[test]
    fn package_rename() {
        let text = "from smokepkg.helpers import add\nimport smokepkg.helpers\nimport smokepkg as sp, os\nfrom .smokepkg import x\n\nprint(add(1, 2), smokepkg.helpers.add, \"smokepkg.x\")  # smokepkg.y\nsmokepkgs.a = smokepkg\n";
        let out = apply_edits(text, &rename_module_refs(text, "smokepkg", "smokelib"));
        assert_eq!(out, "from smokelib.helpers import add\nimport smokelib.helpers\nimport smokelib as sp, os\nfrom .smokepkg import x\n\nprint(add(1, 2), smokelib.helpers.add, \"smokepkg.x\")  # smokepkg.y\nsmokepkgs.a = smokepkg\n");
    }

    #[test]
    fn module_rename_in_package() {
        let text = "from pkg import util, other\nfrom pkg.util import add\nimport pkg.util\nx = pkg.util.add\n";
        let out = apply_edits(text, &rename_module_refs(text, "pkg.util", "pkg.helpers"));
        assert_eq!(out, "from pkg import helpers, other\nfrom pkg.helpers import add\nimport pkg.helpers\nx = pkg.helpers.add\n");
        // Aliased only: code uses the alias, nothing qualified to change.
        let text = "import pkg.util as u\nu.add\n";
        assert_eq!(apply_edits(text, &rename_module_refs(text, "pkg.util", "lib.util")), "import lib.util as u\nu.add\n");
    }
}
