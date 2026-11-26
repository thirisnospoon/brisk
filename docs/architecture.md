# Architecture

This document describes how Brisk's code is organized today, what is wrong with that
organization, and the order in which it is going to change. It is written for someone who
wants to work on the code. What Brisk is meant to be (targets, principles, PyCharm
compatibility, resource limits) is in [goals.md](goals.md).

Numbers in this document were measured on `main` at `a59b7de` (0.3.0). File paths that name
no crate are in `crates/brisk-app/src/`. Line numbers drift; search for the named item if a line no longer matches.

## Bird's eye view

Brisk is one native window drawn with GPUI. The window owns one `Workspace` entity, which
owns everything else: the project tree, editor tabs, tool windows, popups, dialogs and the
state of every subsystem (git, runs, debugger, database, language servers).

Work that is not drawing is done in library crates that do not depend on GPUI: the text
buffer, syntax, git, run configurations, the DAP and LSP clients, database drivers. They
are called from `brisk-app` and their results come back to the UI thread through GPUI's
executors.

```
            ┌───────────────────────── brisk-app (GPUI) ─────────────────────────┐
key / mouse │ Workspace ──► Editor, TerminalPanel, DiffView, MergeView, GridView │
  events  ──►   │  dispatch(Action)                                              │
            │   ├─ git_ui, run_ui, debug_ui, db_ui, lsp_ui, find_ui, ...         │
            │   │   (impl Workspace blocks, one file per area)                   │
            └───┼────────────────────────────────────────────────────────────────┘
                │ plain function calls, background tasks
   brisk-text  brisk-syntax  brisk-git  brisk-run  brisk-dap  brisk-lsp  brisk-db ...
                │
           child processes: git, ty, ruff, debugpy, python, shells (all in one Job Object)
```

## Code map

### Library crates

All of these are UI-free and tested on their own (`cargo test -p <crate>`). Line counts are
of `src/`.

| Crate | Lines | Tests | Depends on | Owns |
|---|---|---|---|---|
| `brisk-proc` | 178 | 2 | | The kill-on-close Job Object every long-lived child is adopted into. |
| `brisk-keymap` | 1,415 | 11 | | The `Action` enum (236 actions, `action.rs`), the PyCharm default keymap (`pycharm.rs`), the user keymap and its import from PyCharm XML, and `KeyResolver` (chords, double Shift). |
| `brisk-text` | 2,459 | 40 | | The rope buffer, multi-caret selections, undo / redo, edit operations. |
| `brisk-project` | 672 | 6 | | The lazy project tree and the flat file list for Go to File. |
| `brisk-syntax` | 3,396 | 35 | | tree-sitter parsing, highlighting of the visible range, folding, outline, the symbol index. |
| `brisk-search` | 398 | 7 | | Find / Replace in Files. |
| `brisk-lint` | 472 | 4 | proc, run | ruff: discovery, checks, fixes, noqa. |
| `brisk-lsp` | 1,436 | 10 | proc, lint | JSON-RPC over stdio and a typed API for ty. |
| `brisk-dap` | 562 | 5 | proc | The Debug Adapter Protocol client for debugpy. |
| `brisk-run` | 4,555 | 44 | project | Interpreter discovery, run configurations (`.brisk/run.json`, `.idea` and `.run/*.xml` import), command lines, pytest / unittest runners, traceback links, uv / Poetry detection, SSH interpreters. `env`: creating a venv and installing requirements, uv / Poetry sync and dependency changes, an interpreter's packages and the PyPI name list, the Terminal's activation of the project environment. |
| `brisk-git` | 2,384 | 23 | proc | Git through the `git` CLI: status, log with its lane layout, diff, merge, stash, blame. |
| `brisk-terminal` | 1,282 | 6 | proc | The PTY (ConPTY, openpty) and VT emulation through `alacritty_terminal`. |
| `brisk-db` | 1,777 | 14 | | PostgreSQL, MySQL / MariaDB and SQLite: connections, schema, queries. |
| `brisk-web` | 1,087 | 7 | project | Django / FastAPI / Flask routes and template references. |
| `brisk-vm` | 198 | 3 | | The Windows Hypervisor Platform check for the planned container VM (`docs/design/containers.md`). |

The dependency graph is shallow and has no cycles. Only `brisk-app` depends on GPUI.

### `brisk-app`

57,457 lines, 86 unit tests. It is a binary crate (`[[bin]] brisk`), so it has no library
target and nothing outside it can link against its code.

- `main.rs`: picks the project folder, loads settings, loads fonts, opens the window with
  `Workspace::new`, starts the stall watchdog.
- `workspace.rs` (3,536 lines) and `workspace/` (90 files): the `Workspace` struct and
  everything it does. See below.
- `editor.rs` and `editor/` (2,548 + 7,733 lines): `Editor`, the code editor entity. It
  is also used for one-line input fields (`is_field`). It emits `EditorEvent`
  (`editor.rs:224`).
- `terminal_view.rs` and `terminal_view/`: `TerminalPanel`, used for both the Terminal tool
  window and the Run console. It emits `RunEvent` (`terminal_view.rs:71`).
- `diff_view.rs`, `merge_view.rs`, `grid_view.rs`, `data_view.rs`: the other kinds of editor
  tab (`Tab`, `workspace.rs:238`).
- UI-free modules that live here anyway: `local_history.rs`, `py_console.rs`,
  `todo_scan.rs`, `coverage.rs`, `profile.rs`, `editorconfig.rs`, `run_anything.rs`,
  `settings.rs`, `keymap_store.rs`. None of them imports `gpui`.
- `theme.rs`, `icons.rs`, `ui_kit.rs`, `scrollbar.rs`: drawing helpers. Colors and sizes are
  read from global state (`settings.rs` keeps them in statics).
