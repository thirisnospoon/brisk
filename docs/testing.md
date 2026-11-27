# Testing

Brisk has two kinds of tests: unit tests in each crate, and smoke tests that drive the real application through environment variables.

## Unit tests

```sh
cargo test --workspace --no-fail-fast
```

Tests against live servers are skipped by default:

```sh
# ty: without BRISK_TEST_TY the test takes ty from PATH, or is skipped
BRISK_TEST_TY=/path/to/ty cargo test -p brisk-lsp

# PostgreSQL and MySQL: ignored tests, run them explicitly
BRISK_TEST_PG_URL=postgresql://postgres:pw@127.0.0.1:5432/postgres \
BRISK_TEST_MYSQL_URL=mysql://root:pw@127.0.0.1:3306/shop \
cargo test -p brisk-db -- --ignored
```

## Smoke tests

A smoke test starts Brisk on a project folder, performs a scripted sequence of actions, prints what it observes to stderr, and quits.

Smokes and the `BRISK_OPEN_*` helpers are compiled only with the `smoke` feature; the release exe has none of them. Build the smoke exe with the `smoke` profile. It is the release build without LTO, so a rebuild after a change takes about a minute, and it lands in `target\smoke`, beside the release exe instead of over it:

```powershell
cargo build --profile smoke -p brisk-app --features smoke
```

To run a smoke, set the variable and pass the project folder as the argument:

```powershell
$env:APPDATA = "$env:TEMP\brisk-smoke-appdata"
$env:BRISK_SMOKE_SCOPE = "1"
target\smoke\brisk.exe C:\tmp\empty-folder
```

- Point `APPDATA` at a temporary folder. Several smokes change settings, local history or run configurations, and a fresh `APPDATA` keeps your own settings out of the result.
- Many smokes write their own files into the folder. Those marked "empty folder" expect one; those marked "copy of the project" should run on a copy of `examples/demo_project` (or another project), not on the original.
- Output lines usually start with the smoke's name, e.g. `smoke-ansi: …`. Most smokes only report; you read the output. Smokes that check results themselves exit with code 1 on failure, as noted below.
- Smokes never touch the network. Under any `BRISK_SMOKE_*` variable pip gets `PIP_NO_INDEX=1` unless `PIP_INDEX_URL` is set, the PyPI name index is read only from a `file://` URL, and no browser is opened.
- Smokes that use real Windows mouse messages say so below; they need Windows.
- Smokes run in the background. On Windows the window is nearly transparent (alpha 1 of 255), behind all other windows, click-through, without focus or a taskbar button, so nothing flashes on the screen. It still draws real frames, and `PrintWindow` captures it as usual (`scripts\ui-shot.ps1`). `BRISK_SMOKE_VISIBLE=1` opens it normally, to watch a smoke or look at a hold.

Arguments:

- `1` runs the smoke.
- A path (`<file>`, `<file.py>`) names the file the smoke opens.
- `=hold` (or a named phase such as `=menu`) runs up to a given point and leaves the window open in that state, usually for screenshots. Some smokes take the hold as a separate `*_HOLD` variable.

## Startup helpers

These are not smokes. They open something at startup, for screenshots and measurements. Like smokes, they run in the background unless `BRISK_SMOKE_VISIBLE=1` is set.

- `BRISK_OPEN_FILE=<file>` opens the file.
- `BRISK_OPEN_SETTINGS=1` opens the Settings dialog.
- `BRISK_OPEN_STRUCTURE=<line>`, together with `BRISK_OPEN_FILE`, opens the Structure tool window with the caret on that line.
- `BRISK_OPEN_TERMINAL=1` opens the terminal.
- `BRISK_TY=<ty.exe>` sets the ty executable explicitly.
- `BRISK_PYPI_INDEX=<url or file>` sets the index for package name suggestions in Python Packages.
- `BRISK_DESIGN=islands|classic` overrides the UI style setting (it can be combined with a palette, e.g. `islands,graphite`).

## Budget probe

Used by `scripts\budget.ps1` (see [building.md](building.md)).

- `BRISK_BUDGET=<report.json>` records named startup moments and when the first window frame was drawn, then, after a period of idle, the working set, and exits. The report is rewritten at each stage so the script can follow it.
- `BRISK_BUDGET_STARTUP_ONLY=1` exits right after the first frame.
- `BRISK_BUDGET_IDLE_MS=<ms>` sets the idle period before the working set is measured (default 5000).
- `BRISK_BUDGET_HOLD=<ms>` keeps the process alive after the measurement so the script can sample child processes (default 1500).

## Editor

### `BRISK_SMOKE_EDIT=<file.py>`

Folder: a copy of the project, with the folder of the file as the root (`examples/demo_project/app`).

Checks Find / Replace, scrolling, mouse drag, the context menu and file operations in the Project tree (a deleted file goes to the Recycle Bin).

### `BRISK_SMOKE_EDIT2=1`

