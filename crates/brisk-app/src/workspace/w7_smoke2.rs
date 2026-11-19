//! Second half of `BRISK_SMOKE_W7`: scratch files and New › Python Package / Python File with a
//! template (E4), Python typing (E8), places: Back, Recent Locations, Last Edit Location and
//! Go to Line:Column (E9), paste with the indent of where it goes (E10), the right margin (E11).
//! The place steps run one update each: places are taken when the editor's notify is handled.

use super::files_ui::{FileOp, NewKind};
use super::Workspace;
use brisk_keymap::Action;
use brisk_text::Selection;
use gpui::{Context, Window};
use std::time::Duration;

impl Workspace {
    pub(super) fn w7_smoke2(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // --- E4 ---------------------------------------------------------------------------
        self.dispatch(Action::NewScratchFile, window, cx);
        eprintln!("smoke-w7: new scratch popup {}", self.smoke_popup());
        self.smoke_query("pyth");
        self.smoke_press("enter", window, cx);
        let scratch = self.active_editor().and_then(|e| e.read(cx).path.clone());
        let rows: Vec<String> = self.tree.rows().to_vec().iter().map(|r| format!("{}{}", "  ".repeat(r.depth), self.tree.nodes[r.id].name)).collect();
        eprintln!("smoke-w7: scratch {:?} exists={} | tree {rows:?}", scratch.as_ref().and_then(|p| p.file_name()), scratch.as_ref().is_some_and(|p| p.is_file()));
        let root = self.root.clone();
        self.start_file_op(FileOp::New(root.clone(), NewKind::Package), cx);
        self.smoke_query("pkg.sub");
        eprintln!("smoke-w7: new package popup {}", self.smoke_popup());
        self.smoke_press("enter", window, cx);
        eprintln!("smoke-w7: package -> pkg/__init__.py={} pkg/sub/__init__.py={}", root.join("pkg/__init__.py").is_file(), root.join("pkg/sub/__init__.py").is_file());
        let tpl = brisk_db::config_dir().join("fileTemplates");
        let _ = std::fs::create_dir_all(&tpl).and_then(|_| std::fs::write(tpl.join("Python Script.py"), "# ${NAME} in ${PROJECT_NAME}\n"));
        self.start_file_op(FileOp::New(root.join("pkg"), NewKind::Python), cx);
        self.smoke_query("mod1");
        self.smoke_press("enter", window, cx);
        eprintln!("smoke-w7: python file from template -> {:?}", std::fs::read_to_string(root.join("pkg/mod1.py")).ok());
        let _ = std::fs::remove_file(tpl.join("Python Script.py"));

        // --- E8 typing (through the platform input path) --------------------------------------
        let a = root.join("a.py");
        self.open_file(&a, window, cx);
        let Some(ed) = self.active_editor().cloned() else { return cx.quit() };
        let show = |ed: &gpui::Entity<crate::editor::Editor>, cx: &gpui::App| {
            let b = &ed.read(cx).buffer;
            let mut t = b.text();
            let at = t.char_indices().nth(b.primary().head).map_or(t.len(), |(i, _)| i);
            t.insert(at, '|');
            t
        };
        ed.update(cx, |e, _| {
            e.buffer.set_text("def f():\n    ");
            e.buffer.selections = vec![Selection::caret(13)];
        });
        for c in ["\"", "\"", "\""] {
            self.smoke_type(c, window, cx);
        }
        eprintln!("smoke-w7: typed three quotes {:?}", show(&ed, cx));
        self.smoke_press("enter", window, cx);
        eprintln!("smoke-w7: docstring {:?}", show(&ed, cx));
        ed.update(cx, |e, _| {
            e.buffer.set_text("if a:\n    x = 1\n    ");
            e.buffer.selections = vec![Selection::caret(20)];
        });
        for c in ["e", "l", "s", "e", ":"] {
            self.smoke_type(c, window, cx);
        }
        eprintln!("smoke-w7: else: dedent {:?}", show(&ed, cx));
        ed.update(cx, |e, _| {
            e.buffer.set_text("def f():\n    return g(a,");
            e.buffer.selections = vec![Selection::caret(24)];
        });
        self.smoke_press("enter", window, cx);
        eprintln!("smoke-w7: enter in call {:?}", show(&ed, cx));
        ed.update(cx, |e, _| {
            e.buffer.set_text("s = 'abcdef'\n");
            e.buffer.selections = vec![Selection::caret(8)];
        });
        self.smoke_press("enter", window, cx);
        eprintln!("smoke-w7: enter in string {:?}", show(&ed, cx));

        // --- E10 paste ----------------------------------------------------------------------
        ed.update(cx, |e, _| {
            e.buffer.set_text("class A:\n    \n");
            e.buffer.selections = vec![Selection::caret(13)];
        });
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("def m(self):\n        return 1".into()));
        self.smoke_press("ctrl-v", window, cx);
        eprintln!("smoke-w7: paste into class {:?}", show(&ed, cx));
        let margin = ed.read(cx).right_margin;
        eprintln!("smoke-w7: right margin {margin:?}");