- `crash.rs` (panic log) and `stall.rs` (main-thread watchdog and frame timing).

### The `Workspace` entity

`Workspace` (`workspace.rs:402`) has 79 fields. Some are sub-structs for one area
(`runs: run_ui::Runs`, `git: Option<git_ui::Git>`, `debug: debug_ui::Debug`, `db: db_ui::Db`,
`lsp: lsp_ui::Lsp`, `find: find_ui::Find`, `problems: problems_ui::Problems`); the rest are
loose fields for tabs, popups, toasts, the status bar, focus handles, split sizes, the key
resolver and a dozen `Option<...>` panels created on first open (`packages`, `todo`, `pyc`,
`coverage`, `profile`, `endpoints`, ...).

Behaviour is spread over 101 `impl Workspace` blocks in 90 files under `workspace/`, with
about 900 methods in total. Each file is named after an area (`git_ui.rs`, `run_ui.rs`,
`debug_ui.rs`) and adds methods to the same struct. Because the files are child modules of
`workspace`, every one of them can read and write every field of `Workspace`, including the
private fields of the sub-structs declared in its siblings when they are `pub`.

The sub-structs are plain data. They have no handles of their own and do not emit events;
the code that changes them is `impl Workspace` code that takes `&mut self`.

### Startup

1. `main.rs` resolves the folder (argument, else the most recent project, else the current
   directory), calls `settings::load()` and starts a thread that prunes Local History.
2. `Workspace::new` (`workspace.rs:545`–`968`) reads `.git/HEAD` for the branch, loads run
   configurations (`run_ui::Runs::new`, which imports `.idea` on first open), builds the
   project tree and the debugger state, and defers interpreter discovery, the web framework
   scan and the disk poll (`disk_ui.rs:64`) to after the first frame.
3. The same function then makes 72 environment variable reads (`BRISK_SMOKE_*`,
   `BRISK_OPEN_*`) and schedules the matching smoke test or screenshot helper.

`workspace/budget_probe.rs` records the time of each stage when `BRISK_BUDGET` is set;
`scripts/budget.ps1` reads it.

### Keys and actions

`Workspace::render` puts `capture_key_down(Self::on_key_down)` on the root element, so the
workspace sees every key before the focused view does.

`on_key_down` (`workspace.rs:1342`) is a sequence of special cases tried in order: the
Keymap dialog's key capture, Ctrl Ctrl + arrows, the Terminal and Run console (which get raw
keys except a fixed list of IDE actions, `IDE_ACTIONS` at `workspace.rs:1505`), the Python
Console, Packages, TODO and Endpoints windows, the Git windows (`git_pre_key`), the editor's
search bar, the completion list. Only then does it build the context list
(`contexts`, `workspace.rs:1291`: `"Editor"`, `"ProjectView"`, `"GitLog"`, ...) from which
focus handle is focused, and asks `KeyResolver::key_down` for an action. If the resolver
has none, a second `match` on the first context sends the key to the view's own handler
(`tree_key`, `commit_key`, `log_key`, ...).

`Workspace::dispatch` (`workspace.rs:1552`–`1760`) runs an action. It first offers the
action to `lsp_action` (`lsp_ui.rs:299`), then matches on it. Most arms call into an area's
handler (`git_action`, `run_action`, `db_action`, `python_packages_action`, ...); some do
the work inline. Actions that act on text go to `Editor::perform` (`editor.rs:857`).

Find Action, the main menu (`main_menu.rs`) and the context menus build their entries from
`Action::info()` and the keymap's labels, but whether an entry is enabled, and what it does,
is decided again by hand in each place.

User keymaps are kept in a global (`keymap_store.rs`) with a generation counter;
`sync_keymap` (`keymap_ui.rs:27`) rebuilds the resolver when the counter moves.

### Tool windows

The tool windows are two enums, `LeftTool` (4 variants) and `BottomTool` (13 variants), in
`workspace.rs`. The Database window is neither: it is the right side, shown while
`db.open` is true. Adding a tool window means editing, by hand:

- the enum, its `title()` and `stage()` (placeholder text);
- `render_left_stripe` / `render_right_stripe` (`workspace.rs:2310`, `2339`), each a fixed
  list of `stripe_button` calls with the icon, title and toggle action;
- the `match` that picks the panel in `Workspace::render` (`workspace.rs:3229`);
- `dispatch`, for the toggle action and often a focus or refresh after it;
- `contexts` and the fallback key `match`, if the window takes keys;
- `IDE_ACTIONS`, if the shortcut should work from the terminal.

`IDE_ACTIONS` already lists Endpoints but not Coverage or Profiler, which shows how these
lists fall out of step.

### Editors and events

The editor, terminal, diff, merge and grid views are separate GPUI entities and the only
places where Brisk uses events. `Workspace` subscribes to them where it creates them, for
example `problems_ui.rs:210` for `EditorEvent` and `run_ui.rs:155` for `RunEvent`.
Between the areas inside `Workspace` there are no events: run code that wants to show a
toast calls `self.show_toast`, debug code that needs the run configuration reads
`self.runs`, and so on.

### Background work

Blocking work (processes, file scans, git, database queries) runs on GPUI's background
executor; the result is applied in `cx.spawn` / `this.update`. Results that may arrive out
of order are guarded by generation counters stored next to the state they protect
(`reindex_gen`, `toast_gen`, `Debug::gen`, ...). Polling, such as the disk poll and the
smoke drivers, uses `background_executor().timer` in a loop.

### Tests and smokes

- Library crates: 217 unit tests. Some need tools on `PATH` (ty, ruff) or a database server
  and are skipped or `#[ignore]`d; see [testing.md](testing.md).
