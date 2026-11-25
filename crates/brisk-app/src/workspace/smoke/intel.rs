//! `BRISK_SMOKE_INTEL=1`: code intelligence end to end with ty and ruff. Writes `intel.py` in
//! the project, then checks Alt+Enter auto-import (and that one Undo takes it back),
//! Optimize Imports, auto-import completion and the documentation preview beside the list.
//! With `BRISK_SMOKE_INTEL_HOLD=1` it stops with Quick Documentation (Ctrl+Q) open on
//! `defaultdict`; with `BRISK_SMOKE_INTEL_HOLD=list`, with the completion list and its preview;
//! with `BRISK_SMOKE_INTEL_HOLD=dumps`, with Quick Documentation on `json.dumps` (prose
//! with inline code); with `=usages`, `=inlay` or `=preview`, with the Find Usages tree, inlay
//! hints or the rename's Refactoring Preview.
//! At the end: Rename (Shift+F6) of a local, of a function (comments and strings, Preview, Do
//! Refactor, one undo in both files) and of a module (its file too); Alt+Enter intentions on
//! `intents.py` (import from the project, Create function, type hints, f-string, 'if' ones).

use crate::workspace::{Target, Workspace};
use gpui::{AsyncWindowContext, Context, WeakEntity, Window};
use std::time::{Duration, Instant};

const SRC: &str = "import sys\nimport os\n\nx = defaultdict(list)\nprint(os.sep, x)\n";

fn pass(ok: bool) -> &'static str {
    if ok {
        "ok"
    } else {
        "FAIL"
    }
}

/// No HTML entity or backslash escape left over from ty's Markdown.
fn clean(s: &str) -> bool {
    !["&gt;", "&lt;", "&amp;", "\\_", "\\*"].iter().any(|e| s.contains(e))
}

