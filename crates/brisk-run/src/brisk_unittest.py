"""Brisk test runner for unittest and doctest: the same JSON events as the pytest plugin
(written to $BRISK_TEST_EVENTS), with unittest's usual console output.

`python -m brisk_unittest <unittest arguments>` stands in for `python -m unittest ...`;
`python -m brisk_unittest --doctest [-v] [-o FLAG] file.py[::name] ...` for
`python -m doctest ...`. The IDE swaps the module only for the process it starts.
"""
import doctest
import importlib
import importlib.util
import inspect
import json
import os
import sys
import time
import unittest

_out = None


def _emit(**ev):
    global _out
    if _out is None:
        path = os.environ.get("BRISK_TEST_EVENTS")
        if not path:
            return
        _out = open(path, "a", encoding="utf-8")
    _out.write(json.dumps(ev, ensure_ascii=False) + "\n")
    _out.flush()


def _rel(path):
    try:
        return os.path.relpath(path, os.getcwd()).replace("\\", "/")
    except ValueError:
        return path.replace("\\", "/")


def _flat(suite):
    for t in suite:
        if isinstance(t, unittest.TestSuite):
            yield from _flat(t)
        else:
            yield t


def _failed_import(test):
    return type(test).__name__ == "_FailedTest" or type(test).__module__ == "unittest.loader"


def _info(test):
    """Node id (`rel/path.py::Class::method`, as pytest's), file and 1-based line."""
    if isinstance(test, doctest.DocTestCase):
        dt = test._dt_test
        f = dt.filename or ""
        line = dt.lineno + 1 if dt.lineno is not None else None
        return "%s::%s" % (_rel(f) if f else dt.name, dt.name), f, line
    if _failed_import(test):
        name = test.id().rsplit(".", 1)[-1]
        return name.replace(".", "/") + ".py", "", None
    cls = type(test)
    mod = sys.modules.get(cls.__module__)
    f = getattr(mod, "__file__", "") or ""
    name = getattr(test, "_testMethodName", "") or test.id().rsplit(".", 1)[-1]
    line = None
    try:
        line = inspect.getsourcelines(getattr(cls, name))[1]
    except Exception:
        pass
    return "%s::%s::%s" % (_rel(f) if f else cls.__module__, cls.__qualname__, name), f, line


class _Result(unittest.TextTestResult):
    def startTest(self, test):
        self._current = test
        self._state = {"outcome": "passed", "message": [], "t0": time.perf_counter()}
        if not _failed_import(test):
            _emit(event="start", id=_info(test)[0])
        super().startTest(test)

    def _set(self, outcome, message):
        st = getattr(self, "_state", None)
        if st is None:
            return False
        rank = {"passed": 0, "skipped": 1, "error": 2, "failed": 3}
        if rank[outcome] >= rank[st["outcome"]]:
            st["outcome"] = outcome
        if message:
            st["message"].append(message)
        return True

    def addFailure(self, test, err):
        super().addFailure(test, err)
        self._set("failed", self._exc_info_to_string(err, test))

    def addError(self, test, err):
        super().addError(test, err)
        text = self._exc_info_to_string(err, test)
        if getattr(self, "_current", None) is test:
            self._set("error", text)
        else:
            # setUpClass / setUpModule: no test is running; mark the class or module.
            desc = str(test)
            target = desc[desc.find("(") + 1 : desc.rfind(")")] if "(" in desc else desc
            parts = target.split(".")
            mod = sys.modules.get(".".join(parts[:-1])) or sys.modules.get(target)
            f = getattr(mod, "__file__", "") or ""
            node = _rel(f) + ("::" + parts[-1] if sys.modules.get(target) is None and f else "") if f else target
            _emit(event="collecterror", id=node, message=text)

    def addSkip(self, test, reason):
        super().addSkip(test, reason)
        self._set("skipped", reason)

    def addExpectedFailure(self, test, err):
        super().addExpectedFailure(test, err)
        self._set("skipped", "XFAIL")

    def addUnexpectedSuccess(self, test):
        super().addUnexpectedSuccess(test)
        self._set("failed", "Unexpected success")

    def addSubTest(self, test, subtest, err):
        super().addSubTest(test, subtest, err)
        if err is not None:
            failed = issubclass(err[0], test.failureException)
            self._set("failed" if failed else "error", "%s\n%s" % (subtest, self._exc_info_to_string(err, test)))

    def stopTest(self, test):
        super().stopTest(test)
        st = getattr(self, "_state", None) or {"outcome": "passed", "message": [], "t0": time.perf_counter()}
        node, _, _ = _info(test)
        message = "\n".join(m for m in st["message"] if m)
        if _failed_import(test):
            _emit(event="collecterror", id=node, message=message)
        else:
            _emit(event="finish", id=node, outcome=st["outcome"], duration=time.perf_counter() - st["t0"], message=message, output="")
        self._state = None
        self._current = None


class BriskRunner(unittest.TextTestRunner):
    resultclass = _Result

    def run(self, test):
        _emit(event="session", rootdir=os.getcwd())
        tests = []
        for t in _flat(test):
            if _failed_import(t):
                continue
            node, f, line = _info(t)
            tests.append({"id": node, "file": f, "line": line})
        _emit(event="collected", tests=tests)
        result = None
        try:
            result = super().run(test)
        finally:
            ok = result is not None and result.wasSuccessful()
            _emit(event="done", exitstatus=0 if ok else 1)
        return result


def _load_module(path):
    rel = _rel(os.path.abspath(path))
    dotted = rel[:-3].replace("/", ".") if rel.endswith(".py") and not rel.startswith("..") else None
    if dotted:
        try:
            return importlib.import_module(dotted)
        except Exception:
            pass
    name = os.path.splitext(os.path.basename(path))[0]
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


def _doctest_main(argv):
    verbosity, flags, paths = 1, 0, []
    it = iter(argv)
    for a in it:
        if a in ("-v", "--verbose"):
            verbosity = 2
        elif a in ("-o", "--option"):
            flags |= doctest.OPTIONFLAGS_BY_NAME[next(it)]
        elif a.startswith("-o"):
            flags |= doctest.OPTIONFLAGS_BY_NAME[a[2:]]
        else:
            paths.append(a)
    suite = unittest.TestSuite()
    for p in paths:
        path, _, only = p.partition("::")
        if path.endswith(".py"):
            sub = doctest.DocTestSuite(_load_module(path), optionflags=flags)
        else:
            sub = doctest.DocFileSuite(path, module_relative=False, optionflags=flags)
        for t in _flat(sub):
            if not only or t._dt_test.name == only:
                suite.addTest(t)
    result = BriskRunner(verbosity=verbosity).run(suite)
    sys.exit(0 if result.wasSuccessful() else 1)


def main():
    argv = sys.argv[1:]
    if argv[:1] == ["--doctest"]:
        _doctest_main(argv[1:])
        return
    sys.argv[0] = "python -m unittest"
    unittest.main(module=None, argv=[sys.argv[0]] + argv, testRunner=BriskRunner)


if __name__ == "__main__":
    main()
