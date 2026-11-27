# Goals and design

Brisk is a lightweight IDE with PyCharm's behaviour. Someone coming from PyCharm should be able to open a project and work with the same habits (tool windows, shortcuts, behaviour), but the IDE should start in under a second and not use gigabytes of memory.

The focus is Python first, followed by modern frontend work (TS / JS, React / Vue, CSS, Tailwind). Frontend support beyond syntax highlighting is planned.

## Targets

| Metric | Target |
|---|---|
| Cold start to an open project | < 1 s |
| Memory without a project | < 80 MB |
| Memory with a medium Python and frontend project, including language servers | < 400 MB |
| Key press to character on screen | < 10 ms |
| Installer | < 50 MB |
| Reopening a project (index from disk, mmap) to a working Search Everywhere | < 300 ms |

`scripts\budget.ps1` is meant to run on every merge and checks stricter limits for the release build: executable size, zip size, startup time and memory. See [building.md](building.md#size-startup-and-memory-budget).

## Principles

1. Rust everywhere: core, UI and index. External tools are native where possible (ruff, ty, tsgo and biome).
2. Laziness. Nothing starts or gets indexed until it is needed. Dependencies should be indexed in the background.
3. Processes are the main source of bloat. Each external server should start lazily, one per project, and stop when idle. Today ty starts in the background once interpreters are found; idle shutdown is planned.
4. Virtualize everything. Trees, lists, tables and the log draw only what is visible.
5. A core without UI. Logic lives in crates that do not depend on the GUI and are tested on their own. The UI layer is thin.
6. Compatible with PyCharm in behaviour, not in appearance. Icons and graphics are Brisk's own, since JetBrains' are copyrighted. Window layout, shortcuts and logic are the same. Brisk reads `.idea/` for run configurations, data sources and the test runner.

## Crates

The crates, how they depend on each other and how the UI is put together are described in [architecture.md](architecture.md).

Planned:

- `brisk-index`: a persistent symbol index (mmap on disk) and a trigram index for Find in Files in large monorepos.
- Browser debugging through CDP in `brisk-dap`.
- `brisk-docker`: the Docker Engine API (named pipe or Unix socket) and compose.
- `brisk-remote`: a small agent for WSL, SSH and Docker.

## External engines

| Task | Engine | Language | Status |
|---|---|---|---|
| Python types, completion, navigation | `ty` (fallbacks: `pyrefly`, `basedpyright`) | Rust | ty in use, fallbacks planned |
| Python lint and format | `ruff` | Rust | in use |
| Python debugging | `debugpy` | Python | in use |
| TypeScript | `tsgo` | Go | in use |
| JS / TS / CSS / JSON lint and format | `biome` (`oxc` later) | Rust | biome in use |
| CSS / HTML / Tailwind | Brisk's own, on tree-sitter and the index | Rust | planned |
| ESLint, Prettier, Volar | only when the project needs them, started lazily | Node | Prettier in use, the rest planned |

## Components Brisk writes itself

These exist to keep the IDE light.

| Component | Purpose | Status |
|---|---|---|
| Symbol index | Instant Search Everywhere, Go to Symbol and auto-import. For site-packages, signatures only. String interning, flat arrays, mmap, a cache keyed by `package==version`. | An in-memory tree-sitter index of project classes, functions and module-level names. The persistent index is planned. |
| Language server manager | Lazy start, idle shutdown, memory limits | Planned beyond the current ty client |
| Trigram index | Instant Find in Files in a monorepo | Planned. Find in Files is a parallel scan today. |
| Python environment detection | Read `pyvenv.cfg`, `uv.lock`, `poetry.lock` and conda without starting Python; cache `sys.path` | `pyvenv.cfg` and conda are read directly. The rest is planned. |
| Test discovery from the index | Run marks next to `test_*` at once, without `pytest --collect-only` | Done: run marks are computed from the file itself, without running pytest. |
| Local History | Deduplication and compression | Revisions are stored gzip-compressed. Deduplication is planned. |
| Git log graph | Own layout, reading `commit-graph` | The lane layout is Brisk's own (`brisk-git`). Reading `commit-graph` is planned. |
| CSS / HTML / Tailwind | Instead of Node servers | Planned |
| Memory discipline | Unload buffers and tree-sitter trees of inactive files, a budget per subsystem | Planned. Growth is bounded today, see below. |
| Remote agent | A small binary instead of the whole IDE on the remote side | Planned |

## PyCharm compatibility

### Tool windows

| Window | Shortcut |
|---|---|
| Project | `Alt+1` |
| Bookmarks | `Alt+2` |
| Find | `Alt+3` |
| Run | `Alt+4` |
| Debug | `Alt+5` |
| Problems | `Alt+6` |
| Structure | `Alt+7` |
| Services (runs, database; Docker planned) | `Alt+8` |
| Git | `Alt+9` |
| Commit | `Alt+0` |
| Terminal | `Alt+F12` |
| Database | none (right stripe or Find Action) |

### Behaviour

Implemented:

- Autosave when focus leaves the window and when switching files.
- Double `Shift` for Search Everywhere, `Ctrl+Shift+A` for Find Action.
- `Alt+Enter` for quick fixes and intentions, `Ctrl+B` / `Ctrl+Click` for Go to Declaration.
- Live templates and postfix completion (`.if`, `.for`, `.not`, `main`).
- Local History, TODO, breadcrumbs (in the Islands style), sticky lines, inlay hints, `Ctrl+P`.
- Import of run configurations, data sources and the default test runner from `.idea/`.

Planned:

- Language injection: SQL in Python strings, HTML / CSS in JS.
- Shelve and changelists.
- Import of the interpreter, source roots and exclusions from `.idea/`, and of keymap XML.

The full shortcut table is in `crates/brisk-keymap/src/pycharm.rs`.

## Resource limits

Everything that could grow without bound is capped:

| What | Limit |
|---|---|
| Run console (per tab, up to 5 tabs) | 20,000 lines of scrollback |
| Terminal | 10,000 lines of scrollback |
| Debug console, Python Console | 5,000 lines of output; Python Console input history 1,000 |
| Undo | 1,000 steps per file |
| Find in Files, Find Usages | 10,000 matches; files over 10 MB are skipped |
| Commit window | draws up to 1,000 files per group, the rest as "… N more files not shown" (selection and commit still cover all) |
| Git log, file history, database query result | 3,000 commits, 500 commits, 500 rows |
| Closed tabs, recent files, recent projects | 30, 50, 20 |
| Symbol index | files over 2 MB are skipped; after a change on disk only files with a different size or modification time are reread |

## Child processes

On Windows, every long-lived child process (ty, ruff, debugpy, runs, terminal shells, Python Console, git blame) is placed in one Job Object created with `KILL_ON_JOB_CLOSE`. If Brisk crashes or is killed from Task Manager, Windows ends them together with their own children. A browser or a new Brisk window opened from Brisk is not part of the job.

## Crash log

A panic on any thread is appended to `crash.log` in the settings folder (`%APPDATA%\Brisk`, or `data` for the portable build): the thread, the location and a backtrace. The file starts over after 1 MB.

## Risks

- GPUI changes its API quickly. The version is pinned and the UI layer is kept thin.
- Weak hardware without a GPU: on Windows, DirectX 11 falls back to WARP (software rendering). It works, but slower.
- PyCharm-level refactorings (Change Signature, Move) will be partly Brisk's own, built on the index.
- Framework knowledge (Django ORM, templates) needs its own logic.

## Roadmap

| # | Stage | Scope | Status |
|---|---|---|---|
| 1 | Skeleton | Window, tool windows, project tree, editor with highlighting, PyCharm keymap, Search Everywhere over files, terminal | Done |
| 2 | Python | Symbol index, ty, ruff, environments, run configurations, debugpy, pytest | Done, except the persistent index |
| 3 | Git | Commit, branches, log graph, diff, merge, blame, shelve | Done, except shelve (stash is available) |
| 4 | Frontend | tsgo, biome, own CSS / HTML / Tailwind support | Planned |
| 5 | Services | Docker, databases | Databases done, Docker planned |
| 6 | Migration | `.idea` import, refactorings, frameworks (Django, FastAPI), remote | Partly done: run configuration and data source import, Rename, Extract Variable, module moves, Django / FastAPI / Flask run configurations. Remote, more refactorings and framework knowledge are planned. |
