use super::*;

#[test]
fn uris() {
    let p = if cfg!(windows) { PathBuf::from(r"C:\Users\me\мой проект\a b.py") } else { PathBuf::from("/home/me/мой проект/a b.py") };
    let u = path_to_uri(&p);
    assert!(u.starts_with("file:///") && u.contains("%20") && !u.contains(' '), "{u}");
    assert_eq!(uri_to_path(&u).unwrap(), p);
    if cfg!(windows) {
        // VS Code style lower-case, encoded drive.
        assert_eq!(uri_to_path("file:///c%3A/x/y.py").unwrap(), PathBuf::from(r"C:\x\y.py"));
    }
}

#[test]
fn parsing() {
    let sig = parse_signature(&json!({"activeParameter": 1, "activeSignature": 0, "signatures": [{"documentation": "Add.", "label": "(a: int, b: int) -> int", "parameters": [{"label": "a: int"}, {"label": "b: int"}]}]})).unwrap();
    assert_eq!(sig.params, [1..7, 9..15]);
    assert_eq!(sig.active, Some(1));
    assert_eq!(sig.doc, "Add.");
    let md = "```python\ndef add(\n    a: int\n) -> int\n```\n---\nAdd two numbers.\n";
    assert_eq!(doc_blocks(md), [DocBlock::Code("def add(\n    a: int\n) -> int".into()), DocBlock::Rule, DocBlock::Text("Add two numbers.".into())]);
    let c = parse_completion(&json!({"isIncomplete": true, "items": [{"label": "path", "kind": 9, "sortText": "1"}, {"label": "PathLike", "kind": 7, "sortText": " 0", "detail": "<class 'PathLike'>"}]}));
    assert_eq!(c.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["PathLike", "path"]);
    assert_eq!(c[0].insert, "PathLike");
    let a = parse_code_actions(&json!([{"title": "import collections.defaultdict", "isPreferred": true, "kind": "quickfix", "edit": {"changes": {path_to_uri(Path::new("/a.py")): [{"newText": "from collections import defaultdict\n", "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}}]}}}, {"title": "cmd", "command": "x"}]));
    assert_eq!(a.len(), 1);
    assert_eq!((a[0].title.as_str(), a[0].preferred, a[0].edits[0].1[0].content.as_str()), ("import collections.defaultdict", true, "from collections import defaultdict\n"));
    let d = parse_diagnostics(Path::new("a.py"), &json!({"items": [{"code": "invalid-assignment", "message": "bad", "severity": 1, "range": {"start": {"line": 2, "character": 9}, "end": {"line": 2, "character": 18}}}]}));
    assert_eq!((d[0].severity, d[0].start, d[0].title()), (Severity::Error, Pos { line: 2, col: 9 }, "bad (invalid-assignment)".to_string()));
}

/// The real ty server: set BRISK_TEST_TY to ty.exe, or have ty on PATH.
#[test]
fn real_ty() {
    let ty = std::env::var_os("BRISK_TEST_TY").map(PathBuf::from).or_else(|| find_tool("ty", None));
    let Some(ty) = ty else {
        eprintln!("ty not found, skipping");
        return;
    };
    let root = std::env::temp_dir().join(format!("brisk-lsp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    std::fs::write(root.join("pkg/__init__.py"), "").unwrap();
    let util = root.join("pkg/util.py");
    std::fs::write(&util, "def add(a: int, b: int) -> int:\n    \"\"\"Add two numbers.\"\"\"\n    return a + b\n").unwrap();
    let main = root.join("main.py");
    // Unsaved text: on disk the file is empty. Cyrillic before the call checks UTF-32 columns.
    std::fs::write(&main, "").unwrap();
    let text = "from pkg.util import add\n\nx: str = add(1, 2)\ns = 'привет'; print(add(3, 4))\n";
    let t = Ty::start(&ty, &root, None).unwrap();
    t.did_open(&main, 1, text);


    let d = futures::executor::block_on(t.diagnostics(&main)).unwrap();
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!((d[0].code.as_str(), d[0].severity, d[0].start), ("invalid-assignment", Severity::Error, Pos { line: 2, col: 9 }));

    let add_at = Pos { line: 3, col: 21 };
    let def = futures::executor::block_on(t.definition(&main, add_at)).unwrap();
    assert_eq!(def, [Location { path: util.clone(), start: Pos { line: 0, col: 4 }, end: Pos { line: 0, col: 7 } }]);
    let h = futures::executor::block_on(t.hover(&main, add_at)).unwrap().unwrap();
    assert!(h.contains("def add") && h.contains("Add two numbers."), "{h}");
    let sig = futures::executor::block_on(t.signature_help(&main, Pos { line: 3, col: 25 })).unwrap().unwrap();
    assert_eq!(sig.label, "(a: int, b: int) -> int");
    let refs = futures::executor::block_on(t.references(&main, add_at)).unwrap();
    assert_eq!(refs.len(), 4, "{refs:?}");

    // Edit: completion after "pkg.util." sees the new text.
    let text2 = format!("{text}import pkg.util\npkg.util.a");
    t.did_change(&main, 2, &text2);
    let c = futures::executor::block_on(t.completion(&main, Pos { line: 5, col: 10 })).unwrap();
    assert!(c.iter().any(|c| c.label == "add"), "{:?}", c.iter().map(|c| &c.label).collect::<Vec<_>>());

    let r = futures::executor::block_on(t.prepare_rename(&main, add_at)).unwrap();
    assert_eq!(r, Some((Pos { line: 3, col: 20 }, Pos { line: 3, col: 23 })));
    let edits = futures::executor::block_on(t.rename(&main, add_at, "plus")).unwrap();
    let files: Vec<&PathBuf> = edits.iter().map(|(p, _)| p).collect();
    assert_eq!(files.len(), 2, "{edits:?}");
    let new_main = brisk_lint::apply_edits(&text2, &edits.iter().find(|(p, _)| *p == main).unwrap().1);
    assert!(new_main.starts_with("from pkg.util import plus\n\nx: str = plus(1, 2)\ns = 'привет'; print(plus(3, 4))"), "{new_main}");
    // Alt+Enter on an unresolved name: auto-import; the diagnostic comes back from ty.
    let text3 = "import os\n\nx = defaultdict(list)\n";
    t.did_change(&main, 3, text3);
    let d = futures::executor::block_on(t.diagnostics(&main)).unwrap();
    let un: Vec<_> = d.iter().filter(|d| d.code == "unresolved-reference").cloned().collect();
    assert_eq!(un.len(), 1, "{d:?}");
    let acts = futures::executor::block_on(t.code_actions(&main, un[0].start, un[0].end, &un)).unwrap();
    let imp = acts.iter().find(|a| a.title.contains("collections")).unwrap_or_else(|| panic!("{acts:?}"));
    let fixed = brisk_lint::apply_edits(text3, &imp.edits[0].1);
    assert!(fixed.contains("from collections import defaultdict\n"), "{fixed}");
    // Auto-import completion carries the import line.
    let text4 = "import os\n\nOrderedD";
    t.did_change(&main, 4, text4);
    let c = futures::executor::block_on(t.completion(&main, Pos { line: 2, col: 8 })).unwrap();
    let od = c.iter().find(|c| c.detail == "collections").unwrap_or_else(|| panic!("{c:?}"));
    assert_eq!((od.label.as_str(), od.insert.as_str(), od.extra[0].content.as_str()), ("OrderedDict", "OrderedDict", "from collections import OrderedDict\n"));
    // Moving a module updates the imports of it.
    let text5 = "from pkg.util import add\nimport pkg.util\n";
    t.did_change(&main, 5, text5);
    let moved = futures::executor::block_on(t.will_rename_files(&[(util.clone(), root.join("pkg").join("tools.py"))])).unwrap();
    let m = moved.iter().find(|(p, _)| *p == main).unwrap_or_else(|| panic!("{moved:?}"));
    assert_eq!(brisk_lint::apply_edits(text5, &m.1), "from pkg.tools import add\nimport pkg.tools\n");
    drop(t);
    let _ = std::fs::remove_dir_all(&root);
}
