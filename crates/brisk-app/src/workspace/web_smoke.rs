//! BRISK_SMOKE_WEB=1 on an empty folder (the project interpreter needs fastapi, uvicorn
//! and flask): FastAPI and Flask apps are found from their files, run as servers, answer
//! HTTP on the address their output prints, and that address is a link that opens. Django
//! runs against a stub `django` package written next to `site/manage.py` (it answers
//! `runserver` and `test` the way Django prints them), so no Django install is needed.

use super::Workspace;
use gpui::{Context, Window};
use std::io::{Read, Write};

/// `GET path` on `http://host:port/...`: the status line and the body's first 60 chars.
fn http_get(url: &str) -> String {
    let rest = url.trim_start_matches("http://");
    let (hostport, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
    let hostport = hostport.replace("0.0.0.0", "127.0.0.1");
    let Ok(mut s) = std::net::TcpStream::connect(&hostport) else { return format!("cannot connect to {hostport}") };
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let _ = write!(s, "GET {path} HTTP/1.0\r\nHost: {hostport}\r\n\r\n");
    let mut buf = String::new();
    let _ = s.read_to_string(&mut buf);
    let status = buf.lines().next().unwrap_or("").to_string();
    let body = buf.split("\r\n\r\n").nth(1).unwrap_or("").chars().take(60).collect::<String>();
    format!("{status} {body}")
}

impl Workspace {
    pub(super) fn web_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let _ = std::fs::create_dir_all(root.join("api"));
        let _ = std::fs::write(root.join("api/main.py"), "from fastapi import FastAPI\n\napp = FastAPI()\n\n\n@app.get(\"/\")\ndef index():\n    return {\"hello\": \"fastapi\"}\n");
        let _ = std::fs::write(root.join("webapp.py"), "from flask import Flask\n\nweb = Flask(__name__)\n\n\n@web.route(\"/\")\ndef index():\n    return \"hello flask\"\n");
        // A stub of Django's command line: enough to check what Brisk starts.
        let _ = std::fs::create_dir_all(root.join("site/django/core/management"));
        let _ = std::fs::write(root.join("site/manage.py"), "import os\nimport sys\n\n\ndef main():\n    os.environ.setdefault('DJANGO_SETTINGS_MODULE', 'mysite.settings')\n    from django.core.management import execute_from_command_line\n    execute_from_command_line(sys.argv)\n\n\nif __name__ == '__main__':\n    main()\n");
        let _ = std::fs::write(root.join("site/django/__init__.py"), "");
        let _ = std::fs::write(root.join("site/django/core/__init__.py"), "");
        let _ = std::fs::write(
            root.join("site/django/core/management/__init__.py"),
            "import http.server, os\n\n\nclass H(http.server.BaseHTTPRequestHandler):\n    def do_GET(self):\n        self.send_response(200)\n        self.end_headers()\n        self.wfile.write(b'hello django')\n\n\ndef execute_from_command_line(argv):\n    settings = os.environ['DJANGO_SETTINGS_MODULE']\n    if argv[1] == 'runserver':\n        addr = argv[2] if len(argv) > 2 else '127.0.0.1:8000'\n        host, port = addr.rsplit(':', 1)\n        print(f'Django version stub, using settings {settings!r}')\n        print(f'Starting development server at http://{addr}/')\n        http.server.HTTPServer((host, int(port)), H).serve_forever()\n    elif argv[1] == 'test':\n        print(f'Ran 1 test, labels {argv[2:]}, settings {settings}')\n        print('OK')\n",
        );
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.runs.interpreter(&t.root).is_some()).unwrap_or(false) {
                    break;
                }
            }
            let keep = std::env::var("BRISK_SMOKE_WEB").is_ok_and(|v| v.starts_with("dialog"));
            let _ = this.update(cx, |this, cx| eprintln!("smoke-web: dialog + FastAPI / + Flask / + Django / + Django tests -> {:?}", this.web_dialog_probe(keep, cx)));
            if keep {
                return;
            }
            for (file, params) in [("api/main.py", "--port 18765"), ("webapp.py", "--port 18766"), ("site/manage.py", "127.0.0.1:18767")] {
                let path = root.join(file);
                let _ = this.update_in(cx, |this, window, cx| {
                    this.open_file(&path, window, cx);
                    let Some(mut cfg) = this.context_config(window, cx) else {
                        eprintln!("smoke-web: {file} -> no context configuration");
                        return;
                    };
                    eprintln!("smoke-web: {file} -> {:?} '{}' target {:?}", cfg.kind, cfg.name, cfg.target);
                    // Fixed ports keep the smoke off servers already running on this machine.
                    cfg.parameters = params.to_string();
                    this.run_config(cfg, window, cx);
                });
                let mut url = None;
                for _ in 0..150 {
                    pause(100).await;
                    url = this.update(cx, |t, cx| t.runs.panel.as_ref().and_then(|p| p.read(cx).first_url())).ok().flatten();
                    if url.is_some() {
                        break;
                    }
                }
                let Some(url) = url else {
                    let out = this.update(cx, |t, cx| t.runs.panel.as_ref().and_then(|p| p.read(cx).active_text())).ok().flatten().unwrap_or_default();
                    eprintln!("smoke-web: {file} printed no URL: {:?}", out.lines().rev().take(6).collect::<Vec<_>>());
                    continue;
                };
                pause(500).await;
                let get = bg.spawn({
                    let u = url.clone();
                    async move { http_get(&u) }
                });
                eprintln!("smoke-web: {file} url {url:?}, GET -> {:?}", get.await);
                let _ = this.update(cx, |this, cx| {
                    let clicked = this.runs.panel.clone().is_some_and(|p| p.update(cx, |p, cx| p.click_first_url(cx)));
                    eprintln!("smoke-web: click link {clicked}");
                });
                pause(200).await;
                let _ = this.update(cx, |this, _| eprintln!("smoke-web: opened {:?}", this.last_url));
            }
            // Django tests through `manage.py test`.
            let _ = this.update_in(cx, |this, window, cx| {
                let mut cfg = brisk_run::RunConfig::for_django(&root, brisk_run::RunKind::DjangoTests, &root.join("site/manage.py"), false);
                cfg.parameters = "polls".into();
                this.run_config(cfg, window, cx);
            });
            for _ in 0..100 {
                pause(100).await;
                let out = this.update(cx, |t, cx| t.runs.panel.as_ref().and_then(|p| p.read(cx).active_text())).ok().flatten().unwrap_or_default();
                if out.contains("Process finished") {
                    let first = out.lines().next().unwrap_or("").to_string();
                    let ran = out.lines().find(|l| l.starts_with("Ran")).unwrap_or("-").to_string();
                    eprintln!("smoke-web: django tests -> {first:?} / {ran:?}");
                    break;
                }
            }
            // Stop the servers before quitting.
            let _ = this.update(cx, |this, cx| {
                if let Some(p) = this.runs.panel.clone() {
                    p.update(cx, |p, _| p.stop_all());
                }
            });
            for _ in 0..50 {
                pause(100).await;
                if this.update(cx, |t, cx| !t.runs.panel.as_ref().is_some_and(|p| p.read(cx).run_tabs().iter().any(|(_, r)| *r))).unwrap_or(true) {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| {
                eprintln!("smoke-web: after stop -> tabs {:?}", this.runs.panel.as_ref().map(|p| p.read(cx).run_tabs()));
                cx.quit();
            });
        })
        .detach();
    }
}