        // --- E9 places, one update per step ---------------------------------------------------
        let text: String = (0..80).map(|i| format!("line_{i} = {i}\n")).collect();
        ed.update(cx, |e, _| {
            e.buffer.set_text(&text);
            e.buffer.selections = vec![Selection::caret(0)];
        });
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let tick = || bg.timer(Duration::from_millis(30));
            let line = |this: &gpui::WeakEntity<Self>, cx: &mut gpui::AsyncWindowContext| {
                this.update(cx, |this, cx| this.active_editor().map(|e| e.read(cx).caret_line_col())).ok().flatten()
            };
            let goto = |this: &gpui::WeakEntity<Self>, cx: &mut gpui::AsyncWindowContext, l: usize| {
                let _ = this.update(cx, |this, cx| {
                    if let Some(ed) = this.active_editor().cloned() {
                        ed.update(cx, |e, cx| e.select_on_line(l, 0..0, cx));
                    }
                });
            };
            tick().await;
            goto(&this, cx, 40);
            tick().await;
            // An edit at line 40.
            let _ = this.update_in(cx, |this, window, cx| this.smoke_type("x", window, cx));
            tick().await;
            goto(&this, cx, 70);
            tick().await;
            let _ = this.update_in(cx, |this, window, cx| this.dispatch(Action::NavigateBack, window, cx));
            tick().await;
            eprintln!("smoke-w7: back from 71 -> {:?}", line(&this, cx));
            let _ = this.update_in(cx, |this, window, cx| this.dispatch(Action::NavigateForward, window, cx));
            tick().await;
            eprintln!("smoke-w7: forward -> {:?}", line(&this, cx));
            goto(&this, cx, 5);
            tick().await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.dispatch(Action::RecentLocations, window, cx);
                eprintln!("smoke-w7: recent locations {}", this.smoke_popup());
                let items: Vec<String> = this.popup.as_ref().map(|p| p.items.iter().map(|i| i.detail.clone()).collect()).unwrap_or_default();
                eprintln!("smoke-w7: recent locations code {items:?}");
                this.dispatch(Action::RecentLocations, window, cx);
                eprintln!("smoke-w7: changed only {}", this.smoke_popup());
                this.smoke_press("escape", window, cx);
                this.dispatch(Action::LastEditLocation, window, cx);
            });
            tick().await;
            eprintln!("smoke-w7: last edit location -> {:?}", line(&this, cx));
            let _ = this.update_in(cx, |this, window, cx| {
                this.dispatch(Action::GotoLine, window, cx);
                this.smoke_query("3:5");
                eprintln!("smoke-w7: go to line popup {}", this.smoke_popup());
                this.smoke_press("enter", window, cx);
            });
            tick().await;
            eprintln!("smoke-w7: go to 3:5 -> {:?}", line(&this, cx));

            // --- E11 soft wraps: a .md file wraps ("Soft-wrap these files"), a .py one not ---
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(ed) = this.active_editor().cloned() {
                    ed.update(cx, |e, _| e.buffer.mark_saved());
                    eprintln!("smoke-w7: a.py soft wrap {}", ed.read(cx).soft_wrap());
                }
                let word = "lorem ipsum dolor sit amet consectetur ";
                let text: String = (0..60).map(|i| format!("{i}. {}\n", word.repeat(8))).collect();
                let md = this.root.join("notes.md");
                let _ = std::fs::write(&md, text);
                this.open_file(&md, window, cx);
            });
            tick().await;
            tick().await;
            let probe = |this: &gpui::WeakEntity<Self>, cx: &mut gpui::AsyncWindowContext| this.update(cx, |this, cx| this.active_editor().map(|e| e.read(cx).wrap_smoke())).ok().flatten().unwrap_or_default();
            eprintln!("smoke-w7: wrap md {}", probe(&this, cx));
            for k in ["end", "end", "home", "home", "down", "down", "up"] {
                let _ = this.update_in(cx, |this, window, cx| this.smoke_press(k, window, cx));
                tick().await;
                eprintln!("smoke-w7: wrap {k} -> {:?}", line(&this, cx));
            }
            let _ = this.update(cx, |this, cx| {
                if let Some(e) = this.active_editor() {
                    let e = e.read(cx);
                    eprintln!("smoke-w7: wrap click row1 col4 -> {:?} row2 col0 -> {:?} row3 col4 -> {:?}", e.wrap_click(1, 4.0), e.wrap_click(2, 0.0), e.wrap_click(3, 4.0));
                }
            });
            let _ = this.update_in(cx, |this, window, cx| this.smoke_press("ctrl-end", window, cx));
            tick().await;
            eprintln!("smoke-w7: wrap ctrl-end {}", probe(&this, cx));
            let _ = this.update_in(cx, |this, window, cx| {
                this.smoke_press("ctrl-home", window, cx);
                this.smoke_press("shift-down", window, cx);
            });
            tick().await;
            let _ = this.update(cx, |this, cx| {
                if let Some(e) = this.active_editor() {
                    let b = &e.read(cx).buffer;
                    eprintln!("smoke-w7: wrap shift-down selects {:?}", b.primary().range());
                }
            });
            let _ = this.update_in(cx, |this, window, cx| this.dispatch(Action::ToggleSoftWrap, window, cx));
            tick().await;
            eprintln!("smoke-w7: wrap toggled {}", probe(&this, cx));
            // --- E12 mouse: double / triple click and drag, Alt+Shift+click twice, Alt+drag ----
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(ed) = this.active_editor().cloned() else { return };
                let none = gpui::Modifiers::default();
                let sel = |ed: &gpui::Entity<crate::editor::Editor>, cx: &gpui::App| {
                    let b = &ed.read(cx).buffer;
                    b.selections.iter().map(|s| b.slice(s.range())).collect::<Vec<_>>()
                };
                ed.update(cx, |e, cx| e.smoke_mouse((1, 5), none, 2, None, window, cx));
                eprintln!("smoke-w7: mouse double click {:?}", sel(&ed, cx));
                ed.update(cx, |e, cx| e.smoke_mouse((1, 5), none, 2, Some((1, 16)), window, cx));
                eprintln!("smoke-w7: mouse double click + drag {:?}", sel(&ed, cx));
                ed.update(cx, |e, cx| e.smoke_mouse((2, 4), none, 3, Some((3, 1)), window, cx));
                eprintln!("smoke-w7: mouse triple click + drag lines {:?}", ed.read(cx).buffer.selections.iter().map(|s| { let b = &ed.read(cx).buffer; (b.pos_to_line_col(s.range().start), b.pos_to_line_col(s.range().end)) }).collect::<Vec<_>>());
                ed.update(cx, |e, cx| e.smoke_mouse((5, 0), none, 1, None, window, cx));
                let alt_shift = gpui::Modifiers { alt: true, shift: true, ..Default::default() };
                ed.update(cx, |e, cx| e.smoke_mouse((5, 4), alt_shift, 1, None, window, cx));
                let n1 = ed.read(cx).buffer.selections.len();
                ed.update(cx, |e, cx| e.smoke_mouse((5, 4), alt_shift, 1, None, window, cx));
                eprintln!("smoke-w7: mouse alt+shift click carets {n1} -> again {}", ed.read(cx).buffer.selections.len());
                let alt = gpui::Modifiers { alt: true, ..Default::default() };
                ed.update(cx, |e, cx| e.smoke_mouse((5, 3), alt, 1, Some((7, 8)), window, cx));
                eprintln!("smoke-w7: mouse alt+drag block {:?}", sel(&ed, cx));
                ed.update(cx, |e, cx| e.smoke_mouse((0, 0), none, 1, None, window, cx));
            });
            if std::env::var_os("BRISK_SMOKE_W7_HOLD").is_some() {
                // For a look: wraps back on and a selection across rows.
                let _ = this.update_in(cx, |this, window, cx| {
                    this.dispatch(Action::ToggleSoftWrap, window, cx);
                    if let Some(ed) = this.active_editor().cloned() {
                        ed.update(cx, |e, _| e.buffer.selections = vec![Selection { anchor: 100, head: 400, goal_col: None }]);
                    }
                });
            }
            let _ = this.update_in(cx, |_, _, cx| {
                eprintln!("smoke-w7: done");
                if std::env::var_os("BRISK_SMOKE_W7_HOLD").is_none() {
                    cx.quit();
                }
            });
        })
        .detach();
    }
}
