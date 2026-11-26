//! BRISK_SMOKE_SSH=<host> on an empty folder: the host's first Python becomes the project
//! interpreter (as Add SSH Interpreter does), a script that imports a project module and
//! fails is run there, its traceback links open the local files, an edit uploads only the
//! changed file, and the project folder on the host is removed at the end.

use crate::workspace::Workspace;
use gpui::{Context, Window};

impl Workspace {
    pub(in crate::workspace) fn ssh_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let host = std::env::var("BRISK_SMOKE_SSH").unwrap_or_default();
        let root = self.root.clone();
        let _ = std::fs::create_dir_all(root.join("pkg"));
        let _ = std::fs::write(root.join("pkg/__init__.py"), "");
        let _ = std::fs::write(
            root.join("pkg/util.py"),
            "import platform\n\n\ndef where():\n    return platform.node()\n\n\ndef boom():\n    raise ValueError('from the host')\n",
        );
        let _ = std::fs::write(root.join("app.py"), "import sys\nfrom pkg import util\n\nprint('host', util.where(), sys.version.split()[0])\nutil.boom()\n");
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            let h = host.clone();
            let info = bg.spawn(async move { brisk_run::ssh::probe(&h) }).await;
            let info = match info {
                Ok(i) => i,
                Err(e) => {
                    eprintln!("smoke-ssh: probe {host} failed: {e}");
                    let _ = this.update(cx, |_, cx| cx.quit());
                    return;
                }
            };
            eprintln!("smoke-ssh: home {:?}, pythons {:?}", info.home, info.pythons);
            let Some((py, ver)) = info.pythons.first().cloned() else {
                let _ = this.update(cx, |_, cx| cx.quit());
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.env.update(cx, |e, _| e.ssh_found = Some((host.clone(), info)));
                this.ssh_pick_python(host.clone(), py, ver, window, cx);
                eprintln!("smoke-ssh: interpreter {:?} / {:?}", this.interpreter_label(cx), this.env.read(cx).selected);
            });
            for round in 0..2 {
                if round == 1 {
                    let _ = std::fs::write(
                        root.join("app.py"),
                        "import sys\nfrom pkg import util\n\nprint('host', util.where(), sys.version.split()[0], 'again')\nutil.boom()\n",
                    );
                }
                let path = root.join("app.py");
                let _ = this.update_in(cx, |this, window, cx| {
                    this.open_file(&path, window, cx);
                    if let Some(cfg) = this.context_config(window, cx) {
                        this.run_config(cfg, window, cx);
                    }
                });
                let mut out = String::new();
                let mut uploaded = String::new();
                for _ in 0..300 {
                    pause(100).await;
                    let (o, st) = this
                        .update(cx, |t, cx| (t.runs.panel.as_ref().and_then(|p| p.read(cx).active_text()).unwrap_or_default(), t.status.clone()))
                        .unwrap_or_default();
                    if st.starts_with("Uploaded") {
                        uploaded = st;
                    }
                    out = o;
                    if out.contains("Process finished") && (round == 0 || out.contains("again")) {
                        break;
                    }
                }
                eprintln!("smoke-ssh: run {round} upload {uploaded:?}");
                let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
                eprintln!("smoke-ssh: run {round} output {lines:?}");
                let _ = this.update(cx, |this, cx| {
                    let link = this.runs.panel.as_ref().and_then(|p| p.read(cx).first_link());
                    eprintln!("smoke-ssh: run {round} status {:?}, first link {link:?}", this.status);
                });
            }
            let rr = this.update(cx, |t, cx| t.env.read(cx).ssh().map(|s| s.remote_root)).ok().flatten().unwrap_or_default();
            if rr.contains("brisk-projects/") {
                let h = host.clone();
                let script = format!("rm -rf {}", brisk_run::ssh::quote(&rr));
                let res = bg.spawn(async move { brisk_run::ssh::run_script(&h, &script, None) }).await;
                eprintln!("smoke-ssh: removed {rr} -> {:?}", res.map(|_| "ok"));
            }
            let _ = this.update(cx, |_, cx| cx.quit());
        })
        .detach();
    }
}
