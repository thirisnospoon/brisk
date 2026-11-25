//! Smoke of the web framework support on a small Django project: Ctrl+B on a template name
//! in a view, Ctrl+Click on `{% extends %}` and `{% url %}` in templates, Go to Related from
//! a template to its view, and the Endpoints window.

use super::Workspace;
use crate::editor::EditorEvent;
use brisk_keymap::Action;
use gpui::{Context, Window};

const FILES: &[(&str, &str)] = &[
    ("manage.py", "import os\nos.environ.setdefault('DJANGO_SETTINGS_MODULE', 'mysite.settings')\n# django\n"),
    ("mysite/__init__.py", ""),
    ("mysite/settings.py", "ROOT_URLCONF = \"mysite.urls\"\n"),
    ("mysite/urls.py", "from django.urls import include, path\n\nurlpatterns = [\n    path(\"polls/\", include(\"polls.urls\")),\n]\n"),
    ("polls/__init__.py", ""),
    ("polls/urls.py", "from django.urls import path\n\nfrom . import views\n\napp_name = \"polls\"\nurlpatterns = [\n    path(\"\", views.index, name=\"index\"),\n    path(\"<int:pk>/\", views.detail, name=\"detail\"),\n]\n"),
    (
        "polls/views.py",
        "from django.shortcuts import render\n\n\ndef index(request):\n    return render(request, \"polls/index.html\")\n\n\ndef detail(request, pk):\n    return render(request, \"polls/detail.html\", {\"pk\": pk})\n",
    ),
    ("polls/templates/polls/index.html", "{% extends \"base.html\" %}\n{% block content %}\n<a href=\"{% url 'polls:detail' 1 %}\">{{ question.text|upper }}</a>\n{% endblock %}\n"),
    ("polls/templates/polls/detail.html", "{% extends 'base.html' %}\n{# the detail page #}\n"),
    ("templates/base.html", "<html><body>{% block content %}{% endblock %}</body></html>\n"),
];

impl Workspace {
    /// BRISK_SMOKE_WEBNAV=1|hold on an empty folder.
    pub(super) fn webnav_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (rel, text) in FILES {
            let p = self.root.join(rel);
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let _ = std::fs::write(p, text);
        }
        self.web.stale = true;
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            // Where the active editor is: "file:line:col" (1-based line).
            let here = |this: &Workspace, cx: &gpui::App| {
                this.active_editor().map(|e| {
                    let e = e.read(cx);
                    let (l, c) = e.caret_line_col();
                    format!("{}:{}:{}", e.title(), l + 1, c + 1)
                })
            };
            // Opens a file and puts the caret inside `needle`; returns its char position.
            let open_at = |this: &mut Workspace, rel: &str, needle: &str, window: &mut Window, cx: &mut Context<Workspace>| -> Option<usize> {
                let p = this.root.join(rel);
                this.open_file(&p, window, cx);
                let ed = this.active_editor()?.clone();
                ed.update(cx, |e, cx| {
                    let text = e.text_string();
                    let byte = text.find(needle)? + 1;
                    let before = &text[..byte];
                    let line = before.matches('\n').count();
                    let col = before[before.rfind('\n').map(|i| i + 1).unwrap_or(0)..].chars().count();
                    e.select_on_line(line, col..col, cx);
                    Some(before.chars().count())
                })
            };
            pause(800).await;
            let _ = this.update_in(cx, |this, window, cx| {
                open_at(this, "polls/views.py", "\"polls/detail.html", window, cx);
                this.dispatch(Action::GotoDeclaration, window, cx);
            });
            pause(1500).await;
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-web: Ctrl+B on the template name -> {:?}", here(this, cx));
                // Ctrl+Click on {% extends 'base.html' %} in the template just opened.
                if let Some(pos) = open_at(this, "polls/templates/polls/detail.html", "'base.html", window, cx) {
                    let ed = this.active_editor().unwrap().clone();
                    ed.update(cx, |_, cx| cx.emit(EditorEvent::CtrlClick(pos)));
                }
            });
            pause(600).await;
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-web: Ctrl+Click on extends -> {:?}", here(this, cx));
                if let Some(pos) = open_at(this, "polls/templates/polls/index.html", "'polls:detail", window, cx) {
                    let ed = this.active_editor().unwrap().clone();
                    ed.update(cx, |_, cx| cx.emit(EditorEvent::CtrlClick(pos)));
                }
            });
            pause(600).await;
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-web: Ctrl+Click on {{% url 'polls:detail' %}} -> {:?}", here(this, cx));
                open_at(this, "polls/templates/polls/detail.html", "{#", window, cx);
                this.dispatch(Action::GotoRelated, window, cx);
            });
            pause(600).await;
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-web: Go to Related from detail.html -> {:?}", here(this, cx));
                // From the view back to its template.
                open_at(this, "polls/views.py", "{\"pk\"", window, cx);
                this.dispatch(Action::GotoRelated, window, cx);
            });
            pause(600).await;
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-web: Go to Related from the view -> {:?}", here(this, cx));
                this.dispatch(Action::ToggleEndpointsWindow, window, cx);
            });
            pause(800).await;
            let _ = this.update_in(cx, |this, window, cx| {
                let routes: Vec<String> = this
                    .web
                    .index
                    .as_ref()
                    .map(|i| i.routes.iter().map(|r| format!("{} {} {}", r.path, r.name.clone().unwrap_or_default(), r.handler)).collect())
                    .unwrap_or_default();
                eprintln!("smoke-web: endpoints -> {routes:?}");
                if let Some(e) = this.endpoints.as_mut() {
                    e.selected = 1;
                    let f = e.focus.clone();
                    window.focus(&f);
                }
                this.smoke_press("enter", window, cx);
                eprintln!("smoke-web: Enter on the second endpoint -> {:?}", here(this, cx));
                if std::env::var("BRISK_SMOKE_WEBNAV").as_deref() != Ok("hold") {
                    cx.quit();
                }
            });
        })
        .detach();
    }
}
