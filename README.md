# Brisk

Brisk is a lightweight Python IDE for people whose hands already know PyCharm.
It keeps PyCharm's window layout, default keymap, action names and most of its
behaviour, and it is written from scratch in Rust on [GPUI](https://www.gpui.rs/).
The whole IDE is one 26 MB executable that opens a project in about half a second
and sits at roughly 60 MB of memory while idle.

![Brisk editing a Python project](docs/images/brisk.png)

## Download

Brisk is currently built for **Windows 10/11 x64**.

Get `Brisk-<version>-windows-x64.zip` from the [latest release](https://github.com/thirisnospoon/brisk/releases/latest),
unpack it anywhere and run `brisk.exe`. There is no installer and no admin
rights are needed. The zip already contains:

- [ty](https://github.com/astral-sh/ty) for types, navigation, completion and rename,
- [ruff](https://github.com/astral-sh/ruff) for inspections, quick fixes and formatting,
- [debugpy](https://github.com/microsoft/debugpy) for the debugger.

You only need a Python interpreter on the machine to run and debug code.
Settings live next to the executable (portable mode), so the folder can be moved
or carried on a USB stick.

## What it does

If you have used PyCharm, nothing below needs explaining, and that is the point.

- **Editor**: multiple carets and column selection, Extend Selection (Ctrl+W),
  smart Enter and paste, Join Lines, folding, sticky lines, soft wraps,
  live templates and postfix completion, Local History, scratch files.
- **Code insight**: completion with auto-import, Ctrl+B / Ctrl+Alt+B, Find Usages,
  Quick Documentation, Parameter Info, inlay type hints, inspections with
  Alt+Enter fixes, Optimize Imports and Reformat Code (also on save).
- **Refactoring**: Rename (in place, or project-wide with a preview), Extract
  Variable / Constant / Method, Inline, Generate (`__init__`, `__repr__`,
  `__eq__`, properties), Override / Implement Methods, Move Statement, Surround With.
- **Run and debug**: run configurations compatible with `.idea/runConfigurations`
  (Python, module, pytest, unittest, FastAPI, Flask, Django), gutter run marks,
  breakpoints with conditions and exception breakpoints, stepping, Smart Step
  Into, Evaluate, Watches, inline values, Attach to Process, View as DataFrame.
- **Testing**: test tree with rerun of failed tests, test history, coverage
  and the cProfile profiler.
- **Git**: commit with partial commits by hunk, diff, three-way merge,
  annotate (blame), log with graph and filters, branches, stash, push/pull,
  cherry-pick, reset, revert.
- **Tools**: Python Console, Python Packages, terminal, Search Everywhere,
  Find in Files, TODO, Problems, Services, a database console for SQLite,
  PostgreSQL and MySQL.

The full list, with the places where Brisk deliberately differs from PyCharm,
is in [docs/features.md](docs/features.md).

## What it does not do (yet)

- Only Python gets full language support. JSON, TOML, YAML, JavaScript,
  TypeScript, HTML and CSS are highlighted but have no code insight.
- No plugins, no remote interpreters (SSH, WSL, Docker), no Jupyter notebook editor.
- Only Windows builds are produced and tested. GPUI runs on macOS and Linux,
  but Brisk has not been ported there.
- It is early software (0.2.x). Expect rough edges, and please
  [open an issue](https://github.com/thirisnospoon/brisk/issues) when you hit one.

## Performance budget

Every merge is checked against fixed limits with `scripts\budget.ps1` and a set
of UI smoke tests that drive the real window:

| Metric | Limit | 0.2.7 |
|---|---|---|
| `brisk.exe` size | 26 MB | 25.7 MB |
| Portable zip (with ty, ruff, debugpy) | 39 MB | 38.5 MB |
| Idle working set, demo project | 110 MB | 62 MB |
| Working set after a heavy session (200k lines of output, 50 tabs, find in files) | 200 MB | 148 MB |
| Frame time while scrolling any large list (20k files, 10k variables, 2k commits) | p95 16 ms | under 10 ms |

## Building from source

You need a stable Rust toolchain (MSVC on Windows) and PowerShell.

```powershell
git clone https://github.com/thirisnospoon/brisk
cd brisk
powershell -ExecutionPolicy Bypass -File scripts\fetch-tools.ps1     # ty and ruff into vendor\tools
powershell -ExecutionPolicy Bypass -File scripts\fetch-debugpy.ps1   # debugpy for the debugger
cargo run --release -p brisk-app -- examples\demo_project
```

`scripts\package-portable.ps1` builds the release zip. See
[docs/building.md](docs/building.md) for details and [docs/testing.md](docs/testing.md)
for the unit and smoke tests.

## How it is put together

The UI is one GPUI crate; everything else is a plain Rust library that can be
tested without a window.

| Crate | Purpose |
|---|---|
| `brisk-app` | Window, editor, tool windows, dialogs |
| `brisk-text` | Rope buffer, undo, encodings, smart editing |
| `brisk-syntax` | tree-sitter parsing, highlighting, folding, outline, intentions |
| `brisk-lsp` | ty client (LSP) |
| `brisk-lint` | ruff integration |
| `brisk-dap` | Debug Adapter Protocol client for debugpy |
| `brisk-run` | Run configurations (including `.idea` import), test runners, output parsing |
| `brisk-git` | Git through the `git` CLI, so your credentials, hooks and LFS just work |
| `brisk-project` | Project tree and the fuzzy file index behind Go to File |
| `brisk-search` | Find in Files |
| `brisk-terminal` | ConPTY plus `alacritty_terminal` for VT emulation |
| `brisk-db` | Database console (SQLite, PostgreSQL, MySQL) |
| `brisk-keymap` | Actions and the PyCharm keymap |
| `brisk-proc` | Child processes in one Job Object, so nothing outlives the IDE |

Design notes are in [docs/architecture.md](docs/architecture.md).

## Relationship to JetBrains

Brisk is an independent project and is not affiliated with JetBrains. It
reproduces PyCharm's behaviour and shortcuts so that the habits carry over; the
code, icons and artwork are its own.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Bundled third-party components keep their own licenses: Lucide icons (ISC),
ty and ruff (MIT), debugpy (MIT), and the Rust crates compiled into `brisk.exe`.
Their full texts are in [THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt), which
`scripts\third-party-notices.ps1` regenerates and the release zip includes.
