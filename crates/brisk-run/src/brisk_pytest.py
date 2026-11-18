"""Brisk test runner plugin: reports pytest progress as JSON lines to $BRISK_TEST_EVENTS.

Loaded through PYTEST_PLUGINS, so the command line stays a plain `python -m pytest ...`.
Only reporting hooks are used; collection and execution are pytest's own.
"""
import json
import os

_out = None
_reports = {}


def _emit(**ev):
    global _out
    if _out is None:
        path = os.environ.get("BRISK_TEST_EVENTS")
        if not path:
            return
        _out = open(path, "a", encoding="utf-8")
    _out.write(json.dumps(ev, ensure_ascii=False) + "\n")
    _out.flush()


def _file_of(item):
    p = getattr(item, "path", None) or getattr(item, "fspath", None)
    return str(p) if p is not None else ""


def pytest_sessionstart(session):
    root = getattr(session.config, "rootpath", None) or session.config.rootdir
    _emit(event="session", rootdir=str(root))


def pytest_collection_finish(session):
    tests = []
    for item in session.items:
        line = None
        try:
            if item.location[1] is not None:
                line = item.location[1] + 1
        except Exception:
            pass
        tests.append({"id": item.nodeid, "file": _file_of(item), "line": line})
    _emit(event="collected", tests=tests)


def pytest_collection_modifyitems(session, config, items):
    # "Rerun Failed Tests First": the IDE lists the last run's failures in this file.
    path = os.environ.get("BRISK_TESTS_FIRST")
    if not path:
        return
    try:
        with open(path, encoding="utf-8") as f:
            first = set(l for l in f.read().split("\n") if l)
    except OSError:
        return
    items[:] = [i for i in items if i.nodeid in first] + [i for i in items if i.nodeid not in first]


def pytest_collectreport(report):
    if report.failed:
        _emit(event="collecterror", id=report.nodeid, message=report.longreprtext)


def pytest_runtest_logstart(nodeid, location):
    _reports[nodeid] = []
    _emit(event="start", id=nodeid)


def pytest_runtest_logreport(report):
    _reports.setdefault(report.nodeid, []).append(report)


def pytest_runtest_logfinish(nodeid, location):
    reps = _reports.pop(nodeid, [])
    outcome = "passed"
    message = []
    duration = 0.0
    for r in reps:
        duration += getattr(r, "duration", 0.0) or 0.0
        if r.failed:
            if r.when == "call":
                outcome = "failed"
            elif outcome != "failed":
                outcome = "error"
            message.append(r.longreprtext)
        elif r.skipped:
            if outcome == "passed":
                outcome = "skipped"
            if hasattr(r, "wasxfail"):
                message.append("XFAIL " + (r.wasxfail or ""))
            elif isinstance(r.longrepr, tuple) and len(r.longrepr) == 3:
                message.append(str(r.longrepr[2]))
            else:
                message.append(r.longreprtext)
    # Each phase report repeats the captures of the earlier phases: take the last one.
    output = ""
    if reps:
        output = "".join("----- %s -----\n%s\n" % (name, text) for name, text in reps[-1].sections if text)
    _emit(event="finish", id=nodeid, outcome=outcome, duration=duration, message="\n".join(m for m in message if m), output=output)


def pytest_sessionfinish(session, exitstatus):
    _emit(event="done", exitstatus=int(exitstatus))