Creates `stage2.py`, `messy.py`, `onsave.py`, `third.py` and `moves.py` in the project. Checks bracket and occurrence highlighting, folding, Reformat Code and Optimize Imports, Actions on Save (the file is saved as typed, then ruff's result is saved from the background, waited for up to 5 s), Code Cleanup, Move Statement, Unwrap/Remove, `Ctrl+W` inside brackets, method navigation, File Structure, bookmarks, extend selection, caret cloning, Paste from History, tabs and split editors.

### `BRISK_SMOKE_W7=1`

Folder: empty, with `APPDATA` in a temporary folder. Writes `a.py` and `sub/b.py`.

Checks, in order:

- Local History: revisions on save, Put Label, a deleted file in the folder history and its restore with `Ctrl+Enter`.
- Copy and paste of lines with several carets, Column Selection Mode (`Alt+Shift+Insert` and `Shift+Arrow`), Replace with In Selection (on by itself for a multi-line selection), Preserve case and regex groups.
- A scratch file (`Ctrl+Alt+Shift+Insert`, the Scratches and Consoles node in the tree), New → Python Package (`pkg.sub` gives two `__init__.py`) and a Python File from the template `<config>/fileTemplates/Python Script.py` (`${NAME}`, `${PROJECT_NAME}`, `${DATE}`, `${TIME}`, `${USER}`).
- Typing: `"""` and `Enter` (docstring), `else:` (dedents to `if`), `Enter` inside a call and inside a string, pasting a block into a class with the indent of the paste position.
- Join Lines (`Ctrl+Shift+J`): `if x:` + `y = 1` → `if x: y = 1`, `('abc'` + `'def')` → `('abcdef')`, a line ending in `\`, two carets.
- `Ctrl+/` on an empty line (`# ` and the caret after it), `Ctrl+Shift+/` in Python (does nothing, as in PyCharm), the right margin (120 or `max_line_length` from `.editorconfig`).
- Back / Forward, Recent Locations (`Ctrl+Shift+E`, pressed again for changed locations only), Last Edit Location (`Ctrl+Shift+Backspace`), Go to Line:Column `3:5`.
- Soft wraps (View → Active Editor → Soft-Wrap, default patterns from Settings → Editor → General → Soft Wraps): `a.py` is not wrapped, `notes.md` with long lines wraps to the window width. `End` / `Home` stop at the visual row edge first, then the logical line. `↑` / `↓` and `Shift+↓` move by visual rows, a click lands on the right visual row, `Ctrl+End` scrolls to the caret on a wrapped line, the action turns wrapping off. Sticky class and method headers show inside a method of a wrapped `wrapsticky.py`.
- Mouse (through the same handlers): double click selects a word, double click and drag extends by words, triple click and drag by lines, `Alt+Shift+Click` adds a caret and removes it on a second click, `Alt+Drag` selects a block.

`BRISK_SMOKE_W7_HOLD=1` turns wrapping back on with a multi-line selection and stays open.

### `BRISK_SMOKE_SCOPE=1`

Folder: empty. Writes `scope.py`.

Checks imports folded on open, Collapse All / Expand All, usage highlighting at the caret (absent after 100 ms, present after 500 ms), Find Usages and Rename without ty (this file only; a local parameter renamed in place by typing) and the message for a module-level name. With ty available the same steps go through ty.

`BRISK_SMOKE_SCOPE_HOLD=1` leaves the window open.

### `BRISK_SMOKE_TEMPLATES=1`

Writes `tpl.py`, `tpl.toml`, `tpl.yaml`, `tpl.json`, `tpl.css`. With real key presses, checks live templates on `Tab`, `Ctrl+J`, Surround With, `Ctrl+/` and `Ctrl+Shift+/`.

### `BRISK_SMOKE_DISK=1`

Folder: a copy of the project.

Checks reload from disk, the conflict with unsaved edits, a new file appearing in the tree, closing the tab of a deleted file, saving in an ANSI encoding, and a binary file.

### `BRISK_SMOKE_BIG=<file.py>`

Opens a large file, waits 6 s, puts the caret in the middle and types a line character by character. Prints the median and maximum key handling time and time to the next frame, the number of problems, the number of lines with inlay hints, and whether folding markers are up to date.

### `BRISK_SMOKE_MOTION=1`

Folder: empty. Writes `motion.py`.

Checks smooth wheel scrolling, `Page Down` / `Page Up` (the caret stays on the same screen row), `Ctrl+End` / `Ctrl+Home`, Go to Line, arrow keys at the edge, smooth scrolling turned off, the caret fade, how many frames one second of idle costs, and smooth caret movement. `=hold` leaves the window open.

### `BRISK_SMOKE_STICKY=1`

Writes `sticky.py`. Checks sticky headers inside a method, on the `def` line, pushing out, at the start of the file, a click on the class header, and the setting turned off. `=hold` leaves the window open with the hovered line highlighted.

### `BRISK_SMOKE_WHITESPACE=1`

Writes `ws.py` and `tabs.mk`. Turns on Show Whitespaces through the action, as Find Action does, and checks the per-line cache, that another editor is not affected, and the default from Settings. `=hold` for a screenshot.

### `BRISK_SMOKE_PALETTE=1`

Writes `palette.py`. Prints the number of search matches, the block indent guide and the caret marks. `=hold` leaves a three-line selection, search matches and the guide of the caret's block on screen. `=caret` leaves the caret at a bracket with its match highlighted.

## Mouse and scrollbars

These send real Windows mouse messages.

### `BRISK_SMOKE_MOUSE=1`

Folder: empty. Writes a 400-line `mouse.py`.

Checks the scrollbar (hover on and off the thumb, a click on the track scrolls a page, dragging the thumb) and middle-button autoscroll (a 3 px move does not scroll, 100 px scrolls with the anchor icon, scrolling stops after release). Then a middle click without movement on `target()` goes to its declaration.

`BRISK_SMOKE_MOUSE_HOLD=hover|drag|middle` stops in that state.

### `BRISK_SMOKE_GUTTER=1`

Folder: empty. Writes `gutter.py`.

Checks the mouse cursor over the editor gutter: a hand over a line number (with the faint preview of a breakpoint), over a breakpoint (with a tooltip showing the line and condition), over a run mark and a fold marker; an arrow over the rest of the gutter; an I-beam over text.

`BRISK_SMOKE_GUTTER_HOLD=preview|tip` stops on the breakpoint preview or the tooltip.

### `BRISK_SMOKE_BARS=1`

Folder: a folder with a couple of hundred files. Writes `out.py`, which prints 500 lines.

Checks the scrollbars of the Project tree and the Run console: hover, a click on the track, dragging the thumb and releasing. The tree selection does not change. In Run, a click above the thumb stops following output and dragging to the bottom resumes it.

`BRISK_SMOKE_BARS_HOLD=tree|run` stops with the thumb being dragged.

### `BRISK_SMOKE_UI=1`

Clicks the Search Everywhere tabs and outside the popup, prints the classes and symbols found and the project menu, then clicks the project widget.

### `BRISK_SMOKE_ACTIONS=1`

Folder: empty. Writes `a.py` and `b.py`.

Checks keys and menu entries that go through the keymap: `Enter` and `F4` on a file in the Project view open it, `Shift+F6`, `F5`, `F6` and `Delete` there start Rename, Copy, Move and Delete (not on the project root), `Alt+Insert` with no row selected and File → New… show the New menu, Copy Path (`Ctrl+Shift+C`) in the editor puts the file's path on the clipboard, Reload All from Disk (`Ctrl+Alt+Y`) picks up an outside edit before the disk poll does, and Tools lists Python Console and Python Packages. Then a right click below the last row of the Project view (a real Windows mouse message) opens the project root's menu, with the shortcuts from the keymap. Exits with code 1 on a failure.

## Navigation and search

### `BRISK_SMOKE_FIND=<text>`

Searches the project for the text, prints the time to stderr (debug build) and opens the Find tool window.

## Code insight

### `BRISK_SMOKE_LINT=<file.py>`

Opens the file, prints ruff's problems, presses `F2` and `Alt+Enter`, applies the first fix and checks the project, then folds and unfolds the file rows of Problems › Project. Does not exit by itself.

### `BRISK_SMOKE_LSP=<main.py>`

Folder: a project with `pkg/util.py` containing `def add(a: int, b: int) -> int` and a `main.py` whose fourth line is `print(add(3, 4))`.

Checks ty: diagnostics, `Ctrl+B` and Back / Forward, `Ctrl+Q`, completion after `pkg.util.`, Parameter Info, `Alt+F7`, and renaming `add` to `plus`.

### `BRISK_SMOKE_INTEL=1`

Folder: a copy of the project. Needs ty. Writes `intel.py`, `smokepkg/` (renamed to `smokelib/` during the run) and `uses_util.py`.

Checks `Alt+Enter` → Import and its undo, Optimize Imports, auto-import in completion (typing `OrderedD` character by character) and its one-step undo, the documentation of the selected item, and `Ctrl+Q` on `defaultdict` (no leftover Markdown escapes). Then renames a module and the package folder (`smokepkg` → `smokelib`), with imports updated inside and outside the package, and checks with ty that a file still using the old package name gets an import error and the updated files do not. At the end it checks the Find Usages tree (groups, files, collapsing with `←`) and inlay hints in `hints.py`.

`BRISK_SMOKE_INTEL_HOLD` stays open in a given state:

- `1`: `Ctrl+Q` on `defaultdict`.
- `dumps`: `Ctrl+Q` on `json.dumps`.
- `list`: the completion list with its documentation.
- `usages`: the Find Usages tree for `add`.
- `inlay`: the end of the run, with inlay hints.

### `BRISK_SMOKE_COMPLETE=1`

Folder: empty. Writes `comp.py`.

Types `co`, `re`, `order.` and `val` + `Enter` character by character and prints the completion lists. Checks postfix `.if`, `.par`, `.print`, the `ifn` and `tryexcept` templates on `Tab`, the `Ctrl+J` list, `Ctrl+P` and Quick Definition (`Ctrl+Shift+I`). Then Extract Variable (`Ctrl+Alt+V`) with the "Multiple occurrences found" popup, replacing both occurrences and renaming by typing, and Surround With (`Ctrl+Alt+T`) → try / except on two lines. Finally it middle-clicks `compute_total` with a real mouse message and prints where the caret went.

With `BRISK_TY` pointing at a file that does not exist, and no ty in the interpreter or on `PATH`, it checks the behaviour without ty (file words, keywords, the notification). Otherwise it runs with ty.

`BRISK_SMOKE_COMPLETE_HOLD=quickdef` stays open with Quick Definition shown.

### `BRISK_SMOKE_STRIPE=1`

Writes `stripe.py`. Needs ruff. Prints the error stripe marks, the search marks, how many times the marks were rebuilt during 1.5 s of idle (expected 0), the tooltip over an error mark and a TODO mark, and where a click on a mark leads. `=hold` leaves the tooltip on screen.

### `BRISK_SMOKE_SQUIGGLE=1`

Writes `squiggle.py`. Needs ruff. Prints which columns of each line get a wavy underline (a multi-line `RUF023` starts at the first non-blank character of each line) and adds a struck-through `deprecated` the way ty reports it. `=hold` for screenshots.

## Run

### `BRISK_SMOKE_RUN=<file.py>`

Runs the file as `Ctrl+Shift+F10` does, prints the interpreter and the Run window output, and follows the first traceback link. For a test file it prints the test tree and opens the source of the first failed test.

`BRISK_SMOKE_RERUN_FAILED=1`, together with it, also reruns the failed tests.

### `BRISK_SMOKE_RUNCFG=1`

Opens Run/Debug Configurations, enters a module configuration, copies it as pytest, reorders, checks the empty-name error, saves and rereads `.brisk/run.json`, then checks that `Esc` discards edits.

### `BRISK_SMOKE_RUNWIN=1`

Folder: empty. Writes `ask.py` (with `input()`), `main.py`, `sleep.py` and `w1.py` to `w5.py`.

Checks input in the Run console, saving before a run, the five-tab limit, `Ctrl+F5`, Soft-Wrap, Scroll to End, Clear All, and saving a temporary configuration.

`BRISK_SMOKE_RUNWIN_HOLD=1` leaves the window open.

### `BRISK_SMOKE_RUNANYTHING=1`

Folder: empty. Writes `hello.py`, `test_ra.py` and a configuration `Hello cfg`.

Opens Run Anything with double `Ctrl` and runs `python hello.py a "b c"`, `pytest -q test_ra.py`, `pip --version`, `echo brisk-%OS%` and `Hello cfg`, checking each tab's output and the history, and that `Ctrl, Ctrl+↑` does not open Run Anything. `=hold` leaves the list open.

### `BRISK_SMOKE_ENVFILE=1`

Folder: empty. Writes `show_env.py` and `.env`.

Enters a configuration with Environment `B=explicit` and `.env` in Paths to .env files and runs it (expected output `A=from-file B=explicit C=from-file-x`). Checks the debugger's launch environment, and that a missing `.env` stops the launch with a message.

### `BRISK_SMOKE_COMPOUND=1`

Folder: empty. Writes `server.py`, `client.py` and their configurations.

Creates a Compound `Both` in the dialog, checks that an unknown name is rejected, runs it (two Run tabs) and checks the message when debugging it.

### `BRISK_SMOKE_BEFORE=1`

Folder: empty. Writes `prep.py`, `app.py`, `bad.py` and their configurations.

Checks that `app` starts after `prep` and the command (it sees their files), that Rerun runs Before launch again, that `bad` with a failing command does not start, and that a Compound in Before launch is rejected.

### `BRISK_SMOKE_RUNTPL=1`

Folder: empty. Writes `show.py` and `sub/`.

Sets working directory `sub` and `TPL=1` in the Python template and saves. Checks a new configuration from `+` (uses the template), a new pytest configuration (does not), and a temporary one from `Ctrl+Shift+F10` (output `cwd sub TPL 1`). `=dialog` leaves the template open.

### `BRISK_SMOKE_PYCON=1`

Folder: empty. Writes `con.py` and `tty.py`.

Checks the `>>>` prompt after a script run with Run with Python Console (`x + 1` → `42`, `exit()`), and `tty.py` through pipes and with Emulate terminal: `isatty False` / `True`, a progress line, and exit code 3 from a process that exits immediately.

### `BRISK_SMOKE_MULTI=1`

Folder: empty. Writes `srv.py`.

Checks Show in Services, the Stop and Rerun / Cancel question on a second run, two parallel tabs with Allow multiple instances, console focus, and Don't show. `=dialog` leaves the dialog with these fields open.

### `BRISK_SMOKE_NODE=1`

Folder: empty. Needs Node.js and npm on PATH. Writes `package.json` with the scripts `hello` and `greet`, `hello.js` and `greet.ts`.

Checks the ▶ marks on the script lines, that a script runs through npm with the configuration's variable, that `greet.ts` runs with node, that both configurations are saved, that Debug on an npm configuration shows that it is not available, that Go to Class and Go to Symbol find `UserRepo`, `makeRepo` and `UserRepo.save` from `repo.ts`, that `Ctrl+B` in `main.ts` goes from `'./repo'` to `repo.ts` and from the imported `makeRepo` to its declaration, and that typing `import z from './` lists `repo` and `greet` without extensions. In `index.html` it types `<section class="a">` and checks the closing tag, that typing in the opening tag's name renames the closing one and Backspace renames it back, and that `ul>li.item$*2` and `Tab` expand to a list. In `style.css` it checks the swatch on the `color: #ff0000` line, and that typing `backgr` lists `background-color` and `background-color: rebecc` lists `rebeccapurple`.

### `BRISK_SMOKE_TS=1`

Folder: empty. Needs `BRISK_TSC` set to TypeScript 7's native `tsc` (the `lib/tsc.exe` of the `@typescript/typescript-<platform>` npm package). Writes `tsconfig.json`, `user.ts` and `main.ts`.

Checks that the TypeScript server starts, that the type error in `u.greet('x')` shows in the editor, that typing `u.` lists `greet`, that `Ctrl+B` on `User` goes to `user.ts`, and that Quick Documentation on `greet` shows its signature.

The `brisk-lsp` test `real_typescript` runs against the same server when `BRISK_TEST_TSC` is set, with a line holding an emoji to check the UTF-16 position conversion.

### `BRISK_SMOKE_PROBE=ts` / `BRISK_SMOKE_PROBE=stage1`

Folder: empty. A broad probe of the JS / TS / HTML / CSS features: `ts` needs `BRISK_TSC` and covers the server (diagnostics while typing, completion, auto-import, Ctrl+B, Find Usages, Rename across files, Parameter Info, Quick Documentation, positions after emoji); `stage1` runs without TypeScript (point `APPDATA` at an empty folder so a downloaded server is not found) and covers Brisk's own support (paths, tags, Emmet, colour swatches, CSS completion, local Find Usages and Rename). Every scenario logs `ok` or `FAIL` and the probe goes on; it exits 1 if any failed.

### `BRISK_SMOKE_REFORMAT=1`

Folder: empty. Writes minified JSON, CSS, HTML, SQL and TOML plus a broken JSON file, runs Reformat Code on each and checks the result, that a second run changes nothing, and that the broken file stays as it is. With `BRISK_TSC` set, also formats `m.ts` through TypeScript's server.

### `BRISK_SMOKE_PICKER=1`

Folder: empty. Writes `picker/alpha/.git`, `picker/beta` and `picker/.hidden`.

Checks the folder browser of Open… and New Project…: the listing (the hidden folder left out, `alpha` marked as a project), the places, that Down and Right step into `alpha` and Backspace comes back with it selected, that Ctrl+H shows `.hidden`, and that New Project creates the typed folder and asks This Window / New Window. With `BRISK_TOUR_SYNC=<dir>` it asks for screenshots the way the tour does.

### `BRISK_SMOKE_UPDATE=1`

Folder: any. The exe must run from a copy of a portable install (a `portable` file next to it), and `BRISK_UPDATE_FEED` must point at a release JSON (`file:///…/feed.json`, GitHub's `/releases/latest` format) whose `.zip` asset is a `file://` URL. Without the feed the smoke never reaches the network: the check stays off under smokes.

Checks the update flow end to end: the startup check shows the balloon for the newer version, the dialog parses the release notes, Update downloads the zip, swaps its files into the install folder and reaches Restart Now, and the running `brisk.exe` was renamed to `brisk.exe.brisk-old`. `=balloon`, `=dialog` and `=ready` stop at those points, for screenshots. Exits with code 1 on a failure.

`cargo test -p brisk-update -- --ignored` asks the real GitHub for the latest release and the one of the running version.

### `BRISK_SMOKE_PIN=1`

Folder: empty. Writes `a.py` and `big.py`.

Pins the `a` tab with a real click, checks that a second run opens a second tab and that Rerun in the pinned tab stays in it. Then prints 150,000 lines and checks that the last 20,000 remain. `=hold` leaves the window open.

### `BRISK_SMOKE_SPACE=1`

Folder: empty.

Types `12 34` with real key presses (posted WM_KEYDOWN, WM_CHAR and WM_KEYUP) into the Search Everywhere field and into a Terminal shell, and checks that the space arrives in both. gpui on Windows gives Space no text, so views that read keys themselves used to drop it. Only digits are typed: letters would follow the active keyboard layout.

### `BRISK_SMOKE_SERVICES=1`

Folder: empty. Writes `long.py`, `quick.py` and `fail.py`.

Runs the three scripts, opens Services (`Alt+8`), checks the statuses and `↑`, then with real mouse clicks presses Stop on `long` and Rerun on `fail` (a second run of `fail` is expected). `=hold` leaves the window open.

### `BRISK_SMOKE_CONSOLEFIND=1`

Folder: empty. Writes `out.py`.

Checks `Ctrl+F` in the Run console: the counter, `Enter` / `Shift+Enter` / `F3`, `Alt+C`, Filter and a click on a line, a match beyond the right edge with Soft-Wrap off, a match split by wrapping with Soft-Wrap on, and `Esc`. `=hold` leaves the search open.

### `BRISK_SMOKE_ANSI=1`

Checks 256-colour and truecolor output, OSC 8 hyperlinks and `x.py:N:M` links in Run output.

### `BRISK_SMOKE_WEB=1`

Folder: empty. Needs fastapi, uvicorn and flask.

Checks app detection in the dialog, runs `api/main.py` as FastAPI and `webapp.py` as Flask on ports 18765 and 18766, checks the HTTP response at the address printed in the output and opening the link. Then runs `site/manage.py` as Django server on port 18767 and Django tests with the label `polls`, against a stub `django` package the smoke writes next to it (Django itself is not needed). Stops the servers at the end. `=dialog` leaves the dialog open, `=dialog-add` the template list.

### `BRISK_SMOKE_TOUR=1`

Not a check: opens the main windows and popups one by one (editor, Search Everywhere, Settings, Commit, Git log, Run, the Python tool windows, Run Configurations, Find in Files, Debug, the light theme) and prints `tour: <n> <name>` for each, for UI review screenshots. With `BRISK_TOUR_SYNC=<dir>` it writes `<n>.ready` there and waits for `<n>.done` (up to 15 s) before the next step, so a screenshot script can keep up. Run it on a git copy of `examples/demo_project` with `app/main.py`.

### `BRISK_SMOKE_KEYMAP=1`

Settings folder: a scratch `APPDATA` (the smoke writes `keymap.json` and a fake PyCharm settings folder there). Ty is not needed.

Filters Keymap by `ctrl+d`, opens Add Keyboard Shortcut for Delete Line, presses `Ctrl+D` and `Enter` through `on_key_down`, and checks the conflict with Duplicate Line, the saved `keymap.json` and that `Ctrl+D` now deletes a line in the editor. Then imports a PyCharm keymap (one known action, one unknown) and checks `Ctrl+Alt+F`, and ends with Reset All. `=hold` stops with the shortcut box open, for a screenshot.

### `BRISK_SMOKE_MARKS=1`

Folder: empty. Needs ty.

Writes `backend/app/__init__.py` and `main.py` with `import app`. Checks that ty reports the import as not found, marks `backend` as a sources root, checks that ty now resolves it (the type error in `main.py` is reported instead) and that running `main.py` prints `hello 42`. Then marks `data` excluded and checks the tree, the file index and `.brisk/structure.json`.

### `BRISK_SMOKE_SSH=<host>`

Folder: empty. Needs a host reachable with `ssh <host>` without a password, with Python 3.

Makes the host's first Python the project interpreter, runs `app.py` (it imports `pkg/util.py` and raises), checks the output and that the first traceback link opens the local `app.py`, edits it and runs again (one file uploaded), then removes `~/brisk-projects/<folder>` on the host. `cargo test -p brisk-run ssh -- --ignored` with `BRISK_SSH_TEST_HOST=<host>` checks the upload and a run without the UI.

### `BRISK_SMOKE_WEBNAV=1`

Folder: empty. Writes a small Django project (`mysite`, app `polls` with templates).

Checks `Ctrl+B` on a template name in a view (expected `detail.html`), `Ctrl+Click` on `{% extends 'base.html' %}` (`base.html`) and on `{% url 'polls:detail' %}` (`urls.py:8`), Go to Related from `detail.html` to its view (`views.py:9`) and back, the Endpoints list (`/polls/`, `/polls/<int:pk>/`) and `Enter` on an endpoint (`views.py:8`, `def detail`). `=hold` leaves the window open.

### `BRISK_SMOKE_COVERAGE=1`

Folder: empty. Writes `calc.py` and `test_calc.py`.

Runs the script and pytest with coverage and checks the percentages in the Coverage window, the gutter marks, `Enter` navigation, marks disappearing after an edit, and clearing. `=hold` leaves the window open. `=offer` shows the offer to install coverage.py (needs an interpreter without it).

### `BRISK_SMOKE_PROFILE=1`

Folder: empty. Writes `prof.py`.

Profiles it, prints the top of the table by Own Time, sorts by Call Count and Name with real clicks, and opens `fib` with `Enter` (expected `prof.py`, line 10). `=hold` leaves the Profiler window open.

### `BRISK_SMOKE_LATENCY=1`

Prints the time from `Shift+F10` to process start and to the first line of output. `=pytest` does the same for a pytest run.

### `BRISK_SMOKE_FLOOD=1`

Floods output for 4 s while typing in the editor and prints UI thread stalls, key handling time and the number of redraws. `=pty` uses Emulate terminal. `=find` does the same with the console search open and also prints the search time over 20,000 lines.

## Testing

### `BRISK_SMOKE_RUNMARKS=1`

Folder: empty. Writes `app.py` with a `__main__` block and `test_calc.py`.

Opens the Run / Debug menu by clicking the gutter run marks, runs the script and a test class, checks the pass / fail marks, and checks that a click on a line number sets a breakpoint instead of opening the menu. `=menu` stops on the menu, `=hold` leaves the window open.

### `BRISK_SMOKE_TESTVIEW=1`

Checks the test tree toggles, sorting, Rerun Failed Tests First, history and export. `=shot` leaves the window open.

### `BRISK_SMOKE_TESTRUNNERS=1`

Checks the run button on a parameter set (in the tree and in the gutter menu), Default test runner = Unittests, and a doctest run.

## Debug

### `BRISK_SMOKE_DEBUG=<file.py>`

Folder: a copy of `examples/debug_demo`, file `dbg_main.py`.

Sets a breakpoint, starts debugging and checks:

- the stack, variables, Set Value and Copy Value;
- Watches (add, edit, remove, re-evaluation after a step, persistence);
- evaluation, and `Tab` completion in the console (`items.co` → `copy` / `count`);
- `F7` / `Shift+F8` / `F8` (after `Shift+F8`, `↩ add()` in Locals), a conditional breakpoint, Run to Cursor, finishing the session;
- Pause and Rerun on an infinite loop, Stop while debugpy is starting;
- stopping on an exception, with the header and traceback links; inline values;
- stopping in a worker thread and switching to MainThread (`dbg_threads.py`);
- Smart Step Into the outer call of `a(b(3))` (`dbg_smart.py`);
- a breakpoint on class `ValueError`: a caught `KeyError` does not stop, a caught `ValueError` does (`dbg_exc_class.py`);
- Jump to Cursor backwards inside a function, and the refusal to jump out of it (`dbg_jump.py`);
- Attach to Process on a running `dbg_attach_target.py`: a breakpoint in its loop, the process survives Stop, Rerun attaches again, and after the process exits a message is shown;
- the Breakpoints window: a disabled breakpoint does not stop, On raised catches a caught exception, conditions, removal;
- debugging pytest (one test and the whole file, stopping in a test, result `passed`).

### `BRISK_SMOKE_DEBUGPY_OFFER=1`

Starts debugging with an interpreter that has no debugpy, from an exe with no bundled copy (`lib\debugpy` next to it or `vendor\debugpy-lib` above it), and checks the offer to install debugpy, then presses Install; pip runs offline, so the check is that its error is shown. Prints `SKIP` when debugpy is found. `=offer` leaves the offer open.

### `BRISK_SMOKE_DEBUG_HOLD=1`

Stays at the first stop, adds a watch, and prints every state change to stderr (`smoke-hold: …`). `scripts\debug-mouse-smoke.ps1` starts Brisk this way and drives its window with real Windows mouse messages (line numbers, right click on a breakpoint, variable rows, toolbar buttons, hover, drag, keys with modifiers), capturing the window with `PrintWindow`.

### `BRISK_SMOKE_DATAVIEW=1`

Self-contained. Creates a venv with pandas and numpy (and debugpy, if the build has no bundled copy) in `%TEMP%\brisk-smoke-dataview`, or in `BRISK_SMOKE_DATAVIEW_DIR`, and reuses it on later runs. Writes `frames.py` there and debugs it with that interpreter. Without Python, network access or wheels it prints `SKIP` and exits with 0. Any failed check prints `FAIL` and exits with 1.

`BRISK_SMOKE_DATAVIEW=<file.py>` uses the project interpreter instead, with the project path as the argument.

Stops on the line marked `# BREAK`, opens View as DataFrame, sorts, filters, formats, scrolls a 1M-row frame to the middle, opens View as Array, and checks Copy Value of a long string, Copy as Expression, Add to Watches, View Value, and table reloads after steps.

- `BRISK_SMOKE_DATAVIEW_DIR=<folder>`: where the venv and `frames.py` live.
- `BRISK_SMOKE_DATAVIEW_HOLD=1`: stays on the table.

## Python console and packages

### `BRISK_SMOKE_PYCONSOLE=1`

Opens Python Console and enters an expression, a multi-line function, a loop with `Shift+Enter`, an error, `input()`, and an infinite loop interrupted with `Ctrl+C`. Browses history, then restarts and stops the console. `=hold` leaves the window open.

### `BRISK_SMOKE_PYPROJ=uv` / `BRISK_SMOKE_PYPROJ=poetry`

Folder: empty. Needs uv or Poetry on `PATH`. Writes `pyproject.toml` and local wheels in `wheels/`, and keeps uv offline (`UV_NO_INDEX`, `UV_FIND_LINKS`) and Poetry's environments in a `<folder>-poetry-cache` folder next to it.

uv: syncs the environment from Add Interpreter, adds and removes `briskdemo` through Python Packages (checking `pyproject.toml`), checks that an unrelated edit of `pyproject.toml` offers nothing and a new dependency offers a sync, and runs it from the balloon. Poetry: `poetry install` creates an environment outside the project, which becomes the project interpreter and which Python Packages recognizes as the project's.

### `BRISK_SMOKE_PACKAGES=1`

Folder: empty. Creates `.venv` with the first Python it finds and local wheels in `wheels/`; does not use the network.

Shows the list, presses `Down` in the filter field and checks that the selection stays, installs `briskdemo==1.0` before the packaging tool is detected (the operation waits in the queue), filters, and uninstalls the package through the confirmation. `=hold` stops on the confirmation.

### `BRISK_SMOKE_PKGQUEUE=1`

Folder: empty. Creates its own `.venv` with briskdemo 1.0, local wheels and a local index.

Checks outdated detection, name suggestions, queued install and upgrade, the working set before, during and after suggestions, and the search time over a PyPI-sized name list (944,000 names).

### `BRISK_SMOKE_VENV=1`

Folder: no venv, with a `requirements.txt`. Offline: set `PIP_NO_INDEX=1` and `PIP_FIND_LINKS=<folder with the briskdemo-1.0 wheel>`.

Chooses Add Interpreter… and the first base Python, waits for `.venv`, checks the project interpreter and the offer to install requirements, then clicks Install requirements with a real mouse click. When the offer is shown again it clicks Not now. `=hold` stops on the offer.

### `BRISK_SMOKE_TERMVENV=1`

Folder: no interpreter set in `.brisk/run.json`. `.venv` is created with the first base Python if missing.

Opens Terminal and checks the `(.venv) PS` prompt, `sys.executable` and `VIRTUAL_ENV`. Then switches the interpreter to the system one and checks that a new tab is not activated. `=hold` leaves the terminal open.

## Version control

### `BRISK_SMOKE_GIT=<file>`

Opens the file, the Commit and Git windows, a diff and the branches popup.

### `BRISK_SMOKE_VCS=1`

Folder: empty only. Creates a repository in it and commits.

Checks the diff viewer (side by side and unified, live while the file changes) and the merge window, then Annotate and Annotate Previous Revision, a change marker's popup, a commit that leaves one diff block out, and a commit's menu in the Git log.

`BRISK_SMOKE_VCS_HOLD=diff|unified|merge|merged|annotate|previous|popup|partial|log` stops at that phase.

It prints a `smoke-vcs: …` line per check, `smoke-vcs: FAIL …` for a failed one and `smoke-vcs: done` at the end, then quits (with a hold the window stays open). On a folder that is already a repository it prints why and quits without changing anything.

### `BRISK_SMOKE_GITX=1`

Folder: empty only. Creates a repository in it and commits.

Checks Annotate with Git Blame, Conflicts and Merge after a conflicting `git merge`, Stash / Unstash (a file's diff, Apply, and Drop with its confirmation: `Esc` cancels only the question, `Enter` drops), Show History for a file and a selection, Git log filters (including real clicks on the Branch and User filters), Compare with Branch, Show Diff with Working Tree, Rollback of one of two files, Push to a bare repository next to the folder (`<folder>-origin.git`), Fetch and Update Project from a second clone, Merge into Current, Cherry-Pick and Undo Commit with a draft in the Commit window (the draft stays; Undo Commit adds the commit's message after it), Delete and Force Delete of branches. Steps that finish in the background are waited for (up to 5 s each); a step that never finishes prints `FAIL timed out …`.

`BRISK_SMOKE_GITX_HOLD=annotate|merge|stash|history|log|logmenu|push` stops at that phase.

## Database

### `BRISK_SMOKE_DB=<sql>`

Runs the SQL in the console of the first data source, prints the result and opens the data of the first table. Then folds and unfolds the first data source with real Windows mouse clicks on its arrow. `examples/demo_project` has a SQLite data source `shop.db` (see [building.md](building.md)).

## Settings, history and TODO

### `BRISK_SMOKE_SETTINGS=1`

Checks the Settings dialog (live preview, `Esc`, OK and saving), font zoom (kept after an edit and Cancel), `.editorconfig` (creates and deletes it in the project root, writes `ec_smoke/ec.py` and `tabs.mk`) and Local History, then restores default settings.

### `BRISK_SMOKE_SETTINGS_HOLD=1`

Leaves the Settings dialog open.

### `BRISK_SMOKE_TODO=1`

Folder: empty. Writes `app.py`, `pkg/`, `notes.md` and an ignored `build/`.

Checks the TODO list, adding a TODO and saving, `Enter` navigation and collapsing. `=hold` leaves the window open.

## Memory

### `BRISK_SMOKE_MEMBUDGET=1`

Folder: a copy of the project. Writes `mb_flood.py` and `mb_tabs/`.

Runs a script that prints 200,000 lines, opens and closes 50 tabs, runs Find in Files, and prints the working set, private bytes and the number of live child processes. Exits with code 1 if the working set is over budget (250 MB by default, `=<MB>` sets another). `=hold` leaves the window open.
