use super::*;
use futures::executor::block_on;
use futures::StreamExt;
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

fn python() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("BRISK_TEST_PYTHON") {
        return Some(PathBuf::from(p));
    }
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    ["Python314", "Python313", "Python312"].iter().map(|v| local.join("Programs").join("Python").join(v).join("python.exe")).find(|p| p.is_file())
}

#[test]
fn breakpoints_json_is_one_based() {
    let v = set_breakpoints_args(Path::new("a.py"), &[SourceBreakpoint { line: 2, condition: Some("x > 1".into()) }, SourceBreakpoint { line: 5, condition: None }]);
    assert_eq!(v["breakpoints"][0]["line"], 3);
    assert_eq!(v["breakpoints"][0]["condition"], "x > 1");
    assert!(v["breakpoints"][1].get("condition").is_none());
    let back = parse_breakpoints(&json!({"breakpoints": [{"verified": true, "line": 3}, {"verified": false}]}));
    assert_eq!(back, vec![Some(2), None]);
}

#[test]
fn bootstrap_path_is_a_python_literal() {
    // `{:?}` would write the zero-width space as `\u{200b}`, which Python rejects.
    let boot = bootstrap(Path::new("C:\\Users\\a\"b\\lib\u{200b}"));
    assert!(boot.contains("sys.path.insert(0, \"C:\\\\Users\\\\a\\\"b\\\\lib\u{200b}\");"), "{boot}");
}

#[test]
fn bad_json_is_skipped() {
    let data = b"Content-Length: 5\r\n\r\n{oops\r\nContent-Length: 2\r\n\r\n{}";
    let mut r = std::io::Cursor::new(&data[..]);
    assert_eq!(read_msg(&mut r), Some(Value::Null));
    assert_eq!(read_msg(&mut r), Some(json!({})));
    assert_eq!(read_msg(&mut r), None);
}

/// The whole flow against a real debugpy (skipped when there is none).
#[test]
fn debug_a_script() {
    let Some(py) = python() else { return };
    let Ok(cmd) = adapter_command(&py) else {
        eprintln!("no debugpy, skipping");
        return;
    };
    let dir = std::env::temp_dir().join(format!("brisk-dap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("prog.py");
    std::fs::write(&script, "def double(n):\n    return n * 2\n\nx = 21\ny = double(x)\nprint('result', y)\n").unwrap();

    let (client, mut events) = Client::start(&cmd.program, &cmd.args, &dir, &[]).expect("adapter");
    let (etx, erx) = std_mpsc::channel();
    std::thread::spawn(move || {
        while let Some(e) = block_on(events.next()) {
            if etx.send(e).is_err() {
                break;
            }
        }
    });
    let wait_for = |name: &str| -> Value {
        loop {
            match erx.recv_timeout(Duration::from_secs(90)).unwrap_or_else(|_| panic!("timed out waiting for {name}")) {
                Event::Dap { event, body } if event == name => return body,
                Event::AdapterExited => panic!("adapter exited waiting for {name}"),
                _ => {}
            }
        }
    };
    block_on(client.request("initialize", initialize_args())).unwrap().unwrap();
    let launch = client.request(
        "launch",
        launch_args(&Launch { python: py.clone(), target: script.to_string_lossy().into(), cwd: dir.clone(), just_my_code: true, ..Default::default() }),
    );
    wait_for("initialized");
    let bp = block_on(client.request("setBreakpoints", set_breakpoints_args(&script, &[SourceBreakpoint { line: 4, condition: None }]))).unwrap().unwrap();
    assert_eq!(parse_breakpoints(&bp), vec![Some(4)]);
    block_on(client.request("setExceptionBreakpoints", json!({"filters": []}))).unwrap().unwrap();
    block_on(client.request("configurationDone", json!({}))).unwrap().unwrap();
    block_on(launch).unwrap().unwrap();

    let stopped = wait_for("stopped");
    let thread = stopped["threadId"].as_i64().unwrap();
    let frames = parse_frames(&block_on(client.request("stackTrace", json!({"threadId": thread}))).unwrap().unwrap());
    assert_eq!(frames[0].line, 4, "{frames:?}");
    let scopes = parse_scopes(&block_on(client.request("scopes", json!({"frameId": frames[0].id}))).unwrap().unwrap());
    let vars = parse_variables(&block_on(client.request("variables", json!({"variablesReference": scopes[0].reference}))).unwrap().unwrap());
    let x = vars.iter().find(|v| v.name == "x").expect("x");
    assert_eq!((x.value.as_str(), x.type_name.as_str()), ("21", "int"));
    let (r, _) = parse_evaluate(&block_on(client.request("evaluate", json!({"expression": "x * 2", "frameId": frames[0].id, "context": "repl"}))).unwrap().unwrap());
    assert_eq!(r, "42");

    // Step into double(), then out, then run to the end.
    block_on(client.request("stepIn", json!({"threadId": thread}))).unwrap().unwrap();
    wait_for("stopped");
    let frames = parse_frames(&block_on(client.request("stackTrace", json!({"threadId": thread}))).unwrap().unwrap());
    assert_eq!((frames[0].name.as_str(), frames[0].line), ("double", 1), "{frames:?}");
    block_on(client.request("continue", json!({"threadId": thread}))).unwrap().unwrap();
    let mut out = String::new();
    loop {
        match erx.recv_timeout(Duration::from_secs(90)).expect("event timeout") {
            Event::Dap { event, body } if event == "output" => out.push_str(body["output"].as_str().unwrap_or("")),
            Event::Dap { event, .. } if event == "terminated" => break,
            Event::AdapterExited => break,
            _ => {}
        }
    }
    assert!(out.contains("result 42"), "{out:?}");
    let _ = block_on(client.request("disconnect", json!({})));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn python_processes_filter() {
    let ps = "12\tC:\\Py\\python.exe app.py --x\n13\t\"C:\\Program Files\\Py\\pythonw.exe\" gui.pyw\n14\tpython -m debugpy.adapter\n15\tnode server.js\n16\t/usr/bin/python3 worker.py\nbad line\n17\tpython own.py\n";
    let p = python_processes(ps, 17);
    assert_eq!(p.iter().map(|x| x.0).collect::<Vec<_>>(), [12, 13, 16]);
    assert_eq!(p[0].1, "C:\\Py\\python.exe app.py --x");
    assert_eq!(attach_args(42, true)["processId"], 42);
}
