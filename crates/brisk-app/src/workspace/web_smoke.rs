//! BRISK_SMOKE_WEB=1 on an empty folder (the project interpreter needs fastapi, uvicorn
//! and flask): FastAPI and Flask apps are found from their files, run as servers, answer
//! HTTP on the address their output prints, and that address is a link that opens.

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
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.runs.interpreter(&t.root).is_some()).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| eprintln!("smoke-web: dialog + FastAPI / + Flask -> {:?}", this.web_dialog_probe(cx)));
            for (file, port) in [("api/main.py", 18765), ("webapp.py", 18766)] {
                let path = root.join(file);
                let _ = this.update_in(cx, |this, window, cx| {
                    this.open_file(&path, window, cx);
                    let Some(mut cfg) = this.context_config(window, cx) else {
                        eprintln!("smoke-web: {file} -> no context configuration");
                        return;
                    };
                    eprintln!("smoke-web: {file} -> {:?} '{}' target {:?}", cfg.kind, cfg.name, cfg.target);
                    // Fixed ports keep the smoke off servers already running on this machine.
                    cfg.parameters = format!("--port {port}");
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
            // Stop both servers before quitting.
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