- `brisk-app`: 86 unit tests, mostly on the UI-free modules listed above.
- Smokes: 41 `*_smoke.rs` files under `workspace/` (7,803 lines), plus smoke functions in
  ordinary files such as `debug_ui.rs:3584`. Each is started by an environment variable,
  drives the real window from inside the process, prints to stderr and quits. They are
  compiled into every build, including the release exe.
- `scripts/budget.ps1` checks the release exe size (26 MB), the zip (39 MB), start to first
  frame (400 ms), idle memory (110 MB) and memory after `BRISK_SMOKE_MEMBUDGET` (200 MB).

There is no CI. `rustfmt` and `clippy` are not part of any script, there is no
`rustfmt.toml`, and 5,050 lines in `crates/*/src` are longer than 100 columns, so the code
has never been formatted with `cargo fmt`.

## Problems

Ranked by how much each one costs when changing the code. The first three are why a change
in one area tends to need edits in five files.

### 1. `Workspace` owns all state, and every area can touch all of it

79 fields, 101 `impl` blocks, about 900 methods, 506 `pub(super)` / `pub(crate)` functions
under `workspace/`. Nothing in the type system says which code owns which field, so the
only way to know what a change affects is to grep.

Measured reach (files under `workspace/`, smokes excluded, that read the field through
`self`):

| Field | Files |
|---|---|
| `runs` | 17 (`run_ui`, but also `debug_ui`, `lsp_ui`, `problems_ui`, `packages_ui`, `venv_ui`, `console_ui`, `services_ui`, `status_ui`, ...) |
| `git` | 12 |
| `tabs` | 8 |
| `problems`, `symbols` | 5 each |

`runs` is reached from so many places because the selected interpreter lives in
`run_ui::Runs` (`run_ui.rs:13`) next to the run configurations and the Run console.
Anything that needs "the project's Python" (ty, ruff, packages, the console, venv creation)
reads it from the run state.

Consequences: an area cannot be tested without a whole window; two people working on Git
and on Debug edit the same struct; a field added for one area is visible to all of them; and
the borrow checker cannot help, because everything goes through one `&mut self`.

### 2. Actions and tool windows are wired by hand in several places

