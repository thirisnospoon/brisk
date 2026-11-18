# Brisk Python Console backend. Requests come as JSON lines on stdin:
#   {"op": "run", "code": "..."}    run a statement block (the last expression is echoed)
#   {"op": "check", "code": "..."}  is the code a complete statement?
#   {"op": "input", "text": "..."}  a line for a pending input() call
#   {"op": "interrupt"}             KeyboardInterrupt in the running code (the session stays)
# Replies and events go to stdout as "\x1e<kind> <json>\n"; anything else on stdout is the
# program's own output. stderr from Python code is sent in-band as "err" events so it keeps
# its order with stdout.
import sys

# With -c the working directory comes first on sys.path: a project file named like a standard
# module (types.py, json.py) must not break the console's own imports. It is put back after.
_cwd_entry = sys.path.pop(0) if sys.path and sys.path[0] in ("", ".") else None

import ast  # noqa: E402
import builtins  # noqa: E402
import codeop  # noqa: E402
import json  # noqa: E402
import os  # noqa: E402
import queue  # noqa: E402
import signal  # noqa: E402
import threading  # noqa: E402
import traceback  # noqa: E402
import types  # noqa: E402

sys.path.insert(0, os.getcwd())
try:
    sys.stdout.reconfigure(newline="\n")  # no \r\n from Windows text mode
except (AttributeError, ValueError):
    pass

_out = sys.stdout
_stdin = sys.stdin


def _send(kind, payload):
    _out.write("\x1e" + kind + " " + json.dumps(payload) + "\n")
    _out.flush()


class _Err:
    encoding = "utf-8"

    def write(self, s):
        if s:
            _send("err", s)
        return len(s)

    def flush(self):
        pass

    def isatty(self):
        return False


sys.stderr = _Err()


# Requests are read by a thread so that "interrupt" arrives while code runs; the rest go
# through this queue (None: stdin closed).
_requests = queue.Queue()
_busy = threading.Event()


def _reader():
    for line in _stdin:
        try:
            req = json.loads(line)
        except ValueError:
            continue
        if req.get("op") == "interrupt":
            if _busy.is_set():
                # The C-level handler also wakes time.sleep() and blocking waits.
                signal.raise_signal(signal.SIGINT)
            continue
        _requests.put(req)
    _requests.put(None)


def _next_request():
    # A timeout keeps the wait interruptible on Windows.
    while True:
        try:
            return _requests.get(timeout=0.2)
        except queue.Empty:
            pass


def _input(prompt=""):
    sys.stdout.flush()
    _send("input", str(prompt))
    while True:
        req = _next_request()
        if req is None:
            raise EOFError
        if req.get("op") == "input":
            return req.get("text", "")


builtins.input = _input


class _Quitter:
    # site's exit() closes sys.stdin first, which would wait for the reader thread's lock.
    def __init__(self, name):
        self.name = name

    def __repr__(self):
        return "Use %s() to end the console" % self.name

    def __call__(self, code=None):
        raise SystemExit(code)


builtins.exit = _Quitter("exit")
builtins.quit = _Quitter("quit")
ns = {"__name__": "__main__", "__builtins__": builtins}


def _variables():
    out = []
    for name, value in list(ns.items()):
        if name.startswith("_") or isinstance(value, types.ModuleType):
            continue
        if callable(value) and getattr(value, "__module__", None) == "builtins":
            continue
        try:
            text = repr(value)
        except Exception as e:  # noqa: BLE001
            text = "<repr failed: %s>" % e
        if len(text) > 300:
            text = text[:300] + "…"
        size = None
        try:
            if isinstance(value, (list, tuple, dict, set, str, bytes)):
                size = len(value)
        except Exception:  # noqa: BLE001
            pass
        out.append([name, type(value).__name__, text, size])
    out.sort(key=lambda v: v[0].lower())
    return out


def _run(src):
    try:
        tree = ast.parse(src, "<console>", "exec")
    except SyntaxError:
        traceback.print_exception(*sys.exc_info()[:2], None)
        return
    last = None
    if tree.body and isinstance(tree.body[-1], ast.Expr):
        last = ast.Interactive(body=[tree.body.pop()])
    try:
        if tree.body:
            exec(compile(tree, "<console>", "exec"), ns)
        if last is not None:
            exec(compile(last, "<console>", "single"), ns)
    except SystemExit:
        raise
    except BaseException:
        etype, value, tb = sys.exc_info()
        # Drop this function's own frame from the traceback.
        traceback.print_exception(etype, value, tb.tb_next if tb else None)


def _main():
    signal.signal(signal.SIGINT, signal.default_int_handler)
    threading.Thread(target=_reader, daemon=True).start()
    _send("ready", {"version": sys.version.split()[0], "executable": sys.executable})
    while True:
        try:
            req = _next_request()
        except KeyboardInterrupt:
            continue  # a late interrupt after the command ended
        if req is None:
            break
        op = req.get("op")
        if op == "check":
            src = req.get("code", "")
            try:
                # "single" like the REPL: a compound statement waits for an empty line.
                complete = codeop.compile_command(src, "<console>", "single") is not None
            except (SyntaxError, ValueError, OverflowError):
                complete = True  # running it shows the error
            _send("check", {"complete": complete})
        elif op == "run":
            _busy.set()
            try:
                _run(req.get("code", ""))
            except SystemExit:
                _busy.clear()
                _send("done", {"vars": _variables(), "exit": True})
                break
            except KeyboardInterrupt:
                # Raised outside the user's code (between bytecodes of _run itself).
                print("KeyboardInterrupt", file=sys.stderr)
            finally:
                _busy.clear()
            sys.stdout.flush()
            _send("done", {"vars": _variables()})


_main()
# The reader thread holds stdin's lock: a normal shutdown would wait for it forever.
sys.stdout.flush()
os._exit(0)
