use super::*;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("brisk-run-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn links() {
    let l = find_links(r#"  File "C:\proj\app\main.py", line 12, in <module>"#);
    assert_eq!(l.len(), 1);
    assert_eq!(l[0].path, r"C:\proj\app\main.py");
    assert_eq!(l[0].line, 12);
    assert_eq!(l[0].cols, 7..37);
    assert!(find_links(r#"  File "<frozen runpy>", line 88, in _run_code"#).is_empty());
    let l = find_links("tests/test_x.py:7: AssertionError");
    assert_eq!((l[0].path.as_str(), l[0].line, l[0].cols.clone()), ("tests/test_x.py", 7, 0..17));
    let l = find_links(r"C:\a b\x.py:3:1: F401 unused");
    assert_eq!((l[0].path.as_str(), l[0].line, l[0].col), ("b\\x.py", 3, 1));
    let l = find_links("app/models.py:12:5: error: Incompatible types");
    assert_eq!((l[0].path.as_str(), l[0].line, l[0].col, l[0].cols.clone()), ("app/models.py", 12, 5, 0..18));
    assert_eq!(find_links("tests/test_x.py:7: AssertionError")[0].col, 0);
    let l = find_links(r"warn at C:\p\x.py:3: UserWarning");
    assert_eq!(l[0].path, r"C:\p\x.py");
    // Cyrillic before the link: columns are characters.
    let l = find_links("ошибка app.py:5");
    assert_eq!(l[0].cols, 7..15);
    assert!(find_links("time 12:30 ok").is_empty());
}

/// "Open in Terminal" lines quote everything and leave PATH to the terminal.
#[test]
fn shell_line_for_the_terminal() {
    let cmd = Command {
        program: PathBuf::from(r"C:\venv\python.exe"),
        args: vec!["app.py".into(), "a b".into(), "it's".into()],
        cwd: PathBuf::from(r"C:\p r"),
        env: [("PATH".to_string(), "x".to_string()), ("B".into(), "2".into()), ("A".into(), "o'k".into())].into_iter().collect(),
    };
    assert_eq!(cmd.shell_line(true), r"Set-Location -LiteralPath 'C:\p r'; ${env:A}='o''k'; ${env:B}='2'; & 'C:\venv\python.exe' 'app.py' 'a b' 'it''s'");
    assert_eq!(cmd.shell_line(false), r"cd 'C:\p r' && A='o'\''k' B='2' 'C:\venv\python.exe' 'app.py' 'a b' 'it'\''s'");
    // Names `$env:X` cannot take.
    let cmd = Command {
        env: [("MY-VAR".to_string(), "1".to_string()), ("ProgramFiles(x86)".into(), "2".into()), ("a}b".into(), "3".into())].into_iter().collect(),
        ..cmd
    };
    assert_eq!(
        cmd.shell_line(true),
        r"Set-Location -LiteralPath 'C:\p r'; ${env:MY-VAR}='1'; ${env:ProgramFiles(x86)}='2'; ${env:a`}b}='3'; & 'C:\venv\python.exe' 'app.py' 'a b' 'it''s'"
    );
}

/// A cmd.exe line reaches cmd unchanged: inner quotes are not MSVC-escaped to `\"`.
#[test]
fn cmd_line_is_passed_raw() {
    let cmd = shell_command(Path::new("."), &RunConfig::default(), Path::new("python"), r#"echo "a b" & echo x"#);
    if cfg!(windows) {
        assert_eq!(cmd.cmd_line(), Some(r#"echo "a b" & echo x"#));
        let out = Command { cwd: std::env::temp_dir(), ..cmd }.to_std().output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert_eq!(text.lines().map(str::trim).collect::<Vec<_>>(), vec![r#""a b""#, "x"]);
    } else {
        assert_eq!(cmd.cmd_line(), None);
    }
}

#[test]
fn args() {
    assert_eq!(split_args(r#"-v --name "a b" x\"y "" end"#), ["-v", "--name", "a b", "x\"y", "", "end"]);
    assert!(split_args("   ").is_empty());
}

#[test]
fn idea_import_and_temporaries() {
    let root = tmp("idea");
    std::fs::create_dir_all(root.join(".idea/runConfigurations")).unwrap();
    std::fs::write(
        root.join(".idea/runConfigurations/serve.xml"),
        r#"<component name="ProjectRunConfigurationManager">
  <configuration default="false" name="serve" type="PythonConfigurationType" factoryName="Python">
    <module name="demo" />
    <option name="INTERPRETER_OPTIONS" value="-X dev" />
    <envs>
      <env name="PYTHONUNBUFFERED" value="1" />
      <env name="APP_ENV" value="local" />
    </envs>
    <option name="SDK_HOME" value="" />
    <option name="WORKING_DIRECTORY" value="$PROJECT_DIR$" />
    <option name="IS_MODULE_SDK" value="true" />
    <option name="SCRIPT_NAME" value="uvicorn" />
    <option name="PARAMETERS" value="app:app --reload" />
    <option name="MODULE_MODE" value="true" />
    <option name="MODULE_NAME" value="uvicorn" />
  </configuration>
</component>"#,
    )
    .unwrap();
    std::fs::write(
        root.join(".idea/workspace.xml"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<project version="4">
  <component name="RunManager" selected="Python.main">
    <configuration name="main" type="PythonConfigurationType" factoryName="Python" temporary="true" nameIsGenerated="true">
      <option name="SDK_HOME" value="C:\Python312\python.exe" />
      <option name="IS_MODULE_SDK" value="false" />
      <option name="WORKING_DIRECTORY" value="$PROJECT_DIR$/app" />
      <option name="SCRIPT_NAME" value="$PROJECT_DIR$/app/main.py" />
      <option name="PARAMETERS" value="--port 8000" />
    </configuration>
    <configuration name="pytest in tests" type="tests" factoryName="py.test" temporary="true">
      <option name="WORKING_DIRECTORY" value="$PROJECT_DIR$" />
      <option name="_new_keywords" value="&quot;not slow&quot;" />
      <option name="_new_parameters" value="&quot;-x&quot;" />
      <option name="_new_additionalArguments" value="&quot;&quot;" />
      <option name="_new_target" value="&quot;$PROJECT_DIR$/tests&quot;" />
      <option name="_new_targetType" value="&quot;PATH&quot;" />
    </configuration>
    <configuration name="pytest for test_m.TestA.test_b" type="tests" factoryName="py.test">
      <option name="_new_target" value="&quot;app.test_m.TestA.test_b&quot;" />
      <option name="_new_targetType" value="&quot;PYTHON&quot;" />
    </configuration>
    <configuration name="unit" type="tests" factoryName="Unittests" />
    <configuration default="true" type="PythonConfigurationType" factoryName="Python" />
  </component>
</project>"#,
    )
    .unwrap();
    std::fs::create_dir_all(root.join("app")).unwrap();
    std::fs::write(root.join("app/test_m.py"), "").unwrap();
    let (mut f, n) = load(&root);
    assert_eq!(n, 5); // the Unittests configuration is imported too
    let t = f.get("pytest in tests").unwrap();
    assert_eq!((t.kind, t.target.as_str(), t.keywords.as_str(), t.parameters.as_str()), (RunKind::Pytest, "tests", "not slow", "-x"));
    let cmd = build(&root, t, Path::new("python"));
    assert_eq!(cmd.args, ["-m", "pytest", "tests", "-k", "not slow", "-x"]);
    assert_eq!(cmd.cwd, root);
    assert_eq!(f.get("pytest for test_m.TestA.test_b").unwrap().target, "app/test_m.py::TestA::test_b");
    f.configurations.retain(|c| c.kind == RunKind::Python);
    let serve = f.get("serve").unwrap();
    assert!(serve.module);
    assert_eq!(serve.target, "uvicorn");
    assert_eq!(serve.interpreter_options, "-X dev");
    assert_eq!(serve.env.get("APP_ENV").map(String::as_str), Some("local"));
    assert!(!serve.env.contains_key("PYTHONUNBUFFERED"));
    let main = f.get("main").unwrap();
    assert!(main.temporary);
    assert_eq!(main.target, "app/main.py");
    assert_eq!(main.interpreter.as_deref(), Some(r"C:\Python312\python.exe"));
    assert_eq!(f.selected.as_deref(), Some("main"));

    // Module command: python -X dev -m uvicorn app:app --reload, cwd = project.
    let cmd = build(&root, serve, Path::new("python"));
    assert_eq!(cmd.args, ["-X", "dev", "-m", "uvicorn", "app:app", "--reload"]);
    assert_eq!(cmd.cwd, root);
    assert_eq!(cmd.env.get("APP_ENV").map(String::as_str), Some("local"));
    assert_eq!(cmd.env.get("PYTHONUNBUFFERED").map(String::as_str), Some("1"));

    // Same target reuses the configuration; new files make temporaries, capped at five.
    assert_eq!(f.use_for_file(RunConfig::for_file(&root, &root.join("app/main.py"))), "main");
    for i in 0..7 {
        f.use_for_file(RunConfig::for_file(&root, &root.join(format!("s{i}.py"))));
    }
    assert_eq!(f.configurations.iter().filter(|c| c.temporary).count(), MAX_TEMPORARY);
    assert_eq!(f.selected.as_deref(), Some("s6"));
    assert!(f.get("serve").is_some());
    save(&root, &f).unwrap();
    assert_eq!(load(&root), (f, 0));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn venv_detection_and_labels() {
    let root = tmp("venv");
    let bin = root.join(".venv").join(if cfg!(windows) { "Scripts" } else { "bin" });
    std::fs::create_dir_all(&bin).unwrap();
    let exe = bin.join(if cfg!(windows) { "python.exe" } else { "python" });
    std::fs::write(&exe, b"").unwrap();
    std::fs::write(root.join(".venv/pyvenv.cfg"), "home = C:\\Python312\nimplementation = CPython\nversion_info = 3.12.4.final.0\n").unwrap();
    let v = project_venvs(&root);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].version, "3.12.4");
    let name = root.file_name().unwrap().to_string_lossy();
    assert_eq!(v[0].label(&root), format!("Python 3.12 ({name})"));
    // Script config: cwd is the script's folder, venv goes first on PATH.
    let cfg = RunConfig::for_file(&root, &root.join("app/main.py"));
    assert_eq!(cfg.target, "app/main.py");
    let cmd = build(&root, &cfg, &v[0].path);
    assert_eq!(cmd.cwd, root.join("app"));
    assert!(cmd.env["PATH"].starts_with(&bin.display().to_string()));
    assert_eq!(cmd.env["VIRTUAL_ENV"], root.join(".venv").display().to_string());
    let sys = Interpreter { path: "/usr/bin/python3".into(), version: "3.11.2".into(), kind: InterpKind::System };
    assert_eq!(sys.label(&root), "Python 3.11");
    let _ = std::fs::remove_dir_all(&root);
}

/// End to end through a real PTY: output with Cyrillic, stdin for input(), exit code,
/// traceback links. Uses the first interpreter found on this machine.
#[test]
fn run_real_python() {
    let root = tmp("real");
    let Some(py) = discover_all(&root).into_iter().next() else {
        eprintln!("no python found, skipping");
        return;
    };
    let script = root.join("my script.py");
    std::fs::write(&script, "import sys\nname = input('name? ')\nprint('привет', name)\nsys.stdout.flush()\ndef f():\n    raise ValueError('boom')\nf()\n")
        .unwrap();
    let cmd = build(&root, &RunConfig::for_file(&root, &script), &py.path);
    assert!(cmd.display().contains("\"") && cmd.display().contains("my script.py"));
    let (tx, rx) = mpsc::channel();
    let s = brisk_terminal::Session::spawn_process(cmd.to_std(), &cmd.display(), 100, 30, move |e| {
        let _ = tx.send(e);
    })
    .expect("spawn python");
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut answered = false;
    let mut code = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(brisk_terminal::TermEvent::ChildExit(c)) => code = c,
            Ok(brisk_terminal::TermEvent::Exit) => break,
            _ => {}
        }
        if !answered && s.text().contains("name?") {
            s.write(b"Bob\r".to_vec());
            answered = true;
        }
        assert!(Instant::now() < deadline, "timeout; screen:\n{}", s.text());
    }
    s.inject(b"\r\n\x1b[0mProcess finished with exit code 1\r\n");
    let text = s.text();
    assert!(text.contains("привет Bob"), "{text}");
    assert!(text.starts_with(&py.path.display().to_string()), "{text}");
    assert!(
        text.contains(
            "name? Bob
"
        ),
        "{text}"
    );
    assert!(text.contains("ValueError: boom"), "{text}");
    assert!(text.contains("Process finished with exit code 1"), "{text}");
    assert_eq!(code, Some(1));
    let frame = text.lines().find(|l| l.contains("my script.py") && l.contains("line 7")).expect("traceback frame");
    let link = &find_links(frame)[0];
    assert_eq!((Path::new(&link.path), link.line), (script.as_path(), 7));
    let _ = std::fs::remove_dir_all(&root);
}

/// `print()` writes its pieces separately when unbuffered; a stderr line must not land in
/// the middle of a stdout line.
#[test]
fn stdout_stderr_lines_whole() {
    let root = tmp("order");
    let Some(py) = discover_all(&root).into_iter().next() else { return };
    let script = root.join("order.py");
    // The short pause keeps the program slower than the readers, as a program in the Run
    // console usually is. Without it all 60 lines sit in the two pipes before either reader
    // wakes, and their order is up to the scheduler (17 of 30 swapped on a Linux runner).
    std::fs::write(
        &script,
        "import sys, time\nfor i in range(30):\n    print('out', i, 'x', 'y')\n    print('err', i, file=sys.stderr)\n    time.sleep(0.005)\n",
    )
    .unwrap();
    let cmd = build(&root, &RunConfig::for_file(&root, &script), &py.path);
    let (tx, rx) = mpsc::channel();
    let s = brisk_terminal::Session::spawn_process(cmd.to_std(), &cmd.display(), 100, 30, move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    while !matches!(rx.recv_timeout(Duration::from_secs(20)), Ok(brisk_terminal::TermEvent::Exit) | Err(_)) {}
    let text = s.text();
    let lines: Vec<&str> = text.lines().skip(1).collect();
    // Every line arrives whole (no "out 3 xerr 3" + " y"). Two pipes carry no global order,
    // so a stderr line may rarely overtake the stdout line before it (PyCharm does the same).
    let mut swapped = 0;
    for i in 0..30 {
        let o = lines.iter().position(|l| *l == format!("out {i} x y")).unwrap_or_else(|| panic!("broken out {i}:\n{text}"));
        let e = lines.iter().position(|l| *l == format!("err {i}")).unwrap_or_else(|| panic!("broken err {i}:\n{text}"));
        swapped += usize::from(o > e);
    }
    assert!(swapped <= 5, "{swapped} of 30 out of order:\n{text}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Stop (Ctrl+F2) kills the process tree, including children it started.
#[test]
fn stop_kills_tree() {
    let root = tmp("stop");
    let Some(py) = discover_all(&root).into_iter().next() else { return };
    let script = root.join("sleepy.py");
    std::fs::write(
        &script,
        "import subprocess, sys, time\nsubprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])\nprint('started', flush=True)\ntime.sleep(60)\n",
    )
    .unwrap();
    let cmd = build(&root, &RunConfig::for_file(&root, &script), &py.path);
    let (tx, rx) = mpsc::channel();
    let s = brisk_terminal::Session::spawn_process(cmd.to_std(), &cmd.display(), 100, 30, move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !s.text().contains("started") {
        assert!(Instant::now() < deadline, "{}", s.text());
        std::thread::sleep(Duration::from_millis(50));
    }
    let t0 = Instant::now();
    s.kill();
    loop {
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(brisk_terminal::TermEvent::Exit) => break,
            Ok(_) => {}
            Err(_) => panic!("no exit after stop"),
        }
    }
    // The grandchild held the pipes too: they close only if it was killed as well.
    assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn pytest_tree_from_events() {
    assert_eq!(pytest::split_nodeid("t/test_a.py::TestA::test_b[x::y]"), ["t/test_a.py", "TestA", "test_b[x::y]"]);
    let mut t = TestTree::new();
    let lines = [
        r#"{"event":"session","rootdir":"/p"}"#,
        r#"{"event":"collected","tests":[{"id":"t/test_a.py::test_ok","file":"/p/t/test_a.py","line":3},{"id":"t/test_a.py::TestA::test_b","file":"/p/t/test_a.py","line":8},{"id":"t/test_a.py::test_skip","file":"/p/t/test_a.py","line":12}]}"#,
        r#"{"event":"start","id":"t/test_a.py::test_ok"}"#,
    ];
    for l in lines {
        assert!(t.apply(l));
    }
    assert!(!t.apply("garbage"));
    assert_eq!(t.nodes[0].status, TestStatus::Running);
    assert_eq!(t.summary(false), "Running: passed: 0 of 3 tests – 0 ms");
    t.apply(r#"{"event":"finish","id":"t/test_a.py::test_ok","outcome":"passed","duration":0.01,"message":"","output":""}"#);
    t.apply(r#"{"event":"finish","id":"t/test_a.py::TestA::test_b","outcome":"failed","duration":0.2,"message":"assert 1 == 2","output":"----- Captured stdout call -----\nhi\n"}"#);
    t.apply(r#"{"event":"finish","id":"t/test_a.py::test_skip","outcome":"skipped","duration":0.0,"message":"no db","output":""}"#);
    t.apply(r#"{"event":"done","exitstatus":1}"#);
    // Root → module → class → test.
    let names: Vec<_> = t.rows(&Default::default()).iter().map(|&i| (t.nodes[i].depth, t.nodes[i].name.clone())).collect();
    assert_eq!(
        names,
        [(0, "Test Results".into()), (1, "test_a".into()), (2, "test_ok".into()), (2, "TestA".into()), (3, "test_b".into()), (2, "test_skip".into())]
    );
    assert_eq!(t.nodes[0].status, TestStatus::Failed);
    assert_eq!(t.summary(true), "Tests failed: 1, passed: 1, ignored: 1 of 3 tests – 210 ms");
    assert_eq!(t.failed_targets(), [format!("{}::TestA::test_b", Path::new("/p/t/test_a.py").display())]);
    let f = t.first_failed().unwrap();
    assert_eq!((t.nodes[f].name.as_str(), t.nodes[f].line), ("test_b", Some(8)));
    assert!(t.details(f).starts_with("----- Captured stdout call -----\nhi\n") && t.details(f).ends_with("assert 1 == 2\n"));

    let mut ok = TestTree::new();
    ok.apply(r#"{"event":"collected","tests":[{"id":"a.py::t","file":"","line":1}]}"#);
    ok.apply(r#"{"event":"finish","id":"a.py::t","outcome":"passed","duration":1.5,"message":"","output":""}"#);
    ok.apply(r#"{"event":"done","exitstatus":0}"#);
    assert_eq!(ok.summary(true), "Tests passed: 1 of 1 test – 1 sec 500 ms");

    let root = Path::new("/p");
    let c = RunConfig::for_tests(root, &root.join("t/test_a.py"), &["TestA".into(), "test_b".into()]);
    assert_eq!((c.name.as_str(), c.target.as_str()), ("pytest for test_a.TestA.test_b", "t/test_a.py::TestA::test_b"));
    let cmd = build(root, &c, Path::new("python"));
    assert_eq!(cmd.args, ["-m", "pytest", "test_a.py::TestA::test_b"]);
    assert_eq!(cmd.cwd, root.join("t"));
    assert_eq!(RunConfig::for_tests(root, &root.join("t"), &[]).name, "pytest in t");
    assert!(is_test_file(Path::new("x/test_a.py")) && is_test_file(Path::new("a_test.py")) && !is_test_file(Path::new("testing.py")));
}

/// Parameter sets under their function, the Show Passed / Show Ignored / by-duration view,
/// HTML export and the Test History.
#[test]
fn pytest_tree_view_params_export_history() {
    let mut t = TestTree::new();
    t.apply(r#"{"event":"collected","tests":[{"id":"a.py::test_p[1-x]","file":"/p/a.py","line":4},{"id":"a.py::test_p[2-y]","file":"/p/a.py","line":4},{"id":"a.py::C::test_q[a::b]","file":"/p/a.py","line":9},{"id":"a.py::test_s","file":"/p/a.py","line":12}]}"#);
    let fin = |id: &str, outcome: &str, d: f64| {
        format!(
            r#"{{"event":"finish","id":"{id}","outcome":"{outcome}","duration":{d},"message":"{}","output":""}}"#,
            if outcome == "failed" { "assert <1>" } else { "" }
        )
    };
    for l in [
        fin("a.py::test_p[1-x]", "passed", 0.1),
        fin("a.py::test_p[2-y]", "failed", 0.3),
        fin("a.py::C::test_q[a::b]", "passed", 0.5),
        fin("a.py::test_s", "skipped", 0.0),
    ] {
        t.apply(&l);
    }
    t.apply(r#"{"event":"done","exitstatus":1}"#);
    let names = |rows: Vec<usize>| rows.iter().map(|&i| format!("{}{}", " ".repeat(t.nodes[i].depth), t.nodes[i].name)).collect::<Vec<_>>();
    assert_eq!(names(t.rows(&Default::default())), ["Test Results", " a", "  test_p", "   [1-x]", "   [2-y]", "  C", "   test_q", "    [a::b]", "  test_s"]);
    let g = t.nodes.iter().position(|n| n.name == "test_p").unwrap();
    assert_eq!((t.nodes[g].id.as_str(), t.nodes[g].is_test, t.nodes[g].status, t.nodes[g].line), ("a.py::test_p", false, TestStatus::Failed, Some(4)));
    assert_eq!(t.counts().total, 4);
    let v = |p, i, d| TreeView { show_passed: p, show_ignored: i, by_duration: d };
    assert_eq!(names(t.rows_view(&Default::default(), v(false, true, false))), ["Test Results", " a", "  test_p", "   [2-y]", "  test_s"]);
    assert_eq!(names(t.rows_view(&Default::default(), v(false, false, false))), ["Test Results", " a", "  test_p", "   [2-y]"]);
    assert_eq!(
        names(t.rows_view(&Default::default(), v(true, true, true))),
        ["Test Results", " a", "  C", "   test_q", "    [a::b]", "  test_p", "   [2-y]", "   [1-x]", "  test_s"]
    );
    assert_eq!(t.failed_ids(), ["a.py::test_p[2-y]"]);

    let html = t.to_html("pytest <a>");
    assert!(html.contains("<title>pytest &lt;a&gt;</title>") && html.contains("Tests failed: 1, passed: 2, ignored: 1 of 4 tests"), "{html}");
    assert!(html.contains("<span class=\"failed\">✗ [2-y]</span>") && html.contains("<pre>assert &lt;1&gt;</pre>"), "{html}");

    let root = tmp("test-history");
    let collected = r#"{"event":"collected","tests":[{"id":"a.py::t1","file":"","line":1},{"id":"a.py::t2","file":"","line":2}]}"#;
    let events = format!("{collected}\n{}\n", fin("a.py::t2", "failed", 0.1));
    for k in 0..12 {
        pytest::history::save(&root, if k == 11 { "other" } else { "tests" }, &events).unwrap();
    }
    let list = pytest::history::list(&root);
    assert_eq!(list.len(), 10);
    assert_eq!(list[0].name, "other");
    let back = pytest::history::load(&list[1]).unwrap();
    assert_eq!(back.counts().total, 2);
    assert_eq!(pytest::history::last_failed(&root, "tests"), ["a.py::t2"]);
    assert!(pytest::history::last_failed(&root, "none").is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

/// A real pytest session through the plugin: live tree, captured output, Rerun Failed.
#[test]
fn pytest_real_run() {
    let root = tmp("pytest");
    let Some(py) = discover_all(&root).into_iter().next() else { return };
    let ok = std::process::Command::new(&py.path).args(["-c", "import pytest"]).status().is_ok_and(|s| s.success());
    if !ok {
        eprintln!("pytest not installed, skipping");
        return;
    }
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(root.join("calc.py"), "def add(a, b):\n    return a + b\n").unwrap();
    std::fs::write(
        root.join("tests/test_calc.py"),
        "import pytest\nfrom calc import add\n\ndef test_add():\n    print('adding')\n    assert add(2, 2) == 4\n\nclass TestBad:\n    def test_wrong(self):\n        print('привет')\n        assert add(2, 2) == 5\n\n@pytest.mark.parametrize('n', [1, 2])\ndef test_param(n):\n    assert n > 0\n\n@pytest.mark.skip(reason='later')\ndef test_skip():\n    pass\n",
    )
    .unwrap();
    let run = |cfg: &RunConfig| {
        let mut cmd = build(&root, cfg, &py.path);
        let events = pytest::attach(&mut cmd).unwrap();
        let (tx, rx) = mpsc::channel();
        let s = brisk_terminal::Session::spawn_process(cmd.to_std(), &cmd.display(), 120, 30, move |e| {
            let _ = tx.send(e);
        })
        .unwrap();
        let mut feed = TestFeed::new(events);
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            feed.poll();
            if let Ok(brisk_terminal::TermEvent::Exit) = rx.recv_timeout(Duration::from_millis(50)) {
                break;
            }
            assert!(Instant::now() < deadline, "{}", s.text());
        }
        feed.poll();
        (feed, s.text(), cmd)
    };
    let cfg = RunConfig::for_tests(&root, &root.join("tests"), &[]);
    let (feed, text, cmd) = run(&cfg);
    let t = &feed.tree;
    assert_eq!(cmd.cwd, root.join("tests"));
    assert!(text.contains("1 failed, 3 passed, 1 skipped"), "{text}");
    assert_eq!(t.done, Some(1));
    assert_eq!(t.summary(true).split(" – ").next().unwrap(), "Tests failed: 1, passed: 3, ignored: 1 of 5 tests", "{text}");
    let f = t.first_failed().expect("failed test");
    assert_eq!((t.nodes[f].name.as_str(), t.nodes[f].line), ("test_wrong", Some(9)));
    let d = t.details(f);
    assert!(d.contains("привет") && d.contains("assert 4 == 5"), "{d}");
    // The failure points at the source line, as a link.
    assert!(d.lines().flat_map(find_links).any(|l| l.path.ends_with("test_calc.py") && l.line == 11), "{d}");
    let names: Vec<String> = t.rows(&Default::default()).iter().map(|&i| t.nodes[i].name.clone()).collect();
    assert!(names.contains(&"test_param".to_string()) && names.contains(&"[1]".to_string()) && names.contains(&"TestBad".to_string()), "{names:?}");

    // Rerun Failed Tests: only the failed node runs, from the same folder.
    let failed = t.failed_targets();
    assert_eq!(failed.len(), 1);
    let again = RunConfig {
        target: failed.iter().map(|f| format!("\"{f}\"")).collect::<Vec<_>>().join(" "),
        working_directory: relativize(&root, &cmd.cwd),
        ..cfg.clone()
    };
    let (feed2, text2, cmd2) = run(&again);
    assert_eq!(cmd2.args[2..], ["test_calc.py::TestBad::test_wrong"]);
    assert_eq!(feed2.tree.counts().total, 1, "{text2}");
    assert_eq!(feed2.tree.counts().failed, 1);
    // Rerun Failed Tests First: the plugin runs the listed node ids before the rest.
    let first = root.join("first.txt");
    std::fs::write(&first, "test_calc.py::test_param[2]\ntest_calc.py::TestBad::test_wrong\n").unwrap();
    let mut cmd3 = build(&root, &RunConfig { working_directory: String::new(), ..cfg.clone() }, &py.path);
    let events3 = pytest::attach(&mut cmd3).unwrap();
    cmd3.env.insert("BRISK_TESTS_FIRST".into(), first.display().to_string());
    let out = cmd3.to_std().output().unwrap();
    let order: Vec<String> = std::fs::read_to_string(&events3)
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["event"] == "start")
        .map(|v| v["id"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(order[..2], ["test_calc.py::TestBad::test_wrong", "test_calc.py::test_param[2]"], "{}", String::from_utf8_lossy(&out.stdout));
    let _ = std::fs::remove_file(&events3);
    drop(feed);
    drop(feed2);
    let _ = std::fs::remove_dir_all(&root);
}

/// unittest and doctest through Brisk's runner: the commands stay `-m unittest` /
/// `-m doctest`, the tree is the pytest one, Rerun Failed narrows to dotted names.
#[test]
fn unittest_and_doctest_real_runs() {
    let root = tmp("unittest");
    let Some(py) = discover_all(&root).into_iter().next() else { return };
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(root.join("tests/__init__.py"), "").unwrap();
    std::fs::write(
        root.join("tests/test_u.py"),
        "import unittest\n\nclass TestA(unittest.TestCase):\n    def test_ok(self):\n        print('hi')\n\n    def test_bad(self):\n        self.assertEqual(1, 2)\n\n    @unittest.skip('later')\n    def test_skip(self):\n        pass\n\n    def test_sub(self):\n        for i in range(3):\n            with self.subTest(i=i):\n                self.assertNotEqual(i, 1)\n",
    )
    .unwrap();
    std::fs::write(root.join("tests/test_broken.py"), "import nope_missing_module\n").unwrap();
    std::fs::write(root.join("mathx.py"), "def double(x):\n    \"\"\"\n    >>> double(2)\n    4\n    >>> double(3)\n    7\n    \"\"\"\n    return x * 2\n\ndef half(x):\n    \"\"\"\n    >>> half(4)\n    2.0\n    \"\"\"\n    return x / 2\n").unwrap();
    let run = |cfg: &RunConfig| {
        let cmd = build(&root, cfg, &py.path);
        let mut spawn = cmd.clone();
        let events = pytest::attach_runner(&mut spawn, cfg.kind).unwrap();
        let out = spawn.to_std().output().unwrap();
        let mut feed = TestFeed::new(events);
        feed.poll();
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        (feed, cmd, spawn, text)
    };
    // The folder: discovery, a broken module, a failure, a skip and a failed subtest.
    let cfg = RunConfig { name: "u".into(), kind: RunKind::Unittest, target: "tests".into(), ..Default::default() };
    let (feed, cmd, spawn, text) = run(&cfg);
    assert_eq!(cmd.args, ["-m", "unittest", "discover"], "{text}");
    assert_eq!(spawn.args[..2], ["-m", "brisk_unittest"]);
    assert_eq!(cmd.cwd, root.join("tests"));
    let t = &feed.tree;
    let names: Vec<String> = t.rows(&Default::default()).iter().map(|&i| format!("{}{}", " ".repeat(t.nodes[i].depth), t.nodes[i].name)).collect();
    assert!(names.contains(&"  TestA".to_string()) && names.contains(&"   test_ok".to_string()), "{names:?}\n{text}");
    let c = t.counts();
    // The broken module counts as a failure, as in the pytest tree.
    assert_eq!((c.passed, c.failed, c.skipped), (1, 3, 1), "{names:?}\n{text}");
    assert!(t.nodes.iter().any(|n| n.name == "test_broken" && n.status == TestStatus::Error && n.message.contains("nope_missing_module")), "{names:?}\n{text}");
    let bad = t.nodes.iter().find(|n| n.name == "test_bad").unwrap();
    assert_eq!((bad.status, bad.line), (TestStatus::Failed, Some(7)));
    assert!(bad.message.contains("AssertionError: 1 != 2"), "{}", bad.message);
    assert!(t.nodes.iter().any(|n| n.name == "test_sub" && n.status == TestStatus::Failed && n.message.contains("(i=1)")));
    assert!(text.contains("FAILED"), "{text}");
    assert_eq!(t.done, Some(1));

    // Rerun Failed: dotted names relative to the same folder.
    let failed = t.failed_targets();
    let again = RunConfig {
        target: failed.iter().filter(|f| f.contains("TestA")).map(|f| format!("\"{f}\"")).collect::<Vec<_>>().join(" "),
        working_directory: relativize(&root, &cmd.cwd),
        ..cfg.clone()
    };
    let (feed2, cmd2, _, text2) = run(&again);
    let mut tail = cmd2.args[2..].to_vec();
    tail.sort();
    assert_eq!(tail, ["test_u.TestA.test_bad", "test_u.TestA.test_sub"]);
    assert_eq!(feed2.tree.counts().total, 2, "{text2}");

    // Doctest: the file's examples, one failing.
    let d = RunConfig { name: "d".into(), kind: RunKind::Doctest, target: "mathx.py".into(), ..Default::default() };
    let (feed3, cmd3, spawn3, text3) = run(&d);
    assert_eq!(cmd3.args, ["-m", "doctest", "mathx.py"]);
    assert_eq!(spawn3.args[..3], ["-m", "brisk_unittest", "--doctest"]);
    let t3 = &feed3.tree;
    let c3 = t3.counts();
    assert_eq!((c3.total, c3.passed, c3.failed), (2, 1, 1), "{text3}");
    let dbl = t3.nodes.iter().find(|n| n.name == "mathx.double").expect("doctest node");
    assert!(dbl.message.contains("Expected:") && dbl.message.contains("7"), "{}", dbl.message);
    assert_eq!(dbl.line, Some(2));
    // One doctest by name.
    let (feed4, _, _, text4) = run(&RunConfig { target: "mathx.py::mathx.half".into(), ..d.clone() });
    assert_eq!((feed4.tree.counts().total, feed4.tree.counts().passed), (1, 1), "{text4}");

    // Context configurations and PyCharm's settings.
    let u = RunConfig::for_tests_with(&root, &root.join("tests/test_u.py"), &["TestA".into(), "test_ok".into()], RunKind::Unittest);
    assert_eq!((u.name.as_str(), u.target.as_str()), ("Unittests for test_u.TestA.test_ok", "tests/test_u.py::TestA::test_ok"));
    assert_eq!(build(&root, &u, &py.path).args, ["-m", "unittest", "test_u.TestA.test_ok"]);
    std::fs::create_dir_all(root.join(".idea")).unwrap();
    std::fs::write(
        root.join(".idea/p.iml"),
        r#"<module><component name="TestRunnerService"><option name="PROJECT_TEST_RUNNER" value="Unittests" /></component></module>"#,
    )
    .unwrap();
    assert_eq!(crate::idea_test_runner(&root), Some(crate::TestRunner::Unittest));
    let xml = r#"<project><component name="RunManager"><configuration name="ut" type="tests" factoryName="Unittests"><option name="_new_additionalArguments" value="&quot;-v&quot;" /><option name="_new_target" value="&quot;$PROJECT_DIR$/tests&quot;" /><option name="_new_targetType" value="&quot;PATH&quot;" /></configuration><configuration name="dt" type="tests" factoryName="Doctests"><option name="_new_target" value="&quot;$PROJECT_DIR$/mathx.py&quot;" /><option name="_new_targetType" value="&quot;PATH&quot;" /></configuration></component></project>"#;
    std::fs::write(root.join(".idea/workspace.xml"), xml).unwrap();
    let f = crate::import_idea(&root);
    let got: Vec<_> = f.configurations.iter().map(|c| (c.name.as_str(), c.kind, c.target.as_str(), c.parameters.as_str())).collect();
    assert_eq!(got, [("ut", RunKind::Unittest, "tests", "-v"), ("dt", RunKind::Doctest, "mathx.py", "")]);
    assert_eq!(f.test_runner, Some(crate::TestRunner::Unittest));
    let json = serde_json::to_string(&f).unwrap();
    assert!(json.contains("\"testRunner\":\"unittest\"") && json.contains("\"type\":\"unittest\""), "{json}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn compound_members_expand_nested_and_skip_cycles() {
    let c = |name: &str, kind: RunKind, target: &str| RunConfig { name: name.into(), kind, target: target.into(), ..Default::default() };
    let f = RunFile {
        configurations: vec![
            c("server", RunKind::Python, "server.py"),
            c("client", RunKind::Python, "client.py"),
            c("tests", RunKind::Pytest, ""),
            c("both", RunKind::Compound, "server; client"),
            c("all", RunKind::Compound, "both;tests;server;all"),
            c("broken", RunKind::Compound, "server;nope"),
        ],
        ..Default::default()
    };
    let names = |n: &str| f.compound_members(f.get(n).unwrap()).map(|v| v.into_iter().map(|c| c.name).collect::<Vec<_>>());
    assert_eq!(names("both").unwrap(), vec!["server", "client"]);
    assert_eq!(names("all").unwrap(), vec!["server", "client", "tests"]);
    assert!(names("broken").unwrap_err().contains("'nope'"));
    let json = serde_json::to_string(f.get("both").unwrap()).unwrap();
    assert!(json.contains("\"type\":\"compound\""), "{json}");
}

#[test]
fn imports_pycharm_compound() {
    let xml = r#"<project><component name="RunManager">
  <configuration name="Both" type="CompoundRunConfigurationType">
    <toRun name="server" type="PythonConfigurationType" />
    <toRun name="client" type="PythonConfigurationType" />
    <method v="2" />
  </configuration>
</component></project>"#;
    let (cfgs, _) = super::config::configurations_in(Path::new("/p"), xml, "RunManager");
    assert_eq!(cfgs.len(), 1);
    assert_eq!((cfgs[0].kind, cfgs[0].target.as_str()), (RunKind::Compound, "server;client"));
}

#[test]
fn before_launch_steps_and_import() {
    let c = |name: &str, kind: RunKind, target: &str| RunConfig { name: name.into(), kind, target: target.into(), ..Default::default() };
    let mut app = c("app", RunKind::Python, "app.py");
    app.before_run = "migrate; tests".into();
    app.before_command = " pip install -e . ".into();
    let f = RunFile {
        configurations: vec![
            c("migrate", RunKind::Python, "migrate.py"),
            c("tests", RunKind::Pytest, ""),
            c("both", RunKind::Compound, "migrate;tests"),
            app.clone(),
        ],
        ..Default::default()
    };
    let steps = f.before_steps(&app).unwrap();
    let shown: Vec<String> = steps
        .iter()
        .map(|s| match s {
            BeforeStep::Run(c) => c.name.clone(),
            BeforeStep::Command(l) => format!("$ {l}"),
        })
        .collect();
    assert_eq!(shown, vec!["migrate", "tests", "$ pip install -e ."]);
    for (bad, why) in [("nope", "does not exist"), ("app", "itself"), ("both", "Compound")] {
        let cfg = RunConfig { before_run: bad.into(), before_command: String::new(), ..app.clone() };
        assert!(f.before_steps(&cfg).unwrap_err().contains(why), "{bad}");
    }
    let cmd = shell_command(Path::new("/p"), &app, Path::new("/usr/bin/python3"), "pip install -e .");
    assert_eq!(cmd.args.last().map(String::as_str), Some("pip install -e ."));
    assert_eq!(cmd.env.get("PYTHONUNBUFFERED").map(String::as_str), Some("1"));
    let xml = r#"<project><component name="RunManager">
  <configuration name="app" type="PythonConfigurationType" factoryName="Python">
    <option name="SCRIPT_NAME" value="$PROJECT_DIR$/app.py" />
    <method v="2">
      <option name="RunConfigurationTask" enabled="true" run_configuration_name="migrate" run_configuration_type="PythonConfigurationType" />
      <option name="RunConfigurationTask" enabled="false" run_configuration_name="off" run_configuration_type="PythonConfigurationType" />
    </method>
  </configuration>
</component></project>"#;
    let (cfgs, _) = super::config::configurations_in(Path::new("/p"), xml, "RunManager");
    assert_eq!(cfgs[0].before_run, "migrate");
}

/// PyCharm's FastAPI / Flask Server / Django Server / Django tests configurations, shaped like
/// real `.run/*.run.xml` files and `workspace.xml` entries.
#[test]
fn imports_pycharm_server_configurations() {
    let root = tmp("webimport");
    std::fs::create_dir_all(root.join("api")).unwrap();
    std::fs::create_dir_all(root.join("server")).unwrap();
    std::fs::create_dir_all(root.join("src/site")).unwrap();
    std::fs::create_dir_all(root.join(".run")).unwrap();
    std::fs::write(root.join("api/main.py"), "from fastapi import FastAPI\napi = FastAPI()\n").unwrap();
    std::fs::write(root.join("app.py"), "from flask import Flask\napp = Flask(__name__)\n").unwrap();
    std::fs::write(root.join("server/web.py"), "from flask import Flask\nweb = Flask(__name__)\n").unwrap();
    std::fs::write(root.join("src/site/manage.py"), "import os\nos.environ.setdefault('DJANGO_SETTINGS_MODULE', 'site.settings')\nimport django\n").unwrap();
    std::fs::write(
        root.join(".run/api.run.xml"),
        r#"<component name="ProjectRunConfigurationManager">
  <configuration default="false" name="api" type="Python.FastAPI" singleton="false" focusToolWindowBeforeRun="true">
    <option name="additionalOptions" value="--port 8001  --reload" />
    <option name="file" value="$PROJECT_DIR$/api/main.py" />
    <option name="ENV_FILES" value="" />
    <option name="WORKING_DIRECTORY" value="" />
    <option name="IS_MODULE_SDK" value="true" />
    <method v="2" />
  </configuration>
</component>"#,
    )
    .unwrap();
    std::fs::write(
        root.join(".run/Flask (app.py).run.xml"),
        r#"<component name="ProjectRunConfigurationManager">
  <configuration default="false" name="Flask (app.py:app)" type="Python.FlaskServer" nameIsGenerated="true">
    <option name="application" value="app" />
    <option name="flaskDebug" value="true" />
    <option name="target" value="$PROJECT_DIR$/app.py" />
    <option name="targetType" value="PATH" />
    <option name="WORKING_DIRECTORY" value="$PROJECT_DIR$" />
    <method v="2" />
  </configuration>
</component>"#,
    )
    .unwrap();
    let ws = r#"<project><component name="RunManager" selected="Django server.runserver">
  <configuration default="false" name="server" type="Python.FlaskServer">
    <option name="application" value="web" />
    <option name="target" value="server.web" />
    <option name="targetType" value="PYTHON" />
    <option name="WORKING_DIRECTORY" value="$PROJECT_DIR$" />
  </configuration>
  <configuration name="runserver" type="Python.DjangoServer" factoryName="Django server">
    <envs>
      <env name="PYTHONUNBUFFERED" value="1" />
      <env name="DJANGO_SETTINGS_MODULE" value="site.settings" />
    </envs>
    <option name="WORKING_DIRECTORY" value="$PROJECT_DIR$" />
    <option name="port" value="8080" />
    <option name="host" value="0.0.0.0" />
    <option name="additionalOptions" value="--insecure" />
    <option name="runNoReload" value="true" />
    <option name="useCustomRunCommand" value="false" />
  </configuration>
  <configuration name="tests" type="DjangoTestsConfigurationType">
    <envs>
      <env name="PYCHARM_DJANGO_MANAGE_MODULE" value="src.site.manage" />
    </envs>
    <option name="TARGET" value="polls" />
    <option name="USE_OPTIONS" value="true" />
    <option name="OPTIONS" value="--noinput" />
  </configuration>
</component></project>"#;
    std::fs::create_dir_all(root.join(".idea")).unwrap();
    std::fs::write(root.join(".idea/workspace.xml"), ws).unwrap();
    let f = config::import_idea(&root);
    let got: Vec<(&str, RunKind, &str, &str)> = f.configurations.iter().map(|c| (c.name.as_str(), c.kind, c.target.as_str(), c.parameters.as_str())).collect();
    assert_eq!(
        got,
        vec![
            ("Flask (app.py:app)", RunKind::Flask, "app:app", ""),
            ("api", RunKind::FastApi, "api.main:api", "--port 8001  --reload"),
            ("server", RunKind::Flask, "server.web:web", ""),
            ("runserver", RunKind::Django, "src/site/manage.py", "0.0.0.0:8080 --noreload --insecure"),
            ("tests", RunKind::DjangoTests, "src/site/manage.py", "polls --noinput"),
        ]
    );
    assert_eq!(f.get("Flask (app.py:app)").unwrap().env.get("FLASK_DEBUG").map(String::as_str), Some("1"));
    assert_eq!(f.get("runserver").unwrap().env.get("DJANGO_SETTINGS_MODULE").map(String::as_str), Some("site.settings"));
    assert!(f.get("tests").unwrap().env.is_empty());
    assert_eq!(f.selected.as_deref(), Some("runserver"));
    let api = f.get("api").unwrap();
    assert_eq!((api.on_start, api.allow_multiple), (OnStart::Focus, true));
    assert_eq!((f.get("tests").unwrap().on_start, f.get("tests").unwrap().allow_multiple), (OnStart::Show, false));
    let _ = std::fs::remove_dir_all(&root);
}

/// "Run with Python Console" (`SHOW_COMMAND_LINE`) adds `-i`; "Emulate terminal in output
/// console" (`EMULATE_TERMINAL`) is kept for the Run window.
#[test]
fn python_console_and_emulate_terminal() {
    let xml = r#"<project><component name="RunManager">
  <configuration name="con" type="PythonConfigurationType" factoryName="Python">
    <option name="SCRIPT_NAME" value="$PROJECT_DIR$/con.py" />
    <option name="SHOW_COMMAND_LINE" value="true" />
    <option name="EMULATE_TERMINAL" value="true" />
  </configuration>
</component></project>"#;
    let (cfgs, _) = super::config::configurations_in(Path::new("/p"), xml, "RunManager");
    assert_eq!((cfgs[0].python_console, cfgs[0].emulate_terminal), (true, true));
    let cmd = build(Path::new("/p"), &RunConfig { interpreter_options: "-X dev".into(), ..cfgs[0].clone() }, Path::new("python"));
    assert_eq!(cmd.args[..3], ["-X".to_string(), "dev".into(), "-i".into()]);
}

/// PyCharm's templates (`default="true"` configurations of `workspace.xml`) become Brisk's
/// Python / pytest templates, and new temporary configurations start from them.
#[test]
fn imports_templates_and_applies_them() {
    let root = tmp("templates");
    std::fs::create_dir_all(root.join(".idea")).unwrap();
    let ws = r#"<project><component name="RunManager">
  <configuration default="true" type="PythonConfigurationType" factoryName="Python">
    <envs>
      <env name="PYTHONUNBUFFERED" value="1" />
      <env name="APP_ENV" value="dev" />
    </envs>
    <option name="WORKING_DIRECTORY" value="$PROJECT_DIR$/src" />
    <option name="EMULATE_TERMINAL" value="true" />
    <option name="SCRIPT_NAME" value="" />
  </configuration>
  <configuration default="true" type="tests" factoryName="py.test">
    <option name="SCRIPT_NAME" value="" />
  </configuration>
  <configuration name="main" type="PythonConfigurationType" factoryName="Python">
    <option name="SCRIPT_NAME" value="$PROJECT_DIR$/main.py" />
  </configuration>
</component></project>"#;
    std::fs::write(root.join(".idea/workspace.xml"), ws).unwrap();
    let mut f = config::import_idea(&root);
    // The empty pytest template is dropped; existing configurations are left as imported.
    assert_eq!(f.templates.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Python"]);
    assert_eq!(f.get("main").unwrap().working_directory, "");
    let name = f.use_for_file(RunConfig::for_file(&root, &root.join("tool.py")));
    let c = f.get(&name).unwrap();
    assert_eq!((c.working_directory.as_str(), c.env.get("APP_ENV").map(String::as_str), c.emulate_terminal), ("$PROJECT_DIR$/src", Some("dev"), true));
    let t = f.use_for_file(RunConfig::for_tests(&root, &root.join("test_x.py"), &[]));
    assert!(f.get(&t).unwrap().env.is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

/// "Store as project file": every kind written as PyCharm's `.run/*.run.xml` reads back the
/// same; unsharing or renaming deletes the old file; a teammate's change or removal of a
/// file (a `git pull`) wins on load; files of other configuration types are left alone.
#[test]
fn project_files_round_trip_and_sync() {
    let root = tmp("projfiles");
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(root.join("main.py"), "").unwrap();
    std::fs::write(root.join("pkg/api.py"), "from fastapi import FastAPI\napp = FastAPI()\n").unwrap();
    std::fs::write(root.join("manage.py"), "").unwrap();
    let env = |k: &str, v: &str| std::iter::once((k.to_string(), v.to_string())).collect();
    let shared = |c: RunConfig| RunConfig { project_file: true, ..c };
    let cfgs = vec![
        shared(RunConfig {
            name: "main".into(),
            target: "main.py".into(),
            parameters: "--x \"a b\"".into(),
            working_directory: "pkg".into(),
            env: env("A", "1&2<3>"),
            env_files: ".env".into(),
            interpreter_options: "-X dev".into(),
            python_console: true,
            allow_multiple: true,
            on_start: OnStart::Focus,
            before_run: "tests".into(),
            ..Default::default()
        }),
        shared(RunConfig {
            name: "mod: run".into(),
            module: true,
            target: "pkg.api".into(),
            emulate_terminal: true,
            interpreter: Some("C:/py/python.exe".into()),
            on_start: OnStart::Nothing,
            working_directory: "$PROJECT_DIR$".into(),
            ..Default::default()
        }),
        shared(RunConfig {
            name: "tests".into(),
            kind: RunKind::Pytest,
            target: "tests".into(),
            keywords: "not slow".into(),
            parameters: "-x".into(),
            ..Default::default()
        }),
        shared(RunConfig { name: "unit".into(), kind: RunKind::Unittest, target: "tests/test_a.py::TestA".into(), ..Default::default() }),
        shared(RunConfig { name: "doc".into(), kind: RunKind::Doctest, target: "main.py".into(), ..Default::default() }),
        shared(RunConfig { name: "api".into(), kind: RunKind::FastApi, target: "pkg.api:app".into(), parameters: "--reload".into(), ..Default::default() }),
        shared(RunConfig { name: "flask".into(), kind: RunKind::Flask, target: "pkg.api:app".into(), ..Default::default() }),
        shared(RunConfig {
            name: "site".into(),
            kind: RunKind::Django,
            target: "manage.py".into(),
            parameters: "127.0.0.1:8001 --noreload".into(),
            ..Default::default()
        }),
        shared(RunConfig { name: "dt".into(), kind: RunKind::DjangoTests, target: "manage.py".into(), parameters: "app.tests".into(), ..Default::default() }),
        shared(RunConfig { name: "all".into(), kind: RunKind::Compound, target: "main;api".into(), ..Default::default() }),
        RunConfig { name: "local".into(), target: "main.py".into(), ..Default::default() },
    ];
    let listing = || {
        let mut v: Vec<String> = std::fs::read_dir(root.join(".run")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    };
    save(&root, &RunFile { configurations: cfgs.clone(), ..Default::default() }).unwrap();
    let files = listing();
    assert_eq!(files.len(), 10, "{files:?}");
    assert!(files.contains(&"mod_ run.run.xml".to_string()) && !files.contains(&"local.run.xml".to_string()), "{files:?}");
    let main_xml = std::fs::read_to_string(root.join(".run/main.run.xml")).unwrap();
    assert!(
        main_xml.contains(r#"<option name="SCRIPT_NAME" value="$PROJECT_DIR$/main.py" />"#) && main_xml.contains(r#"value="1&amp;2&lt;3&gt;""#),
        "{main_xml}"
    );

    // As after a clone (no .brisk): the shared configurations come back unchanged.
    std::fs::remove_file(root.join(CONFIG_FILE)).unwrap();
    let (g, _) = load(&root);
    for c in cfgs.iter().filter(|c| c.project_file) {
        assert_eq!(g.get(&c.name), Some(c), "{}", std::fs::read_to_string(root.join(".run").join(project_files::file_name(&c.name))).unwrap());
    }
    assert!(g.get("local").is_none());

    // Unshare one, rename another: their files go; the unshared one stays in run.json.
    let mut h = g.clone();
    h.configurations.iter_mut().find(|c| c.name == "doc").unwrap().project_file = false;
    h.configurations.iter_mut().find(|c| c.name == "unit").unwrap().name = "unit tests".into();
    std::fs::write(
        root.join(".run/jest.run.xml"),
        r#"<component name="ProjectRunConfigurationManager"><configuration name="jest" type="JavaScriptTestRunnerJest" /></component>"#,
    )
    .unwrap();
    save(&root, &h).unwrap();
    let files = listing();
    assert!(!files.contains(&"doc.run.xml".to_string()) && !files.contains(&"unit.run.xml".to_string()), "{files:?}");
    assert!(files.contains(&"unit tests.run.xml".to_string()) && files.contains(&"jest.run.xml".to_string()), "{files:?}");
    assert_eq!(load(&root).0.get("doc").map(|c| c.project_file), Some(false));

    // A teammate changes one file and deletes another: the files win.
    let p = root.join(".run/tests.run.xml");
    std::fs::write(&p, std::fs::read_to_string(&p).unwrap().replace("&quot;-x&quot;", "&quot;-q&quot;")).unwrap();
    std::fs::remove_file(root.join(".run/api.run.xml")).unwrap();
    let (k, _) = load(&root);
    assert_eq!(k.get("tests").map(|c| c.parameters.as_str()), Some("-q"));
    assert!(k.get("api").is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn link_after_a_no_break_space() {
    let l = find_links("Erreur\u{a0}main.py:1");
    assert_eq!(l.len(), 1);
    let l = find_links("x\u{3000}app/x.py:3: boom");
    assert_eq!(l.len(), 1);
}

#[test]
fn broken_run_json_is_kept() {
    let d = tmp("broken-run-json");
    std::fs::create_dir_all(d.join(".brisk")).unwrap();
    let broken = "{\"configurations\": [{\"name\": \"MyKeep\"},]}";
    std::fs::write(d.join(".brisk/run.json"), broken).unwrap();
    let _ = crate::config::load(&d);
    assert_eq!(std::fs::read_to_string(d.join(".brisk/run.json.bak")).unwrap(), broken);
}

/// Runs saved within the same millisecond get `-1`, `-2`, ... `-10` suffixes; the newest is
/// still listed first.
#[test]
fn test_history_orders_same_millisecond_runs() {
    let root = tmp("test-history-order");
    let d = root.join(".brisk").join("test-history");
    std::fs::create_dir_all(&d).unwrap();
    for (file, name) in [("5 r.jsonl", "first"), ("5-9 r.jsonl", "ninth"), ("5-10 r.jsonl", "tenth"), ("4-11 r.jsonl", "older")] {
        std::fs::write(d.join(file), format!("{{\"event\":\"meta\",\"name\":\"{name}\",\"time\":0}}\n")).unwrap();
    }
    let names: Vec<String> = pytest::history::list(&root).into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["tenth", "ninth", "first", "older"]);
    let _ = std::fs::remove_dir_all(&root);
}