`dispatch` is a 210-line `match` (`workspace.rs:1552`). Tool windows are described in at
least six places (see [Tool windows](#tool-windows)). `on_key_down` has a fixed order of
special cases that each area adds to. Menus decide "enabled" separately from `dispatch`.

There is no single place that says "this action belongs to Git, is enabled when there is a
repository, and runs this function", so there is no test that every `Action` is handled,
and a new tool window or action is easy to wire half-way.

### 3. Smoke tests are part of the product

- 7,803 lines of smoke code in the release exe.
- `Workspace::new` is 420 lines, most of them `if std::env::var_os("BRISK_SMOKE_...")`
  blocks. 91 `env::var` reads of `BRISK_SMOKE_*` outside the smoke files, 67 of them in
  `workspace.rs`; 105 counting all `BRISK_*` variables.
- Production paths change behaviour when any `BRISK_SMOKE_` variable is set:
  `py_packages.rs:152` (pip goes offline), `run_ui.rs:169` (no browser is opened).
- Smokes call private `Workspace` methods, so they pin the internals and have to be updated
  by every refactor below.

### 4. Large files that mix concerns

| File | Lines | Contains |
|---|---|---|
| `workspace/debug_ui.rs` | 4,372 | session lifecycle, breakpoints and their dialog, exception breakpoints, threads and frames, variables and watches, the Debug Console with completion, Smart Step Into, Attach to Process, rendering of all of it, a 670-line smoke, tests, and 300 lines of pure parsing (`python_order`, `hover_expression`, `inline_values`, `process_target`, `targets_on_statement`, ... at lines 561–846) |
| `workspace.rs` | 3,536 | the struct, startup, smoke scheduling, keys, dispatch, popups, the project and structure views, tabs, the status bar, render |
| `editor.rs` | 2,548 | `Editor` with 73 fields and 41 `impl` blocks across `editor/` |
| `workspace/run_config_ui.rs` | 1,554 | the Run Configurations dialog |
| `workspace/git_ui.rs` | 1,541 | Commit, Git log, branches sidebar, commit details |

### 5. Logic that is not UI lives in `brisk-app`

About 2,900 lines in `brisk-app` do not import `gpui` and belong to a library crate:
`local_history.rs`, `py_packages.rs`, `py_venv.rs`, `py_manager.rs`, `py_console.rs`,
`todo_scan.rs`, `coverage.rs`, `profile.rs`, `editorconfig.rs`, `run_anything.rs`,
`terminal_env.rs`, and the file format and load / save half of `settings.rs`. More is
inside UI files: the DAP parsing in `debug_ui.rs` above, `run_file_stamp` in `run_ui.rs`,
the log filter matching in `git_log_filter.rs`.

Because `brisk-app` is a binary, this code can only be tested by `cargo test -p brisk-app`,
which builds GPUI and the whole app.

### 6. No CI, no formatter, no linter

Nothing checks a commit before it is merged. Format, lints and tests run when someone
remembers to run them; `budget.ps1` is run by hand before a release. The first `cargo fmt`
will touch almost every file, which is why it has to be done once, between branches (step 1
below).

### 7. Error handling at the edges

70 `.unwrap()` calls in `brisk-app` outside `#[cfg(test)]` modules; about 20 of them are in
smoke files. The densest production case is `editor/find.rs:80`–`104`, which re-borrows
`self.find` with `as_mut().unwrap()` six times in one function. None of these is on input
from disk or a process, but they are where a refactor would introduce a panic.

### 8. Global state

`settings.rs` keeps the current settings in statics (`CURRENT`, `UI_ZOOM`, `FONT_SIZE`, ...),
`keymap_store.rs` the user keymap, `editor/select.rs:9` the clipboard history. `Theme::*`
reads them. This is convenient for drawing and acceptable for one window, but a unit test
that touches settings affects every other test in the process, and a second window would
share them.

### 9. String key contexts

Keymap contexts are `&'static str` (`"Editor"`, `"ToolWindow"`, `"GitLog"`) written in
`pycharm.rs` and in `Workspace::contexts`. A typo unbinds a shortcut silently.


### 10. Platform code is spread over the crates

Brisk ships for Windows only, but most of its code is not tied to Windows: GPUI, the PTY
(`brisk-terminal` has ConPTY and openpty), git, the DAP and LSP clients and the database
drivers all work on Linux and macOS. What is tied to Windows is scattered:

- 53 `#[cfg(windows)]` / `target_os = "windows"` items in 34 files across 11 crates.
- Virtualenv layout (`Scripts\python.exe` against `bin/python`): 47 references in 12 files,
  most in `brisk-run/src/ssh.rs` (13, for the remote host), `terminal_env.rs` (11) and
  `brisk-run/src/interp.rs` (8).
- Default shells (`cmd`, PowerShell, `$SHELL`) are chosen in 9 files, among them
  `brisk-terminal`, `brisk-run`, `terminal_view.rs` and `run_anything.rs`.
- `CREATE_NO_WINDOW` is set by hand at 24 call sites in 22 files; the kill-on-close Job
  Object lives in `brisk-proc`, but every call site has to remember `adopt`.
- The atomic replace uses `ReplaceFileW` in `editor/disk.rs:121`, with its own fallback.
- Saved database passwords use DPAPI (`brisk-db/src/secrets.rs`); without it they are stored
  as `plain:` hex, which a Linux build would do.
- The settings folder (`%APPDATA%\Brisk`, or `data` in the portable build) is defined in
  `brisk-db/src/config.rs:24` and used by the whole app; `APPDATA` is also read directly in 6
  other files.
- Build and release tooling is PowerShell only, and the mouse smokes post Win32 messages
  (`workspace/smoke/win_input.rs`).
- Nothing builds the workspace on Linux, so whether it compiles there is unknown.

None of this is wrong for a Windows product. The cost comes with a port: every one of these
places would have to be found and fixed on its own.

## Target architecture

The goal is that a change in one area touches one area's files, that each area can be read
and tested without the rest, and that adding a tool window or an action is one entry in a
table plus the code that does the work.

### Sub-models owned by `Workspace`

Each tool area becomes a GPUI entity with its own state, its own methods and its own events.
`Workspace` keeps an `Entity<...>` per area and stops being the place where area logic
lives.

| Entity | Takes over |
|---|---|
| `PythonEnv` | the interpreter list, the selected interpreter, discovery, venv creation, uv / Poetry sync (`runs.venvs`, `runs.all`, `venv_busy`, `venv_offer`, `pyproject_fp`, `ssh_found`) |
| `RunModel` | run configurations, the Run console, Before launch, coverage and profile runs (`runs`, `before`, `before_passed`, `cov_*`, `prof_*`, `test_marks`) |
| `DebugModel` | `debug_ui::Debug`, `debug_edit` |
| `VcsModel` | `git_ui::Git`, `git_init_running`, `branch`, the Commit window |
| `DbModel` | `db_ui::Db` and the data grids it opens |
| `LangModel` | `lsp`, `problems`, `symbols`, `nav` |
| `SearchModel` | `find`, Find Usages |
| `PackagesModel`, `ConsoleModel`, `TodoModel`, `WebModel` | the panels created on first open |

What stays in `Workspace`: the project tree, tabs and splits, popups, dialogs, focus, the
status bar and toasts, and the layout of tool windows.

Rules:

- A sub-model never holds an `Entity<Workspace>`. It depends on what it needs, passed in at
  construction: the project root, other sub-models (for example `RunModel` and `LangModel`
  hold `Entity<PythonEnv>`), and the library crates.
- It reports to the outside through events, `impl EventEmitter<RunEvent> for RunModel`:
  "open this file at this line", "show this message", "the interpreter changed", "focus me".
  `Workspace` subscribes when it creates the entity and is the only place that turns these
  events into tab, toast and focus changes.
- Each tool window's view is its own entity implementing `Render`, reading its model. The
  workspace places it; it does not draw it.
- Generation counters stay with the state they guard, inside the model.

This is how the existing views (`Editor`, `TerminalPanel`, `MergeView`) already work; the
change applies the same pattern to the areas that are still `impl Workspace`.

### Logic in library crates

| From `brisk-app` | To |
|---|---|
| `py_venv.rs`, `py_manager.rs`, `py_packages.rs`, `terminal_env.rs` | `brisk-run::env` (environments next to interpreter discovery) |
| `coverage.rs`, `profile.rs`, `run_anything.rs`, `py_console.rs` (the protocol), `run_file_stamp` | `brisk-run` |
| the free functions in `debug_ui.rs` from line 561 | `brisk-dap` (`stack`, `hover`, `targets`) |
| log filter matching in `git_log_filter.rs` | `brisk-git::log` |
| `todo_scan.rs` | `brisk-search` |
| `editorconfig.rs` | `brisk-project` |
| `local_history.rs` | a new `brisk-history` crate (storage format, pruning; it has no other natural owner) |
| the `Settings` type, its JSON format, load / save with `.bak` | a new `brisk-settings` crate; the statics and `Theme` stay in the app |

Each move brings its tests along, and the moved code gets tests where it has none
(`settings.rs` and `keymap_store.rs` have none today).

`brisk-app` becomes a library plus a thin `main.rs`, so its UI-free parts and the sub-models
can be tested without starting the binary.

### Smokes behind a feature

- `brisk-app` gets a `smoke` cargo feature, off by default. Release builds and the portable
  zip are built without it.
- All smoke code moves to `brisk-app/src/workspace/smoke/`, compiled only with the feature.
  It stays a child of `workspace` until the sub-models of phase 4 give the smokes a public
  API to drive. One function, `workspace::smoke::schedule(window, cx)`, called once at the
  end of `Workspace::new`, reads the environment and starts the matching smoke. Without the
  feature it does not exist and `Workspace::new` has no smoke code. What production code
  needs to know (`crate::smoke::active()`, `crate::smoke::on(name)`) answers no without the
  feature.
- The production checks for "a smoke is running" (`py_packages.rs:152`, `run_ui.rs:169`)
  become `smoke::active()`, which is `false` without the feature.
- Smokes drive the app through actions and sub-model methods, not private `Workspace`
  methods, so they survive refactors of the workspace.
- `budget_probe` stays in the release build: it measures the shipped exe and costs one
  environment read. `BRISK_SMOKE_MEMBUDGET` runs on a `--features smoke` build of the same
  commit.
- The screenshot helpers (`BRISK_OPEN_FILE`, `BRISK_OPEN_SETTINGS`, ...) move under the same
  feature.

### Tool window and action registry

One table describes every tool window:

```rust
pub struct ToolWindow {
    pub id: ToolWindowId,           // replaces LeftTool, BottomTool and `db.open`
    pub title: &'static str,
    pub icon: &'static str,
    pub side: Side,                 // Left, Bottom, Right
    pub toggle: Action,
    pub key_context: Option<KeyContext>,
    pub visible: fn(&Workspace, &App) -> bool,   // Endpoints only in a web project, ...
    pub view: fn(&mut Workspace, &mut Window, &mut Context<Workspace>) -> AnyView,
}
```

The stripes, the panel `match` in `render`, the toggle arms in `dispatch`, `contexts` and
`IDE_ACTIONS` are generated from it. The view is created on first open, as the
`Option<...>` panels are today.

Actions get an owner. Each sub-model implements

```rust
fn handles(action: Action) -> bool;
fn enabled(&self, action: Action, cx: &App) -> bool;
fn run(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>);
```

and `Workspace::dispatch` becomes: focused editor first, then the owner of the action, then
the workspace's own actions (tabs, popups, layout). Menus and Find Action ask the same
`enabled`. A unit test walks `ALL_ACTIONS` and fails if an action has no owner or more than
one.

`on_key_down` keeps its order of precedence, but each special case becomes a key handler
registered by the view that needs it, checked in a defined order, instead of a new `if` in
`workspace.rs`. Key contexts become an enum in `brisk-keymap`, so a misspelt context does
not compile.

`Action` itself stays a static enum in `brisk-keymap`: it is what keymap files and the
PyCharm import refer to by id.


### `brisk-platform`

One crate at the bottom of the dependency graph holds everything that differs between
operating systems. It has no `brisk-*` dependencies, so every crate can use it. Inside, each
concern is a module with `windows.rs` and `unix.rs` (or `linux.rs` / `macos.rs` where they
differ) behind one `#[cfg]` on the `mod` line. A Linux port is a second implementation of
this crate; callers do not change.

| Module | API | Windows today |
|---|---|---|
| `process` | `command(program) -> Command` (no console window, adopted into the kill-on-close group on spawn), `kill_tree`, `memory`, `live_children` | `CREATE_NO_WINDOW`, Job Object (all of `brisk-proc` moves here) |
| `python` | `Layout::Windows` / `Layout::Unix` with `python()`, `scripts_dir()`, `activate()`; `Layout::host()` for this machine | `Scripts\python.exe`. The layout is a value, not a `cfg`, because an SSH or WSL interpreter has the Unix layout on a Windows host. |
| `shell` | `default_shell()`, `shell_command(line)` | `cmd` / PowerShell |
| `fs` | `replace_atomic(tmp, target)`, `reveal(path)`, `trash(path)`, `open_with_default(path)` | `ReplaceFileW`, Explorer |
| `secrets` | `protect(&[u8])`, `unprotect(&[u8])` | DPAPI. The Unix side returns "no secret store" until a keyring backend is written; the data source dialog then refuses to save the password and says why, instead of writing it in clear text. |
| `dirs` | `config_dir()`, `data_dir()`, `cache_dir()`, `portable()` | `%APPDATA%\Brisk`, `data` next to the exe |
| `fonts` | default UI and editor font names, extra font folders | Segoe UI, Consolas (now in `theme.rs` and `main.rs`) |

Rules:

- No `cfg(windows)` outside `brisk-platform`, `main.rs` (the window subsystem attribute) and
  the smoke input module. CI checks this with a grep.
- Paths are built with `Path::join`, never with `\` or `/` in string literals. Tests compare
  `Path`s, not strings.
- The `python` layouts are tested on every OS: they are plain path logic.
- Build and release tooling moves from PowerShell to a Rust `xtask` crate
  (`cargo xtask package`, `cargo xtask budget`), as rust-analyzer does. The `.ps1` files
  become one-line wrappers or go away.

### CI

A GitHub Actions workflow on `windows-latest`, on every push and pull request:

1. `cargo fmt --all --check`, with a `rustfmt.toml` committed in step 1.
2. `cargo clippy --workspace --all-targets -- -D warnings`, and again with
   `--features smoke`.
3. `cargo test --workspace`. Tests that need ty, ruff or a database keep skipping when the
   tool is missing.
4. Release build and `scripts\package-portable.ps1`, then `scripts\budget.ps1` with the size
   limits (exe, zip) enforced. Start time and idle memory on shared runners vary too much
   to fail a build on; they are printed, and enforced only on the release machine as today.
5. A small set of headless smokes on the `--features smoke` build: the ones that exit 1 on
   failure and need no mouse or network (SCOPE, EDIT2, RUNCFG, DISK, MEMBUDGET).

6. `cargo check --workspace --all-targets` and `cargo test` of the library crates on
   `ubuntu-latest` (GPUI needs the `libxkbcommon`, `libwayland`, `libvulkan` and
   `libfontconfig` dev packages). Packaging and the size budget stay Windows-only until there
   is a Linux package.


Workspace lints in the root `Cargo.toml` (`[workspace.lints]`) so every crate gets the same
set, including `clippy::unwrap_used` for non-test code in `brisk-app`.

## Migration path

Every step below is one pull request that builds, passes tests and leaves the app working.
None depends on a later one. The order is chosen so that the steps touching every file
(format, smoke move) come first, while no other branch is open, and the steps that split
`Workspace` go from the least connected area to the most connected.

`ui-audit` was merged in `cef294c`; steps 1 and 2 landed before 0.3.0 (`89bf766`, `0051e6d`).

The exe is at 25.99 of its 26 MB limit (0.3.0), so step 4 (smokes behind a feature) comes
before step 3: it frees the most room, and clippy then runs on code without the smoke hooks.

"Smokes" in the verify column means the smokes listed for that area in
[testing.md](testing.md), run on a copy of `examples/demo_project`. "Budget" means
`scripts\budget.ps1` within limits on a release build.

### Phase 0: tooling

#### Step 1. Format the code once

Add `rustfmt.toml` (`max_width = 160`, `use_small_heuristics = "Max"`: the current style;
at 160 the first run changes 174 files, at 140 it changes 195, at 120 it changes 219). Run `cargo fmt --all` as a commit of its own and list it in
`.git-blame-ignore-revs`.
Files: all `.rs` files. Verify: `cargo build`, `cargo test --workspace`; the commit has no
change other than formatting. Risk: low for behaviour, high for conflicts, so it lands
right after `ui-audit` and before any other branch is cut.

#### Step 2. CI with fmt, tests and size budget

Add `.github/workflows/ci.yml` with jobs 1, 3 and 4 from [CI](#ci). Clippy is not in it yet.
Files: the workflow only. Verify: the workflow passes on `main`. Risk: low. The build cache
matters: a cold GPUI release build is slow, so the workflow caches `target/`.

#### Step 3. Clippy clean, then `-D warnings`

One pull request per crate or two, fixing what `cargo clippy` reports, starting with the
library crates. The last one adds `[workspace.lints]` and the clippy job with
`-D warnings`. No `#[allow]` to get there, per the project rules.
Files: wherever clippy points. Verify: tests, CI. Risk: low to medium; clippy fixes in
`brisk-app` UI code need the smokes of the touched area.

Done on branch `batch1` in one commit: 74 warnings, about half fixed by `cargo clippy --fix`,
the rest by hand (type aliases for complex types, `Box` in two large enum variants,
`sort_by_key`, `if let` instead of `unwrap` after a check). The CI `clippy` job runs with
`-D warnings`, with and without the smoke feature. `[workspace.lints]` with
`clippy::unwrap_used` is not added yet.

### Phase 1: take smokes out of the product

#### Step 4. Add the `smoke` feature and `smoke::schedule`

Done on branch `smoke-feature`. The hooks of `Workspace::new` moved unchanged into
`workspace/smoke.rs` (`schedule`); `crate::smoke` (`smoke.rs`) answers `active()` and
`on(name)` for production code. The 41 smoke files, `win_input.rs`, 55 smoke functions and
70 probe methods in ordinary files are behind `#[cfg(feature = "smoke")]`; the compiler's
dead-code warnings found the probes. `budget.ps1` runs MEMBUDGET on the smoke build
(`-SmokeExe`, default `target\smoke\release\brisk.exe`). Measured: the release exe went from
25.99 to 21.58 MB and contains no `BRISK_SMOKE` string; with a `BRISK_SMOKE_*` variable set
it starts normally. Start-up time is unchanged against `main` on the same machine.

Later the smoke build got its own cargo profile, `smoke`: release without LTO, with 256
incremental codegen units, built to `target\smoke\brisk.exe`. A rebuild after a change takes
about a minute instead of ten.

#### Step 5. Move the smoke files into `workspace/smoke/`

Pure file moves of the 41 `*_smoke.rs` files and `win_input.rs` into `workspace/smoke/`,
then the smoke functions and probes now gated in ordinary files, where they only use public
methods. Files: listed. Verify: tests, all smokes. Risk: low.

Done on branch `smoke-dir` for the files: the 42 smoke files moved to `workspace/smoke/<name>.rs`
without the `_smoke` suffix (`w7_smoke2.rs` became `w7_2.rs`), and `win_input.rs` to
`workspace/smoke/win_input.rs`. `workspace/smoke.rs` declares them, `workspace.rs` no longer
does. Inside the moved files `super::` became `crate::workspace::` and `pub(super)` became
`pub(in crate::workspace)`; nothing else changed. The smoke functions and probes gated inside
ordinary files stay where they are for now: each one needs a check of which module-private
items it reaches, and that is left for later.


### Phase 1b: `brisk-platform`

This comes before the code moves of phase 2, so the code that moves into library crates
already calls the platform API.

#### Step P1. Linux check in CI

Add CI item 6. Where something does not compile on Linux, add the Unix side next to the
Windows code. Do not add a `cfg` at the call site.
Files: the workflow, whatever fails. Verify: the job passes, Windows CI unchanged.
Risk: low.

Started on branch `batch1`: the `linux` job runs `cargo check --workspace --all-targets` and
the tests of every crate but `brisk-app` on `ubuntu-latest`. Nothing here can build for Linux
(no target, no WSL), so the job is `continue-on-error` until its first green run; the fixes
follow from its log.

The first run failed in `xattr` 0.2.3 (through GPUI's HTTP client): it reads `libc::ENOATTR`,
which the `libc` in `Cargo.lock` no longer defines on Linux, and `rustix` keeps `libc` from
going back. `third_party/xattr` is that release with `ENODATA` there, wired in through
`[patch.crates-io]`; it goes once GPUI moves to a fixed `xattr`.

#### Step P2. Create `brisk-platform` from `brisk-proc`

Rename `brisk-proc` to `brisk-platform::process` and add `command()`. Replace the 24
`creation_flags` sites with it, and the `adopt` calls where `command()` now does it.
Files: `crates/brisk-proc`, the 22 files with `creation_flags`, every `Cargo.toml` that
names `brisk-proc`. Verify: tests; RUN, DEBUG, LSP, LINT, GIT smokes; MEMBUDGET reports 0
live children after exit. Risk: medium: a missed `adopt` leaves a child running after a
crash.

Done on branch `batch1`. `crates/brisk-proc` became `crates/brisk-platform` with one module,
`process` (`mod.rs` for the API, `windows.rs` and `unix.rs` behind one `cfg_attr` on the `mod`
line). 21 of the `#[cfg(windows)] { ... creation_flags(...) }` blocks became
`process::no_window(&mut c)`; `process::command(program)` does the same for new code. The
`adopt` calls stay where they are: a `std::process::Command` cannot run code on spawn, so
adopting on spawn needs a wrapper type, left for later. Two Windows-only sites keep their
flag: the `taskkill` in `brisk-terminal` and the PowerShell process list in `debug_ui.rs`.

#### Step P3. `python` and `shell`

Move the venv layout and the default shell choice into the crate. Replace the 47 layout
references and the shell choice in the 9 files.
Files: `brisk-run/src/interp.rs`, `pyproj.rs`, `ssh.rs`, `terminal_env.rs`,
`terminal_view.rs`, `run_anything.rs`, `brisk-lint`, `brisk-lsp`, `brisk-terminal`,
`debug_ui.rs`. Verify: unit tests for both layouts; venv, packages, LSP, RUN, Terminal
smokes; an SSH interpreter still resolves `bin/python`. Risk: medium.

Done on branch `batch2`. `python::Layout` is a value (`Windows` or `Unix`, `Layout::host()` for
this machine), so both layouts are tested on every OS and an SSH interpreter can use the Unix
one on a Windows host. `shell` holds the Terminal's shell list, the default shell,
`command_line` (`%ComSpec% /C` or `/bin/sh -c`) and the `PATH` separator. What is left of the
literals is test data and the `%APPDATA%` tool folders of `pyproj.rs`, which only exist on
Windows.

#### Step P4. `fs`, `dirs` and `fonts`

`replace_atomic` from `editor/disk.rs` (`crate::editor::write_atomic` keeps its name and
calls it). `config_dir` moves out of `brisk-db` into `dirs`, and the 6 direct `APPDATA`
reads go through it. The default fonts move out of `theme.rs` and the font folders out of
`main.rs`.
Verify: tests (atomic write round trip, portable mode); DISK, SETTINGS smokes; budget.
Risk: medium: a wrong settings folder looks like lost settings. Test portable and installed
mode.

Done on branch `batch2`. `fs::replace_atomic` uses `ReplaceFileW` on Windows when the target
exists and falls back to copying the permissions and renaming. `dirs::config_dir` resolves as
before (portable `data`, then `%APPDATA%\Brisk`, `~/Library/Application Support/Brisk` or the
XDG folder); `brisk_db::config::config_dir` re-exports it, and the three `APPDATA` reads that
were left go through `dirs::roaming_app_data`. The path logic takes the environment as a
function, so portable and installed mode are unit tests.

#### Step P5. `secrets`

Move DPAPI into the crate. On other platforms, refuse to save a password without a keyring.
Files: `brisk-db/src/secrets.rs`, the data source dialog. Verify: `cargo test -p brisk-db`
(round trip, damaged file kept). Risk: low on Windows.

#### Step P6. `xtask`

Port `package-portable.ps1` and `budget.ps1` to `cargo xtask`; CI calls the xtask. The
`.ps1` names stay as wrappers until the release checklist and `docs/building.md` are
updated. Verify: the zip matches the script's, file for file, and the budget numbers match.
Risk: medium, since release packaging depends on it. Do it between releases.

### Phase 2: move logic out of `brisk-app`

Each of these is independent of the others. They come after phase 1b, so the code they move
already calls `brisk-platform`, and they touch files that the `Workspace` split does not, so
they can be done in parallel.

#### Step 6. Python environment code to `brisk-run::env`

`py_venv.rs`, `py_manager.rs`, `py_packages.rs`, `terminal_env.rs` with their tests.
Verify: `cargo test -p brisk-run`; smokes for packages, venv, terminal env. Risk: low.

Done on branch `arch1`. The four files moved unchanged, with their tests, to
`brisk-run/src/env/` as `venv.rs`, `manager.rs`, `packages.rs` and `terminal.rs`; brisk-app
calls `brisk_run::env::*` from `venv_ui.rs`, `packages_ui.rs`, `install_offer_ui.rs`,
`terminal_env_ui.rs` and `terminal_view.rs`. The two `crate::smoke::active()` checks (pip
without an index, no PyPI name list from the network) became `env::set_offline`, which
`main.rs` sets from `smoke::active()` at start; `env::pip_offline` is the same check for a
pip started elsewhere (the DataView smoke). The pip command of `venv` and `packages` is one
function, `env::pip_command`. The names cache uses `brisk_platform::dirs::config_dir`. The
tests now also run in the Linux CI job.

#### Step 7. Run outputs to `brisk-run`

`coverage.rs`, `profile.rs`, `run_anything.rs`, the protocol half of `py_console.rs`,
`run_file_stamp`. Verify: `cargo test -p brisk-run`; RUN, RUNANYTHING, PYCON, coverage and
profile smokes. Risk: low.

#### Step 8. DAP helpers to `brisk-dap`

The free functions of `debug_ui.rs` (lines 561–846) and their tests from the test
module at line 4260. Verify: `cargo test -p brisk-dap`; DEBUG smoke. Risk: low.

#### Step 9. Smaller moves

`todo_scan.rs` to `brisk-search`, `editorconfig.rs` to `brisk-project`, the log filter
matching to `brisk-git`. One pull request each. Risk: low.

#### Step 10. `brisk-history` and `brisk-settings`

New crates for `local_history.rs` and for the settings type and its file handling. Add the
tests `settings.rs` lacks: round trip, unknown keys kept, broken file kept as `.bak`.
Verify: new crate tests; SETTINGS and DISK smokes; budget (startup reads settings).
Risk: medium for settings, since a format slip loses user settings; the round-trip tests
are the guard.

#### Step 11. `brisk-app` as library plus binary

Add `src/lib.rs`; `main.rs` keeps only `main`. Verify: tests, budget (no size change
expected). Risk: low.

### Phase 3: registries

#### Step 12. Tool window registry

Introduce `ToolWindowId` and the `ToolWindow` table; replace `LeftTool`, `BottomTool` and
`db.open`; generate the stripes, the render `match`, the toggle arms and `IDE_ACTIONS` from
it. Panels are still drawn by the existing `render_*` methods through the `view` function.
Files: `workspace.rs`, `splitter_ui.rs`, `settings_ui.rs` (saved layout), the files that
call `toggle_bottom` / `toggle_left`. Verify: a unit test that every tool window's toggle
action is in the keymap and in the terminal's pass-through list; UI and STRIPE smokes;
every tool window opened once by hand and by shortcut. Risk: medium; the saved layout in
settings must still load (keep the old names as aliases when reading).

#### Step 13. Action owners

Add `handles` / `enabled` / `run` per area, at first as functions on `Workspace` grouped
by area, and the test that every action has exactly one owner. `dispatch` becomes the
routing described above; menus use `enabled`.
Files: `workspace.rs`, `main_menu.rs`, `menu_ui.rs`, the area files' `*_action` functions.
Verify: the ownership test; smokes; Find Action shows the same enabled state as the menu.
Risk: medium.

#### Step 14. Typed key contexts

A `KeyContext` enum in `brisk-keymap`; `pycharm.rs`, `user.rs` and `Workspace::contexts`
use it; user keymap files keep the string form. Verify: `cargo test -p brisk-keymap`;
KEYMAP smoke. Risk: low.

### Phase 4: split `Workspace`

The pattern for each area is the same, in two pull requests:

1. Move the area's fields into a new entity and keep the methods on `Workspace`, reaching
   the state through the entity. This is mechanical and shows every place that touched the
   state.
2. Move the methods into the entity, replace direct calls from other areas with events or
   with calls through the entity's public methods, and give the tool window its own view.

The order follows the reach measured in problem 1, fewest outside users first, so the
pattern is settled on easy cases.

#### Step 15. `DbModel`

Reached from 3 files (`workspace.rs`, `db_ui.rs`,
`services_ui.rs`). Events: open a grid tab, toast. Verify: SERVICES, DATAVIEW smokes;
`brisk-db` tests. Risk: low.

#### Step 16. `DebugModel`

Reached from 3 files, but `debug_ui.rs` is the largest file:
split it while moving (session, breakpoints, variables, console, views). It needs the run
configuration (from `RunModel` later, from `Workspace` until then) and opens files at
lines. Verify: DEBUG smoke, `brisk-dap` tests. Risk: medium.

#### Step 17. `VcsModel`

Reached from 12 files, all git ones plus `workspace.rs` and
`status_ui.rs`. The Commit and Git windows become views of it. Verify: GIT and VCS smokes (VCS is missing from `testing.md`; document it in step 4),
`brisk-git` tests. Risk: medium.

#### Step 18. `PythonEnv`

Take the interpreter state out of `Runs`. This is the change that
removes most of `runs`' reach (17 files): `lsp_ui`, `problems_ui`, `packages_ui`,
`console_ui`, `venv_ui` read `PythonEnv` instead. Event: `InterpreterChanged`, to which ty,
ruff, packages and the console react. Verify: LSP, INTEL, packages, PYCON, venv smokes.
Risk: high: interpreter selection feeds ty, ruff, runs and debugging; each must be checked
after switching the interpreter.

#### Step 19. `RunModel`

Run configurations, the Run console, Before launch, coverage and
profile. Verify: RUN, RUNWIN, RUNCFG, BEFORE, COMPOUND, RUNTPL, ENVFILE smokes. Risk:
medium.

#### Step 20. `LangModel` and `SearchModel`

`lsp`, `problems`, `symbols`, `nav`; then `find`.
They are used by the editor-facing code in `workspace.rs` and `generate_ui.rs`, so they go
after the tool areas. Verify: LSP, INTEL, COMPLETE, LINT, FIND smokes. Risk: medium.

#### Step 21. The panels created on first open

Packages, Python Console, TODO, Endpoints,
Coverage and Profiler views, one pull request each. Risk: low each.

#### Step 22. What is left of `Workspace`

Split `workspace.rs` into the struct and startup,
keys and dispatch, popups, tabs, and render. Target: no file in `brisk-app` over 1,500
lines, and `Workspace` holding layout, tabs, popups and the sub-model handles. Risk: low;
by now it is file moves.

### Running alongside

- Unwraps: each step replaces the `.unwrap()` calls in the code it moves; the
  `editor/find.rs` case is a separate small pull request with a let-else.
- Each step updates this document where it changes what the code map says.
- `budget.ps1` runs on every step that changes startup (4, 10, 11, 15–22). The split must
  not add work before the first frame: sub-models are created on first use, as the panels
  are today.
