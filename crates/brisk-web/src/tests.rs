use super::*;
use std::path::PathBuf;

fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("brisk-web-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (rel, text) in files {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    root
}

fn rel(root: &Path, l: &Loc) -> String {
    format!("{}:{}", l.path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"), l.line + 1)
}

const DJANGO: &[(&str, &str)] = &[
    ("src/manage.py", "import os\nos.environ.setdefault('DJANGO_SETTINGS_MODULE', 'mysite.settings')\n"),
    ("src/mysite/__init__.py", ""),
    ("src/mysite/settings.py", "ROOT_URLCONF = \"mysite.urls\"\nTEMPLATES = [{\"DIRS\": [BASE_DIR / \"templates\"]}]\n"),
    (
        "src/mysite/urls.py",
        "from django.contrib import admin\nfrom django.urls import include, path\n\nurlpatterns = [\n    path(\"polls/\", include(\"polls.urls\")),\n    path(\"admin/\", admin.site.urls),\n]\n",
    ),
    ("src/polls/__init__.py", ""),
    (
        "src/polls/urls.py",
        "from django.urls import path, re_path\n\nfrom . import views\n\napp_name = \"polls\"\nurlpatterns = [\n    path(\"\", views.IndexView.as_view(), name=\"index\"),\n    path(\"<int:pk>/\", views.detail, name=\"detail\"),\n    re_path(r\"^archive/(?P<year>[0-9]{4})/$\", views.archive, name=\"archive\"),\n]\n",
    ),
    (
        "src/polls/views.py",
        "from django.shortcuts import render\nfrom django.views import generic\n\n\nclass IndexView(generic.ListView):\n    template_name = \"polls/index.html\"\n\n\ndef detail(request, pk):\n    return render(request, \"polls/detail.html\", {\"pk\": pk})\n\n\ndef archive(request, year):\n    return redirect(\"polls:index\")\n",
    ),
    ("src/polls/templates/polls/index.html", "{% extends \"base.html\" %}\n{% block content %}<a href=\"{% url 'polls:detail' q.id %}\">x</a>{% endblock %}\n"),
    ("src/polls/templates/polls/detail.html", "{% extends 'base.html' %}\n{% load static %}<link href=\"{% static 'polls/style.css' %}\">\n"),
    ("src/templates/base.html", "<html>{% block content %}{% endblock %}</html>\n"),
    ("src/polls/static/polls/style.css", "a {}\n"),
];

#[test]
fn django_routes_follow_include_and_namespace() {
    let root = project("django", DJANGO);
    let idx = Index::scan(&root);
    let routes: Vec<String> = idx.routes.iter().map(|r| format!("{} {} {}", r.path, r.name.clone().unwrap_or_default(), r.handler)).collect();
    assert_eq!(
        routes,
        [
            "/admin/  admin.site.urls",
            "/polls/ polls:index views.IndexView.as_view()",
            "/polls/<int:pk>/ polls:detail views.detail",
            "/polls/archive/(?P<year>[0-9]{4})/ polls:archive views.archive",
        ]
    );
    let detail = &idx.routes[2];
    assert_eq!(rel(&root, &detail.at), "src/polls/urls.py:8");
    assert_eq!(rel(&root, detail.handler_at.as_ref().unwrap()), "src/polls/views.py:9");
    assert_eq!(rel(&root, idx.routes[1].handler_at.as_ref().unwrap()), "src/polls/views.py:5");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn scan_of_a_file_list_is_the_same() {
    let mut files = DJANGO.to_vec();
    // A file that is not UTF-8 is skipped, as before.
    files.push(("api/app.py", "from fastapi import FastAPI\napp = FastAPI()\n\n@app.get(\"/items\")\ndef items(): ...\n"));
    let root = project("filelist", &files);
    std::fs::write(root.join("api/latin1.py"), b"# caf\xe9\nfrom flask import Flask\napp = Flask(__name__)\n@app.route('/x')\ndef x(): ...\n").unwrap();
    let a = Index::scan(&root);
    let b = Index::scan_files(&brisk_project::FileList::scan(&root));
    let show = |i: &Index| i.routes.iter().map(|r| format!("{:?} {} {}", r.framework, r.path, r.handler)).collect::<Vec<_>>();
    assert_eq!(show(&a), show(&b));
    assert!(show(&a).iter().any(|r| r.contains("/items")), "{:?}", show(&a));
    assert!(!show(&a).iter().any(|r| r.contains("/x")));
    assert_eq!(a.template_files, b.template_files);
    assert_eq!(a.py_files, b.py_files);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn django_include_under_two_prefixes_and_cycles() {
    let files: &[(&str, &str)] = &[
        ("mysite/settings.py", "ROOT_URLCONF = \"mysite.urls\"\n"),
        ("mysite/__init__.py", ""),
        ("mysite/urls.py", "from django.urls import include, path\n\nurlpatterns = [\n    path(\"api/\", include(\"api.urls\")),\n    path(\"v2/api/\", include(\"api.urls\")),\n]\n"),
        ("api/__init__.py", ""),
        // Includes itself: the walk stops instead of recursing.
        ("api/urls.py", "from django.urls import include, path\n\nfrom . import views\n\nurlpatterns = [\n    path(\"items/\", views.items),\n    path(\"again/\", include(\"api.urls\")),\n]\n"),
        ("api/views.py", "def items(request):\n    pass\n"),
    ];
    let root = project("django-twice", files);
    let idx = Index::scan(&root);
    let paths: Vec<&str> = idx.routes.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(paths, ["/api/items/", "/v2/api/items/"]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn django_references_resolve() {
    let root = project("django-refs", DJANGO);
    let idx = Index::scan(&root);
    let views = root.join("src/polls/views.py");
    let to = |r: Ref| idx.resolve(&views, &r).iter().map(|l| rel(&root, l)).collect::<Vec<_>>();
    assert_eq!(to(Ref::Template("polls/detail.html".into())), ["src/polls/templates/polls/detail.html:1"]);
    assert_eq!(to(Ref::Template("base.html".into())), ["src/templates/base.html:1"]);
    assert_eq!(to(Ref::Url("polls:detail".into())), ["src/polls/urls.py:8"]);
    // Without the namespace, as inside the app.
    assert_eq!(to(Ref::Url("detail".into())), ["src/polls/urls.py:8"]);
    assert_eq!(to(Ref::Module("polls.urls".into())), ["src/polls/urls.py:1"]);
    assert_eq!(to(Ref::Static("polls/style.css".into())), ["src/polls/static/polls/style.css:1"]);
    // Template → views and templates that use it.
    let base = idx.template_usages(&root.join("src/templates/base.html"));
    assert_eq!(base.iter().map(|l| rel(&root, l)).collect::<Vec<_>>(), ["src/polls/templates/polls/detail.html:1", "src/polls/templates/polls/index.html:1"]);
    let detail = idx.template_usages(&root.join("src/polls/templates/polls/detail.html"));
    assert_eq!(detail.iter().map(|l| rel(&root, l)).collect::<Vec<_>>(), ["src/polls/views.py:10"]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn fastapi_routes_with_router_prefixes() {
    let root = project(
        "fastapi",
        &[
            (
                "app/main.py",
                "from fastapi import FastAPI\nfrom fastapi.templating import Jinja2Templates\n\nfrom .routers import items, users\n\napp = FastAPI()\napp.include_router(items.router, prefix=\"/api\")\napp.include_router(users.router)\ntemplates = Jinja2Templates(directory=\"pages\")\n\n\n@app.get(\"/\")\nasync def root():\n    return {}\n",
            ),
            ("app/__init__.py", ""),
            ("app/routers/__init__.py", ""),
            (
                "app/routers/items.py",
                "from fastapi import APIRouter\n\nrouter = APIRouter(prefix=\"/items\", tags=[\"items\"])\n\n\n@router.get(\"/{item_id}\", name=\"item\")\nasync def read_item(item_id: int):\n    ...\n\n\n@router.api_route(\"/\", methods=[\"get\", \"post\"])\ndef items():\n    ...\n",
            ),
            ("app/routers/users.py", "from fastapi import APIRouter\n\nrouter = APIRouter()\n\n@router.websocket(\"/ws\")\nasync def ws(socket):\n    ...\n"),
            ("pages/index.html", "<p>{{ url_for('item', item_id=1) }}</p>\n"),
        ],
    );
    let idx = Index::scan(&root);
    let routes: Vec<String> = idx.routes.iter().map(|r| format!("{} {} {}", r.methods.join(","), r.path, r.name.clone().unwrap_or_default())).collect();
    assert_eq!(routes, ["GET / root", "GET,POST /api/items/ items", "GET /api/items/{item_id} item", "WS /ws ws"]);
    assert_eq!(rel(&root, idx.routes[2].handler_at.as_ref().unwrap()), "app/routers/items.py:7");
    assert_eq!(idx.template_dirs, [root.join("pages")]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn flask_blueprints() {
    let root = project(
        "flask",
        &[
            ("app.py", "from flask import Flask\nfrom auth import bp\n\napp = Flask(__name__)\napp.register_blueprint(bp)\n\n\n@app.route(\"/\")\ndef index():\n    return render_template(\"index.html\")\n"),
            ("auth.py", "from flask import Blueprint\n\nbp = Blueprint(\"auth\", __name__, url_prefix=\"/auth\")\n\n\n@bp.route(\"/login\", methods=[\"GET\", \"POST\"])\ndef login():\n    ...\n"),
            ("templates/index.html", "<a href=\"{{ url_for('auth.login') }}\">"),
        ],
    );
    let idx = Index::scan(&root);
    let routes: Vec<String> = idx.routes.iter().map(|r| format!("{} {} {}", r.methods.join(","), r.path, r.name.clone().unwrap_or_default())).collect();
    assert_eq!(routes, ["GET / index", "GET,POST /auth/login auth.login"]);
    let app = root.join("app.py");
    assert_eq!(idx.resolve(&app, &Ref::Url("auth.login".into())).len(), 1);
    assert_eq!(idx.resolve(&app, &Ref::Url("login".into())).len(), 1);
    let _ = std::fs::remove_dir_all(&root);
}

fn at(src: &str, marker: &str) -> usize {
    src.find(marker).unwrap() + 1
}

#[test]
fn python_references_at_the_caret() {
    let src = "def detail(request):\n    x = reverse(\"polls:detail\", args=[1])\n    y = render(request, \"polls/detail.html\")\n    z = redirect(\"polls:index\")\n    w = redirect(\"/login/\")\n    u = include(\"polls.urls\")\n    s = url_for(\"static\", filename=\"css/a.css\")\n    plain = \"hello\"\n";
    assert_eq!(python_ref(src, at(src, "\"polls:detail")), Some(Ref::Url("polls:detail".into())));
    assert_eq!(python_ref(src, at(src, "\"polls/detail.html")), Some(Ref::Template("polls/detail.html".into())));
    assert_eq!(python_ref(src, at(src, "\"polls:index")), Some(Ref::Url("polls:index".into())));
    assert_eq!(python_ref(src, at(src, "\"/login/")), None);
    assert_eq!(python_ref(src, at(src, "\"polls.urls")), Some(Ref::Module("polls.urls".into())));
    assert_eq!(python_ref(src, at(src, "\"css/a.css")), Some(Ref::Static("css/a.css".into())));
    assert_eq!(python_ref(src, at(src, "\"hello")), None);
    assert_eq!(python_ref(src, at(src, "request):")), None);
    assert_eq!(templates_in_scope(src, at(src, "plain")), ["polls/detail.html"]);
}

#[test]
fn template_references_at_the_caret() {
    let src = "{% extends \"base.html\" %}\n{% include 'parts/nav.html' with x=1 %}\n<a href=\"{% url 'polls:detail' q.id %}\">{{ q }}</a>\n{% static 'css/a.css' %}\n{{ url_for('index') }} {{ url_for('static', filename='b.css') }}\n<p>'not/a.html'</p>\n";
    assert_eq!(template_ref(src, at(src, "\"base")), Some(Ref::Template("base.html".into())));
    assert_eq!(template_ref(src, at(src, "'parts")), Some(Ref::Template("parts/nav.html".into())));
    assert_eq!(template_ref(src, at(src, "'polls:detail")), Some(Ref::Url("polls:detail".into())));
    assert_eq!(template_ref(src, at(src, "'css/a")), Some(Ref::Static("css/a.css".into())));
    assert_eq!(template_ref(src, at(src, "'index")), Some(Ref::Url("index".into())));
    assert_eq!(template_ref(src, at(src, "'b.css")), Some(Ref::Static("b.css".into())));
    assert_eq!(template_ref(src, at(src, "'not/a")), None);
    assert_eq!(template_ref(src, at(src, "q }}")), None);
}
