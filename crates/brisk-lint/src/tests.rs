use super::*;

const SAMPLE: &str = r#"[
  {"cell": null, "code": "F401", "end_location": {"column": 10, "row": 1}, "filename": "C:/p/a.py",
   "fix": {"applicability": "safe", "edits": [{"content": "", "end_location": {"column": 1, "row": 2}, "location": {"column": 1, "row": 1}}], "message": "Remove unused import: `os`"},
   "location": {"column": 8, "row": 1}, "message": "`os` imported but unused", "name": "unused-import", "noqa_row": 1, "severity": "error", "url": null},
  {"cell": null, "code": null, "end_location": {"column": 8, "row": 3}, "filename": "C:/p/a.py", "fix": null,
   "location": {"column": 7, "row": 3}, "message": "Expected `)`", "name": "invalid-syntax", "noqa_row": null, "severity": "error", "url": null},
  {"cell": null, "code": "E711", "end_location": {"column": 13, "row": 2}, "filename": "C:/p/a.py",
   "fix": {"applicability": "unsafe", "edits": [{"content": "x is None", "end_location": {"column": 13, "row": 2}, "location": {"column": 4, "row": 2}}], "message": "Replace with `cond is None`"},
   "location": {"column": 9, "row": 2}, "message": "Comparison to `None` should be `cond is None`", "name": "none-comparison", "noqa_row": 2, "severity": "error", "url": null}
]"#;

#[test]
fn parse_and_fix() {
    let d = parse(SAMPLE).unwrap();
    // Sorted by position.
    assert_eq!(d.iter().map(|d| d.code.as_str()).collect::<Vec<_>>(), ["F401", "E711", "invalid-syntax"]);
    assert_eq!(d[0].severity, Severity::Unused);
    assert_eq!(d[1].severity, Severity::Warning);
    assert_eq!(d[2].severity, Severity::Error);
    assert_eq!((d[0].start, d[0].end), (Pos { line: 0, col: 7 }, Pos { line: 0, col: 9 }));
    assert_eq!(d[0].title(), "`os` imported but unused (F401)");
    assert_eq!(d[2].title(), "Syntax error: Expected `)`");
    assert_eq!(d[2].noqa_line, None);
    assert!(d[0].fix.as_ref().unwrap().safe && !d[1].fix.as_ref().unwrap().safe);

    let text = "import os\nif x == None: pass\n";
    assert_eq!(apply_edits(text, &d[0].fix.as_ref().unwrap().edits), "if x == None: pass\n");
    assert_eq!(apply_edits(text, &d[1].fix.as_ref().unwrap().edits), "import os\nif x is None: pass\n");
    // Character columns, not bytes.
    let e = Edit { start: Pos { line: 0, col: 5 }, end: Pos { line: 0, col: 6 }, content: "X".into() };
    assert_eq!(apply_edits("a = 'привет'", &[e]), "a = 'Xривет'");
}

#[test]
fn noqa() {
    assert_eq!(noqa_line("import os", "F401"), "import os  # noqa: F401");
    assert_eq!(noqa_line("import os  # noqa: E401", "F401"), "import os  # noqa: E401, F401");
    assert_eq!(noqa_line("import os  # noqa: F401", "F401"), "import os  # noqa: F401");
    assert_eq!(noqa_line("import os  # noqa", "F401"), "import os  # noqa");
    assert_eq!(ty_ignore_line("x: str = 1", "invalid-assignment"), "x: str = 1  # ty: ignore[invalid-assignment]");
    assert_eq!(ty_ignore_line("f(x)  # ty: ignore[a]", "b"), "f(x)  # ty: ignore[a, b]");
    assert_eq!(ty_ignore_line("f(x)  # ty: ignore[b]", "b"), "f(x)  # ty: ignore[b]");
    assert!(is_ty_code("invalid-assignment") && !is_ty_code("F401") && !is_ty_code("ARG001"));
}

/// Real ruff, when it is installed next to some Python on this machine.
#[test]
fn real_ruff() {
    let root = std::env::temp_dir().join(format!("brisk-lint-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let ruff = brisk_run::discover_all(&root).iter().find_map(|i| find_ruff(Some(&i.path))).or_else(|| find_ruff(None));
    let Some(ruff) = ruff else {
        eprintln!("ruff not found, skipping");
        return;
    };
    // Pin the rules: the machine may have a user-level ruff config.
    std::fs::write(root.join("ruff.toml"), "[lint]\nselect = [\"F\"]\n").unwrap();
    let file = root.join("app.py");
    // Unsaved text, Cyrillic before the problem.
    let text = "import os\ns = 'привет'; print(undefined_name)\n";
    let d = check_text(&ruff, &root, &file, text).unwrap();
    let codes: Vec<_> = d.iter().map(|d| d.code.as_str()).collect();
    assert_eq!(codes, ["F401", "F821"], "{d:?}");
    assert_eq!(d[1].start, Pos { line: 1, col: 20 });
    assert_eq!(d[1].file, file);
    let fixed = apply_edits(text, &d[0].fix.as_ref().unwrap().edits);
    assert_eq!(fixed, "s = 'привет'; print(undefined_name)\n");
    assert_eq!(fix_all(&ruff, &root, &file, text, Some("F401")).unwrap(), fixed);
    // A config in the project is honoured.
    std::fs::write(root.join("ruff.toml"), "[lint]\nselect = [\"F\"]\nignore = [\"F401\"]\n").unwrap();
    let d = check_text(&ruff, &root, &file, text).unwrap();
    assert_eq!(d.iter().map(|d| d.code.as_str()).collect::<Vec<_>>(), ["F821"]);
    // Project check.
    std::fs::write(&file, text).unwrap();
    std::fs::write(root.join("bad.py"), "def f(:\n  pass\n").unwrap();
    let p = check_project(&ruff, &root).unwrap();
    assert!(p.iter().any(|d| d.file == root.join("bad.py") && d.severity == Severity::Error), "{p:?}");
    assert!(p.iter().any(|d| d.file == file && d.code == "F821"), "{p:?}");
    let _ = std::fs::remove_dir_all(&root);
}
