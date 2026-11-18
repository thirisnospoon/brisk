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
    assert_eq!((l[0].path.as_str(), l[0].line), ("b\\x.py", 3));
    let l = find_links(r"warn at C:\p\x.py:3: UserWarning");
    assert_eq!(l[0].path, r"C:\p\x.py");
    // Cyrillic before the link: columns are characters.
    let l = find_links("ошибка app.py:5");
    assert_eq!(l[0].cols, 7..15);
    assert!(find_links("time 12:30 ok").is_empty());
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
    assert_eq!(n, 4);
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
    std::fs::write(&script, "import sys\nname = input('name? ')\nprint('привет', name)\nsys.stdout.flush()\ndef f():\n    raise ValueError('boom')\nf()\n").unwrap();
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
    assert!(text.contains("name? Bob
"), "{text}");
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
    std::fs::write(&script, "import sys\nfor i in range(30):\n    print('out', i, 'x', 'y')\n    print('err', i, file=sys.stderr)\n").unwrap();
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
    std::fs::write(&script, "import subprocess, sys, time\nsubprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])\nprint('started', flush=True)\ntime.sleep(60)\n").unwrap();
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
    assert_eq!(names, [(0, "Test Results".into()), (1, "test_a".into()), (2, "test_ok".into()), (2, "TestA".into()), (3, "test_b".into()), (2, "test_skip".into())]);
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
    assert!(names.contains(&"test_param[1]".to_string()) && names.contains(&"TestBad".to_string()), "{names:?}");

    // Rerun Failed Tests: only the failed node runs, from the same folder.
    let failed = t.failed_targets();
    assert_eq!(failed.len(), 1);
    let again = RunConfig { target: failed.iter().map(|f| format!("\"{f}\"")).collect::<Vec<_>>().join(" "), working_directory: relativize(&root, &cmd.cwd), ..cfg.clone() };
    let (feed2, text2, cmd2) = run(&again);
    assert_eq!(cmd2.args[2..], ["test_calc.py::TestBad::test_wrong"]);
    assert_eq!(feed2.tree.counts().total, 1, "{text2}");
    assert_eq!(feed2.tree.counts().failed, 1);
    drop(feed);
    drop(feed2);
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