impl Workspace {
    pub(in crate::workspace) fn intel_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t0 = Instant::now();
        let file = self.root.join("intel.py");
        if let Err(e) = std::fs::write(&file, SRC) {
            eprintln!("smoke-intel: cannot write {}: {e}", file.display());
            return;
        }
        self.open_file(&file, window, cx);
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let pause = |ms| bg.timer(Duration::from_millis(ms));
            let wait = async |this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, f: &dyn Fn(&Self, &gpui::App) -> bool| {
                for _ in 0..600 {
                    if this.update(cx, |this, cx| f(this, cx)).unwrap_or(true) {
                        return;
                    }
                    pause(50).await;
                }
            };
            let text = |this: &Self, cx: &gpui::App| this.active_editor().map(|e| e.read(cx).text_string()).unwrap_or_default();
            // ty running and the file checked (ruff's F821 stands for ty's unresolved-reference).
            wait(&this, cx, &|this, _| this.lsp.state != crate::workspace::lsp_ui::TyState::Unknown && this.lsp.state != crate::workspace::lsp_ui::TyState::Starting).await;
            eprintln!("smoke-intel: ty ready after {:.1?}", t0.elapsed());
            wait(&this, cx, &|this, cx| this.active_editor().is_some_and(|e| e.read(cx).diagnostics.iter().any(|d| d.code == "F821" || d.code == "unresolved-reference"))).await;
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-intel: ty {:?} after {:.1?}", this.lsp.state, t0.elapsed());
                if let Some(ed) = this.active_editor().cloned() {
                    for d in &ed.read(cx).diagnostics {
                        eprintln!("smoke-intel: {:?} {}:{} {}", d.severity, d.start.line + 1, d.start.col + 1, d.title());
                    }
                    // Alt+Enter on `defaultdict`.
                    ed.update(cx, |e, cx| e.select_on_line(3, 6..6, cx));
                }
                this.problems.intents.clear();
                this.show_intentions(window, cx);
            });
            wait(&this, cx, &|this, _| !this.problems.intents.is_empty()).await;
            let _ = this.update_in(cx, |this, window, cx| {
                let labels: Vec<String> = this.problems.intents.iter().map(|i| format!("{} [{}]", i.label, i.detail)).collect();
                eprintln!("smoke-intel: Alt+Enter -> {labels:?}");
                let pick = this.problems.intents.iter().find(|i| i.label.contains("collections")).map(|i| i.target.clone());
                this.popup = None;
                if let Some(Target::Intention(i)) = pick {
                    this.apply_intent(*i, window, cx);
                }
                eprintln!("smoke-intel: status {:?}", this.status);
                let t = text(this, cx);
                eprintln!("smoke-intel: after import {:?}", t.lines().take(4).collect::<Vec<_>>());
                if let Some(ed) = this.active_editor().cloned() {
                    ed.update(cx, |e, _| e.buffer.undo());
                    eprintln!("smoke-intel: one undo restores the file: {}", text(this, cx) == SRC);
                    ed.update(cx, |e, _| e.buffer.redo());
                }
                // Optimize Imports drops `import sys` and sorts.
                this.dispatch(brisk_keymap::Action::OptimizeImports, window, cx);
                eprintln!("smoke-intel: Ctrl+Alt+O -> {:?}", this.status);
                eprintln!("smoke-intel: after optimize {:?}", text(this, cx).lines().take(4).collect::<Vec<_>>());
                this.dispatch(brisk_keymap::Action::OptimizeImports, window, cx);
                eprintln!("smoke-intel: Ctrl+Alt+O again -> {:?}", this.status);
                // Auto-import completion at the end of the file.
                if let Some(ed) = this.active_editor().cloned() {
                    ed.update(cx, |e, cx| {
                        let end = e.buffer.len_chars();
                        e.buffer.set_caret(end, false);
                        e.type_for_smoke("y = ", cx);
                        e.close_insight();
                    });
                }
            });
            // Key by key, as typing does: each keystroke is its own event for the workspace.
            for ch in "OrderedD".chars() {
                pause(40).await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(ed) = this.active_editor().cloned() {
                        ed.update(cx, |e, cx| e.type_for_smoke(&ch.to_string(), cx));
                    }
                });
            }
            wait(&this, cx, &|this, cx| this.active_editor().is_some_and(|e| e.read(cx).completion_open())).await;
            pause(200).await;
            let _ = this.update_in(cx, |this, _, cx| {
                let Some(ed) = this.active_editor().cloned() else { return };
                eprintln!("smoke-intel: after 'OrderedD' -> {}", ed.read(cx).insight_summary());
                let before = text(this, cx);
                ed.update(cx, |e, cx| e.completion_key("enter", cx));
                let after = text(this, cx);
                eprintln!("smoke-intel: accepted -> {:?} ... {:?}", after.lines().take(3).collect::<Vec<_>>(), after.lines().last());
                eprintln!("smoke-intel: caret {:?}", ed.read(cx).caret_line_col());
                ed.update(cx, |e, _| e.buffer.undo());
                eprintln!("smoke-intel: one undo removes word and import: {}", text(this, cx) == before);
                ed.update(cx, |e, _| e.buffer.redo());
                // Documentation preview of the selected item.
                ed.update(cx, |e, cx| e.type_for_smoke("\nos.", cx));
            });
            pause(300).await;
            for ch in "pa".chars() {
                let _ = this.update(cx, |this, cx| {
                    if let Some(ed) = this.active_editor().cloned() {
                        ed.update(cx, |e, cx| e.type_for_smoke(&ch.to_string(), cx));
                    }
                });
                pause(40).await;
            }
            wait(&this, cx, &|this, cx| this.active_editor().is_some_and(|e| e.read(cx).completion_open())).await;
            pause(200).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(ed) = this.active_editor() {
                    eprintln!("smoke-intel: after 'os.pa' -> {}", ed.read(cx).insight_summary());
                    let preview = ed.read(cx).doc_shown_text().1.unwrap_or_default();
                    eprintln!("smoke-intel: preview clean of Markdown escapes: {} ({:?}...)", clean(&preview), preview.chars().take(60).collect::<String>());
                }
                cx.notify();
            });
            let hold = std::env::var("BRISK_SMOKE_INTEL_HOLD").ok();
            if hold.as_deref() == Some("list") {
                eprintln!("smoke-intel: holding");
                return;
            }
            // Quick Documentation on `defaultdict`: ty's docstring has `--&gt;` and `\_`.
            let target = if hold.as_deref() == Some("dumps") { "dumps(1)" } else { "defaultdict(list)" };
            if target == "dumps(1)" {
                let _ = this.update(cx, |this, cx| {
                    let Some(ed) = this.active_editor().cloned() else { return };
                    ed.update(cx, |e, cx| {
                        e.close_insight();
                        let end = e.buffer.len_chars();
                        e.buffer.set_caret(end, false);
                        e.type_for_smoke("\nimport json\njson.dumps(1)\n", cx);
                    });
                });
                // Let the Parameter Info that `(` asks for arrive, then close it.
                pause(800).await;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(ed) = this.active_editor().cloned() else { return };
                ed.update(cx, |e, _| while e.close_insight() {});
                let t = text(this, cx);
                let Some(at) = t.find(target) else { return };
                ed.update(cx, |e, _| {
                    e.close_insight();
                    e.buffer.set_caret(t[..at].chars().count() + 3, false);
                });
                this.smoke_press("ctrl-q", window, cx);
            });
            wait(&this, cx, &|this, cx| this.active_editor().is_some_and(|e| e.read(cx).doc_shown_text().0.is_some())).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(ed) = this.active_editor() {
                    let doc = ed.read(cx).doc_shown_text().0.unwrap_or_default();
                    let after = doc.rsplit("---").next().unwrap_or("").trim();
                    eprintln!("smoke-intel: Ctrl+Q clean of Markdown escapes: {}, text {:?}, {} lines", clean(&doc), after.lines().next().unwrap_or(""), doc.lines().count());
                }
                cx.notify();
            });
            if matches!(hold.as_deref(), Some("1" | "dumps")) {
                eprintln!("smoke-intel: holding");
                return;
            }
            // Rename a module in the Project tree: its importers follow.
            let _ = this.update_in(cx, |this, window, cx| {
                let pkg = this.root.join("smokepkg");
                let _ = std::fs::create_dir_all(&pkg);
                let _ = std::fs::write(pkg.join("__init__.py"), "");
                let _ = std::fs::write(pkg.join("util.py"), "def add(a, b):\n    return a + b\n");
                // An importer inside the package, which moves along with the folder later.
                let _ = std::fs::write(pkg.join("inner.py"), "from smokepkg.util import add\n\nprint(add(3, 4))\n");
                let _ = std::fs::write(this.root.join("uses_util.py"), "from smokepkg.util import add\nimport smokepkg.util\n\nprint(add(1, 2), smokepkg.util.add)\n");
                // As the disk poll does when files appear: rescan, which tells ty.
                this.reindex_files(cx);
                let _ = window;
            });
            wait(&this, cx, &|this, _| this.files.as_ref().is_some_and(|f| f.files.iter().any(|f| f == "uses_util.py"))).await;
            let _ = this.update_in(cx, |this, window, cx| {
                let pkg = this.root.join("smokepkg");
                this.file_op = Some(crate::workspace::files_ui::FileOp::Rename(pkg.join("util.py")));
                this.run_file_op(pkg.join("helpers.py"), window, cx);
            });
            wait(&this, cx, &|this, _| this.lsp.moved.is_none() && this.root.join("smokepkg").join("helpers.py").exists()).await;
            pause(200).await;
            let _ = this.update(cx, |this, _| {
                eprintln!("smoke-intel: rename module -> {:?}", this.status);
                let t = std::fs::read_to_string(this.root.join("uses_util.py")).unwrap_or_default();
                let ok = t == "from smokepkg.helpers import add\nimport smokepkg.helpers\n\nprint(add(1, 2), smokepkg.helpers.add)\n";
                eprintln!("smoke-intel: rename module {}: uses_util.py {:?}", pass(ok), t.lines().collect::<Vec<_>>());
            });
            // Rename the package folder: imports of it everywhere follow, including files
            // that moved with it.
            let _ = this.update_in(cx, |this, window, cx| {
                this.file_op = Some(crate::workspace::files_ui::FileOp::Rename(this.root.join("smokepkg")));
                this.run_file_op(this.root.join("smokelib"), window, cx);
            });
            wait(&this, cx, &|this, _| this.lsp.moved.is_none() && this.root.join("smokelib").join("helpers.py").exists()).await;
            pause(200).await;
            let _ = this.update(cx, |this, _| {
                eprintln!("smoke-intel: rename package -> {:?}", this.status);
                let t = std::fs::read_to_string(this.root.join("uses_util.py")).unwrap_or_default();
                let ok = t == "from smokelib.helpers import add\nimport smokelib.helpers\n\nprint(add(1, 2), smokelib.helpers.add)\n";
                eprintln!("smoke-intel: rename package {}: uses_util.py {:?}", pass(ok), t.lines().collect::<Vec<_>>());
                let t = std::fs::read_to_string(this.root.join("smokelib").join("inner.py")).unwrap_or_default();
                eprintln!("smoke-intel: rename package {}: smokelib/inner.py {:?}", pass(t.starts_with("from smokelib.helpers import add\n")), t.lines().next().unwrap_or(""));
                eprintln!("smoke-intel: rename package {}: old folder gone", pass(!this.root.join("smokepkg").exists()));
                // A file that still names the old package: ty must flag it, which proves ty
                // has seen the move before the clean importer below counts.
                let _ = std::fs::write(this.root.join("probe_old.py"), "import smokepkg.helpers\n");
            });
            let _ = this.update_in(cx, |this, window, cx| {
                this.reindex_files(cx);
                let f = this.root.join("probe_old.py");
                this.open_file(&f, window, cx);
            });
            let unresolved = |this: &Self, cx: &gpui::App| -> Vec<String> {
                this.active_editor().map(|e| e.read(cx).diagnostics.iter().filter(|d| d.code.starts_with("unresolved") || d.code == "import-not-found").map(|d| d.title()).collect()).unwrap_or_default()
            };
            // ty may check the new file before it has handled the move and push no update
            // later: touch the file each second so it checks again.
            for _ in 0..15 {
                if this.update(cx, |this, cx| !unresolved(this, cx).is_empty()).unwrap_or(true) {
                    break;
                }
                pause(1000).await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(ed) = this.active_editor().cloned() {
                        ed.update(cx, |e, cx| e.type_for_smoke("\n", cx));
                    }
                });
            }
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-intel: rename package {}: ty flags the old name: {:?}", pass(!unresolved(this, cx).is_empty()), unresolved(this, cx));
                let f = this.root.join("uses_util.py");
                this.open_file(&f, window, cx);
            });
            pause(1500).await;
            let _ = this.update(cx, |this, cx| {
                let bad = unresolved(this, cx);
                eprintln!("smoke-intel: rename package {}: uses_util.py unresolved {bad:?}", pass(bad.is_empty()));
                let _ = std::fs::remove_file(this.root.join("probe_old.py"));
            });
            // Find Usages of `add` as a tree: usage groups, then files, then lines; Left on a
            // line goes to its file, Left again folds the file.
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(ed) = this.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.select_on_line(3, 7..7, cx));
                }
                this.dispatch(brisk_keymap::Action::FindUsages, window, cx);
            });
            wait(&this, cx, &|this, _| !this.find_window_rows().is_empty()).await;
            let _ = this.update_in(cx, |this, window, cx| {
                for r in this.find_window_rows() {
                    eprintln!("smoke-intel: usages | {r}");
                }
                for k in ["left", "left"] {
                    let ks = gpui::Keystroke::parse(k).expect("keystroke");
                    this.find_window_key(&ks, window, cx);
                }
                eprintln!("smoke-intel: after Left, Left -> {:?}", this.find_window_rows());
                // Unfold again so a held window shows the whole tree.
                let ks = gpui::Keystroke::parse("right").expect("keystroke");
                this.find_window_key(&ks, window, cx);
            });
            if hold.as_deref() == Some("usages") {
                eprintln!("smoke-intel: holding");
                return;
            }
            // Inlay hints: inferred types and parameter names inside the lines.
            let _ = this.update_in(cx, |this, window, cx| {
                let f = this.root.join("hints.py");
                let _ = std::fs::write(&f, "def f(a: int, b: str = 'x') -> list[int]:
    return [a]

x = f(1, b='y')
y = [i * 2 for i in x]
");
                this.reindex_files(cx);
                this.open_file(&f, window, cx);
            });
            wait(&this, cx, &|this, cx| this.active_editor().is_some_and(|e| !e.read(cx).inlay_labels(3).is_empty())).await;
            let _ = this.update(cx, |this, cx| {
                let Some(ed) = this.active_editor().cloned() else { return };
                let labels: Vec<Vec<String>> = (3..5).map(|l| ed.read(cx).inlay_labels(l)).collect();
                eprintln!("smoke-intel: inlay hints lines 4-5 {labels:?}");
                // The text is untouched, and typing hides a line's hints until ty answers again.
                eprintln!("smoke-intel: inlay text unchanged: {}", ed.read(cx).buffer.line(3) == "x = f(1, b='y')");
                ed.update(cx, |e, cx| {
                    e.buffer.set_caret(e.buffer.line_start(3) + 1, false);
                    e.type_for_smoke("x", cx);
                });
                eprintln!("smoke-intel: after typing, line 4 hints {:?}", ed.read(cx).inlay_labels(3));
            });
            pause(1500).await;
            let _ = this.update(cx, |this, cx| {
                let Some(ed) = this.active_editor().cloned() else { return };
                eprintln!("smoke-intel: after ty's check, line 4 hints {:?}", ed.read(cx).inlay_labels(3));
                // Settings > Editor > "Show inlay hints" hides and brings them back at once.
                let mut s = crate::settings::current();
                s.inlay_hints = false;
                crate::settings::apply(&s);
                eprintln!("smoke-intel: hints turned off, line 4 hints {:?}", ed.read(cx).inlay_labels(3));
                s.inlay_hints = true;
                crate::settings::apply(&s);
                eprintln!("smoke-intel: hints turned on, line 4 hints {:?}", ed.read(cx).inlay_labels(3));
                ed.update(cx, |e, _| e.buffer.undo());
            });
            if hold.as_deref() == Some("inlay") {
                eprintln!("smoke-intel: holding");
                return;
            }
            // Rename (Shift+F6): a local in place; a function through the popup with "Search in
            // comments and strings", Preview and Do Refactor, then one Ctrl+Z in either file;
            // a module name renames its file.
            const GREETER: &str = "def greet(name):\n    # greet the user\n    msg = \"hello \" + name\n    return msg\n\n\nGREETING = \"greet\"\n";
            const USES: &str = "import greeter\nfrom greeter import greet\n\nprint(greet(\"a\"), greeter.GREETING)\n";
            let _ = this.update_in(cx, |this, window, cx| {
                let _ = std::fs::write(this.root.join("greeter.py"), GREETER);
                let _ = std::fs::write(this.root.join("use_greet.py"), USES);
                this.reindex_files(cx);
                let f = this.root.join("greeter.py");
                this.open_file(&f, window, cx);
            });
            wait(&this, cx, &|this, _| this.files.as_ref().is_some_and(|f| f.files.iter().any(|f| f == "use_greet.py"))).await;
            pause(1000).await;
            let rename_popup = |this: &Self| this.popup.as_ref().is_some_and(|p| p.kind == crate::workspace::PopupKind::Rename);
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(ed) = this.active_editor().cloned() else { return };
                ed.update(cx, |e, cx| e.select_on_line(2, 5..5, cx));
                this.dispatch(brisk_keymap::Action::Rename, window, cx);
                eprintln!("smoke-intel: rename local -> {} carets, popup {}, {:?}", ed.read(cx).buffer.selections.len(), rename_popup(this), this.status);
                ed.update(cx, |e, cx| e.select_on_line(0, 5..5, cx));
                this.dispatch(brisk_keymap::Action::Rename, window, cx);
            });
            wait(&this, cx, &|this, _| rename_popup(this)).await;
            let labels = |this: &Self, q: &str| this.rename_items(q).iter().map(|i| format!("{} [{}]", i.label, i.detail)).collect::<Vec<_>>();
            let pick = |this: &mut Self, q: &str, prefix: &str, window: &mut Window, cx: &mut Context<Self>| {
                let t = this.rename_items(q).into_iter().find(|i| i.label.starts_with(prefix)).map(|i| i.target);
                this.popup = None;
                match t {
                    Some(Target::Intention(i)) => this.apply_intent(*i, window, cx),
                    Some(Target::Rename(n)) => this.apply_rename(n, window, cx),
                    _ => eprintln!("smoke-intel: rename FAIL: no item {prefix:?}"),
                }
            };
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-intel: rename popup query {:?} -> {:?}", this.popup.as_ref().map(|p| p.query.clone()), labels(this, "welcome"));
                pick(this, "welcome", "☐ Search", window, cx);
                eprintln!("smoke-intel: rename checkbox -> popup {} {:?}", rename_popup(this), labels(this, "welcome"));
                pick(this, "welcome", "Preview", window, cx);
            });
            wait(&this, cx, &|this, _| this.nav.pending.is_some()).await;
            if hold.as_deref() == Some("preview") {
                eprintln!("smoke-intel: holding");
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                for r in this.find_window_rows() {
                    eprintln!("smoke-intel: rename preview | {r}");
                }
                this.do_pending_refactor(window, cx);
                let t = text(this, cx);
                let u = std::fs::read_to_string(this.root.join("use_greet.py")).unwrap_or_default();
                eprintln!("smoke-intel: rename done -> {:?}", this.status);
                eprintln!("smoke-intel: rename greeter.py {:?}", t.lines().collect::<Vec<_>>());
                eprintln!("smoke-intel: rename use_greet.py {:?}", u.lines().collect::<Vec<_>>());
                this.dispatch(brisk_keymap::Action::Undo, window, cx);
                let u = std::fs::read_to_string(this.root.join("use_greet.py")).unwrap_or_default();
                eprintln!("smoke-intel: rename one undo {}: {:?}", pass(text(this, cx) == GREETER && u == USES), this.status);
                let f = this.root.join("use_greet.py");
                this.open_file(&f, window, cx);
            });
            pause(1500).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(ed) = this.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.select_on_line(0, 9..9, cx));
                }
                this.dispatch(brisk_keymap::Action::Rename, window, cx);
            });
            wait(&this, cx, &|this, _| rename_popup(this)).await;
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-intel: rename module -> {:?}", labels(this, "welcomer"));
                pick(this, "welcomer", "Rename module", window, cx);
            });
            wait(&this, cx, &|this, _| this.lsp.moved.is_none() && this.root.join("welcomer.py").exists()).await;
            pause(300).await;
            let _ = this.update(cx, |this, cx| {
                let u = text(this, cx);
                let ok = this.root.join("welcomer.py").exists() && !this.root.join("greeter.py").exists() && u.starts_with("import welcomer\nfrom welcomer import greet\n");
                eprintln!("smoke-intel: rename module file {}: {:?} {:?}", pass(ok), this.status, u.lines().collect::<Vec<_>>());
            });
            // Alt+Enter intentions: import from the project, Create function, type hints from
            // ty, f-string, split and merge 'if'.
            let _ = this.update_in(cx, |this, window, cx| {
                let _ = std::fs::write(this.root.join("shapes.py"), "class Square:\n    pass\n");
                let _ = std::fs::write(this.root.join("intents.py"), "def area(n: int):\n    if n < 0:\n        return None\n    squares = [n * n]\n    return squares\n\n\ns = Square()\nv = area(3)\ntotal = compute(v, 2)\nmsg = \"{} has {}\".format(s, v)\nif v and s:\n    print(msg, total)\n");
                this.reindex_files(cx);
                let f = this.root.join("intents.py");
                this.open_file(&f, window, cx);
            });
            wait(&this, cx, &|this, cx| this.active_editor().is_some_and(|e| e.read(cx).diagnostics.iter().filter(|d| d.code == "F821" || d.code == "unresolved-reference").count() >= 2)).await;
            pause(500).await;
            let steps: [(&str, usize, &str, &str); 7] = [
                ("Square()", 1, "Import '", "from "),
                ("compute(v", 2, "Create function 'compute'", "def compute"),
                ("def area", 5, "Specify return type", "def area"),
                ("v = area", 0, "Add type hint for variable 'v'", "v"),
                ("has {}", 2, "Convert to f-string literal", "msg ="),
                ("if v and", 1, "Split into 2 'if' statements", "if "),
                ("if v:", 1, "Merge nested 'if's", "if "),
            ];
            for (needle, off, label, show) in steps {
                // Let ruff and ty check the edited text first.
                pause(1500).await;
                let _ = this.update_in(cx, |this, window, cx| {
                    let Some(ed) = this.active_editor().cloned() else { return };
                    ed.update(cx, |e, _| {
                        let t = e.text_string();
                        let p = t.find(needle).map(|b| t[..b + off].chars().count()).unwrap_or(0);
                        e.buffer.set_caret(p, false);
                    });
                    this.problems.intents.clear();
                    this.status.clear();
                    this.show_intentions(window, cx);
                });
                wait(&this, cx, &|this, _| !this.problems.intents.is_empty() || this.status.starts_with("No context")).await;
                let _ = this.update_in(cx, |this, window, cx| {
                    let labels: Vec<String> = this.problems.intents.iter().map(|i| format!("{} [{}]", i.label, i.detail)).collect();
                    let pick = this.problems.intents.iter().find(|i| i.label.starts_with(label)).map(|i| i.target.clone());
                    this.popup = None;
                    let ok = pick.is_some();
                    if let Some(Target::Intention(i)) = pick {
                        this.apply_intent(*i, window, cx);
                    }
                    let t = text(this, cx);
                    let lines: Vec<&str> = t.lines().filter(|l| l.trim_start().starts_with(show)).collect();
                    eprintln!("smoke-intel: alt-enter {} {needle:?} -> {labels:?} => {lines:?}", pass(ok));
                });
            }
            let _ = this.update(cx, |this, cx| eprintln!("smoke-intel: intentions result {:?}", text(this, cx).lines().collect::<Vec<_>>()));
            // Parameter Info (current parameter in bold, overloads), Quick Documentation's
            // definition link, Type Info.
            let _ = this.update_in(cx, |this, window, cx| {
                let src = "from shapes import Square\n\n\ndef area(n: int, scale: float = 1.0) -> float:\n    \"\"\"Area of a square.\"\"\"\n    return n * n * scale\n\n\na = area(3, 2.0)\nb = max(1, 2)\nsq = Square()\nprint(a, b, sq)\n";
                let f = this.root.join("info.py");
                let _ = std::fs::write(&f, src);
                this.reindex_files(cx);
                this.open_file(&f, window, cx);
            });
            pause(2000).await;
            let caret = |this: &mut Self, needle: &str, off: usize, cx: &mut Context<Self>| {
                if let Some(ed) = this.active_editor().cloned() {
                    ed.update(cx, |e, _| {
                        e.close_insight();
                        let t = e.text_string();
                        let p = t.find(needle).map(|b| t[..b + off].chars().count()).unwrap_or(0);
                        e.buffer.set_caret(p, false);
                    });
                }
            };
            let insight = |this: &Self, cx: &gpui::App| this.active_editor().map(|e| e.read(cx).insight_for_smoke()).unwrap_or_default();
            for (needle, off) in [("area(3, 2", 8), ("max(1", 4)] {
                let _ = this.update_in(cx, |this, window, cx| {
                    caret(this, needle, off, cx);
                    this.dispatch(brisk_keymap::Action::ParameterInfo, window, cx);
                });
                wait(&this, cx, &|this, cx| !insight(this, cx).1.is_empty()).await;
                // The file's own `def` or the builtin shows at once; ty's answer replaces it.
                pause(1500).await;
                let _ = this.update(cx, |this, cx| eprintln!("smoke-intel: ctrl-p {needle:?} -> {:?}", insight(this, cx).1));
            }
            if hold.as_deref() == Some("sig") {
                eprintln!("smoke-intel: holding");
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                caret(this, "area(3", 1, cx);
                this.dispatch(brisk_keymap::Action::QuickDocumentation, window, cx);
            });
            wait(&this, cx, &|this, cx| insight(this, cx).0.is_some()).await;
            let _ = this.update(cx, |this, cx| {
                let (src, _) = insight(this, cx);
                let doc = this.active_editor().map(|e| e.read(cx).doc_shown_text().0.unwrap_or_default()).unwrap_or_default();
                eprintln!("smoke-intel: ctrl-q link {} {src:?}, doc {:?}", pass(src.as_deref() == Some("info:4")), doc.lines().collect::<Vec<_>>());
            });
            for (needle, off) in [("area(3", 1), ("Square()", 1), ("a = area", 0), ("b = max", 0)] {
                let _ = this.update_in(cx, |this, window, cx| {
                    caret(this, needle, off, cx);
                    this.status.clear();
                    this.dispatch(brisk_keymap::Action::TypeInfo, window, cx);
                });
                wait(&this, cx, &|this, _| !this.status.is_empty()).await;
                let _ = this.update(cx, |this, _| eprintln!("smoke-intel: ctrl-shift-p {needle:?} -> {:?}", this.status));
            }
            let _ = this.update(cx, |this, cx| {
                if let Some(ed) = this.active_editor().cloned() {
                    ed.update(cx, |e, _| e.close_insight());
                }
                eprintln!("smoke-intel: done in {:.1?}", t0.elapsed());
                cx.quit();
            });
        })
        .detach();
    }
}
