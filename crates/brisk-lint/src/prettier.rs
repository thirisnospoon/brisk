//! Prettier from the project's `node_modules`, as PyCharm uses it for Reformat Code when a
//! project has it: the project's own version and config (`.prettierrc`, `package.json`),
//! run with Node.js on the text, never installed by Brisk.

use std::path::{Path, PathBuf};

/// The file types Prettier formats out of the box.
pub fn supports(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    matches!(
        ext.as_str(),
        "js" | "jsx"
            | "mjs"
            | "cjs"
            | "ts"
            | "tsx"
            | "mts"
            | "cts"
            | "json"
            | "jsonc"
            | "json5"
            | "css"
            | "scss"
            | "less"
            | "html"
            | "htm"
            | "vue"
            | "yaml"
            | "yml"
            | "md"
            | "markdown"
            | "graphql"
            | "gql"
            | "hbs"
            | "handlebars"
    ) && !path.to_string_lossy().ends_with(".min.js")
}

/// Prettier's command-line script in the nearest `node_modules` from the file's folder up
/// to `root` (Prettier 3's `bin/prettier.cjs`, Prettier 2's `bin-prettier.js`).
pub fn find(file: &Path, root: &Path) -> Option<PathBuf> {
    let dir = file.parent()?;
    dir.ancestors()
        .take_while(|d| d.starts_with(root) || *d == dir)
        .map(|d| d.join("node_modules").join("prettier"))
        .flat_map(|p| [p.join("bin").join("prettier.cjs"), p.join("bin-prettier.js")])
        .find(|p| p.is_file())
}

/// The text formatted by Prettier, which reads the project's config for `file`.
pub fn format(node: &Path, script: &Path, root: &Path, file: &Path, text: &str) -> Result<String, String> {
    let mut c = super::command(node, root);
    c.arg(script).arg("--stdin-filepath").arg(file);
    super::run(c, Some(text)).map_err(|e| format!("Prettier: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_nearest() {
        let d = std::env::temp_dir().join(format!("brisk-prettier-{}", std::process::id()));
        let bin = d.join("node_modules/prettier/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("prettier.cjs"), "").unwrap();
        std::fs::create_dir_all(d.join("src")).unwrap();
        assert_eq!(find(&d.join("src/a.ts"), &d), Some(bin.join("prettier.cjs")));
        assert!(supports(Path::new("a.tsx")) && supports(Path::new("x.yml")) && !supports(Path::new("a.py")) && !supports(Path::new("a.min.js")));
        let _ = std::fs::remove_dir_all(&d);
    }
}
