# Features

This page lists what works today. Brisk follows PyCharm's layout, keymap and behaviour, so action names and shortcuts below are PyCharm's unless noted.

## Interface

### Layout

- The window layout follows PyCharm's New UI: a toolbar with the project, the Git branch (read from `.git/HEAD`) and the run configuration; tool window stripes on the left and right; a status bar with `Ln:Col`, line separator (LF/CRLF), encoding, indent, language and interpreter.
- On Windows the toolbar is also the title bar: its empty space drags the window (double click maximizes) and it ends with the minimize, maximize and close buttons.
- Popups, menus and toasts fade in and rise the last few pixels (120 to 200 ms); modal dialogs also dim the window behind them. A tool window slides the last 14 px in from its edge while the editor takes its new size at once. Nothing waits for an animation: keys go to the new window from the first frame. Settings → Appearance → Animations turns them off.
- Status bar messages fade out after about 6 s. A click on the message area opens Recent Messages, the last messages with how long ago they came. The language is not repeated when it is just "Python".
- The project root in Project shows its folder next to the name, shortened (`~\…\work`); the full path is in the tooltip.
- Tool windows resize by dragging their edge. In Islands the gap between a tool window and the editor is the splitter, in Classic it is a line. The width of the left and right tool windows and the height of each bottom one are saved per project.
- All tool window headers have the same height, with the title on the left and 24 px buttons on the right.
- The active editor tab is underlined. The close button on inactive tabs shows on hover.
- Popups close on a click outside them, and that click does not reach the element underneath. The data source dialog is modal: an outside click does not close it, `Esc` does.
- Keyboard focus looks the same everywhere: a 2 px accent ring that follows the element's corner radius (inputs, buttons, toggles, checkboxes). In Settings, `Tab` / `Shift+Tab` move focus, `Space` presses the focused element, `Enter` is OK (or Cancel when Cancel has focus).
- Shortcuts also work with the Russian keyboard layout.

### UI style: Islands and Classic

- Settings → Appearance → UI style switches between Classic and Islands. The change applies immediately and is saved in `settings.json`.
- Islands: tool windows and the editor are rounded islands (radius 8, gap 6 px) on a slightly darker frame. Tabs are pills. Each group of a split editor has its own island. The file path is shown as breadcrumbs in the editor island header. Active stripe buttons are pills. Popups, menus and dialogs have radius 10 and a soft shadow.
- `BRISK_DESIGN=islands` or `BRISK_DESIGN=classic` overrides the setting. It can be combined with a palette, e.g. `islands,graphite`.

### Scaling

- Settings → Appearance → UI font size (11 to 16, default 13). Row heights, tabs, headers, the status bar, inputs and icons scale with it.
- View → Zoom IDE: `Ctrl+Alt+=` / `Ctrl+Alt+-` / `Ctrl+Alt+0` (also `Ctrl+Alt+Shift+=` / `-` / `0`), in 10 % steps. The zoom is saved.
- The editor font has its own size and is not affected by either.

### Icons

- Icons are SVG files embedded in the executable (`crates/brisk-app/assets/icons`, about 13 KB). General-purpose icons come from [Lucide](https://lucide.dev) (ISC license). Tool window, Run / Debug toolbar, file type and folder icons are Brisk's own, drawn in a matching line style. `LICENSE-lucide.txt` in the same folder lists which is which.
- Sizes: 16 px in toolbars and tool window stripes, 12 to 14 px in the gutter. Breakpoint circles and gutter icons snap to the physical pixel grid, so their edges stay sharp at any Windows scale.
- File type icons (16 px): Python, stubs, Jupyter, JSON, TOML, YAML, ini / cfg / `.env`, Markdown, text, requirements, Rust, JS / TS, HTML, CSS / SCSS, SQL, shell / bat / PowerShell, XML, CSV, lock files, `.gitignore` / `.dockerignore`, Dockerfile, images, archives. Folders: plain, sources (`src`), tests (`tests`), excluded (`.venv`, `node_modules`, `__pycache__`, `build`, `dist` and similar). The same icons are used in Project, tabs, Commit, Log, Find, Recent Files, Search Everywhere and breadcrumbs.
- Tool window and Run / Debug toolbar icons use the same line style (24×24 grid, stroke 1.6) and scale with the UI.
- Differences from PyCharm: icons and graphics are Brisk's own, not JetBrains'. Layout, shortcuts and behaviour are the same.

### Projects

- Clicking the project name in the toolbar opens a menu with New Project…, Open… (pick a folder, then This Window or New Window) and recent projects (they open in a new window).
- Open… and New Project… show a folder browser: places on the left (Home, Desktop, Documents, Downloads, the drives, recent projects), the folder's subfolders on the right (projects marked), an editable path above them. Arrows move, `→` or a double click steps into a folder, `←` / `Backspace` goes up, `Ctrl+H` shows hidden folders, `Enter` opens the selected folder (or the one shown). New Project adds a Name field and creates that folder in the selected one.
- Started without an argument, Brisk opens the last project.

### Updates

- Once a day Brisk asks GitHub for the latest release. A newer one shows a balloon with What's New and Update; the dialog has the version, the release date, the download size and the release notes.
- Update downloads the zip and swaps the new files into the portable install while Brisk runs, then offers Restart Now. The running files are renamed aside and removed at the next start; your `data` folder and other files next to `brisk.exe` stay. If a file cannot be replaced, the ones already replaced are put back.
- Skip This Version hides the balloon until a later release. Settings › Updates turns the daily check off.
- Help › Check for Updates asks at once; Help › What's New in Brisk shows the notes of the version you run.
- A build from source (no `portable` file next to the exe) gets Download, which opens the release page.
- No account and no data is sent: one request to `api.github.com`, through the `curl` that ships with Windows.

## Editor

### Display

- Syntax highlighting by tree-sitter, computed only for the visible range: Python, JSON, TOML, YAML, Markdown, HTML, CSS, JavaScript, TypeScript / TSX, shell. SQL uses its own lexer.
- Line numbers, indent guides, multiple carets, IME input.
- The default dark colour scheme is Brisk's own, kept close to what PyCharm users are used to so that code reads the same at a glance: keywords orange, strings green, numbers cyan, comments grey, function declarations blue, decorators olive, `self` purple. Calls, attributes and class names use the plain text colour.
- Indent guides follow blocks: continuation lines inside brackets get no guides of their own, guides do not break on empty lines, and the guide of the block with the caret is brighter.
- The selection is drawn as one shape with rounded (3 px) outer corners. Search matches, occurrences of the name under the caret and matching brackets are small rounded boxes inset 1 px from the line edges. All edges are aligned to screen pixels.
- The caret is 2 px wide, pixel-aligned, and blinks with a fade.
- Show Whitespaces (View → Active Editor; default in Settings → Editor → Appearance → Show whitespaces): a dot in the middle of each space and an arrow across each tab, pixel-aligned. It applies to the active editor.
- Sticky lines (Settings → Editor → General → Show sticky lines): up to three enclosing `class` / `def` headers stay pinned above the text and get pushed up when their block ends. Clicking a header jumps to it.
- Soft wraps: View → Active Editor → Soft-Wrap. On by default for `*.md; *.txt; *.rst; *.adoc` (Settings → Editor → General → Soft Wraps). `End` / `Home` go to the edge of the visual row first, then of the logical line. Arrow keys move by visual rows.
- Right margin at column 120, or at `max_line_length` from `.editorconfig`.

### Scrolling and mouse

- Smooth scrolling as in PyCharm (Settings → Editor → General → Enable smooth scrolling): the mouse wheel, `Page Up` / `Page Down`, `Ctrl+Home` / `Ctrl+End`, Go to Line and Find Next animate over 120 to 150 ms. Shifts of one or two lines are immediate, touchpads scroll 1:1.
- `Page Up` / `Page Down` move the text by a page and keep the caret on the same screen row, as in PyCharm.
- Caret blinking and Smooth caret movement (off by default, as in PyCharm) are separate settings.
- Horizontal scrolling with `Shift+Wheel` or a touchpad.
- Scrollbars widen and lighten on hover and drag. A click on the track next to the thumb scrolls one page in that direction. The vertical scrollbar of the editor is also the error stripe (see Code insight).
- The same scrollbars are used in Project, the test tree, Find in Files (window and popup), the Run and Terminal consoles, and Python Console.
- Middle-button autoscroll, as in Windows and PyCharm: press the middle button and move the mouse more than 6 px, and the text scrolls in that direction on both axes, faster the further the mouse is from the anchor. Release to stop. A middle click without movement is Go to Declaration.
- Mouse selection: drag with autoscroll at the edges, double click selects a word (drag to extend by words), triple click selects a line (drag to extend by lines), `Alt+Drag` selects a column, `Alt+Shift+Click` adds a caret (again on the same spot removes it).
- Right click opens the context menu: Context Actions, Go to Declaration, Find Usages, Rename, Cut / Copy / Paste, Find, Replace, Comment, Run, Copy Path, Show in Explorer.

### Find and Replace in a file

- `Ctrl+F` Find and `Ctrl+R` Replace, with Match Case / Words / Regex (`Alt+C` / `Alt+W` / `Alt+X`).
- `Enter` / `Shift+Enter` / `F3` / `Shift+F3` move between matches. A counter shows the position ("3/18"). Matches are highlighted in the text and on the scrollbar.
- Replace, Replace All (`Ctrl+Alt+Enter`), `$1` groups in regex mode, Preserve case. In Selection turns on by itself for a multi-line selection. Regex errors are shown in the field.

### Editing

- `Ctrl+D` Duplicate Line, `Ctrl+Y` Delete Line, `Ctrl+/` Comment with Line Comment, `Alt+Shift+↑/↓` Move Line, `Ctrl+Shift+J` Join Lines, `Ctrl+Shift+U` Toggle Case, `Alt+J` / `Ctrl+Alt+Shift+J` select next / all occurrences, `Shift+Enter` and `Ctrl+Alt+Enter` start a new line below / above, `Tab` / `Shift+Tab` indent, smart `Home`, `Ctrl+Backspace`.
- Join Lines understands Python: `if x:` + `y = 1` becomes `if x: y = 1`, adjacent string literals merge (`('abc'` + `'def')` → `('abcdef')`), and lines ending in a `\` continuation join too. It works with several carets.
- `Ctrl+/` adds `#` in Python, TOML and YAML, `//` in JSON and JS. In CSS, HTML and Markdown each line is wrapped in `/* */` or `<!-- -->`. `Ctrl+Shift+/` puts a block comment around the selection. On an empty line `Ctrl+/` inserts `# ` and puts the caret after it.
- Auto-closing brackets and quotes, smart `Enter` (indent after `:`), smart `Backspace` through indentation. Typing `"""` and `Enter` opens a docstring, `else:` dedents to its `if`, `Enter` inside a call or a string continues it correctly. A pasted block is re-indented to the paste position.
- Copy or cut without a selection takes the whole line. Pasting a whole line inserts it above the current one.
- Copy and paste work per caret with multiple carets. Column Selection Mode: `Alt+Shift+Insert`, then `Shift+Arrow`.
- `Ctrl, Ctrl+↑/↓` (hold the second `Ctrl`) clones the caret up or down.
- `Ctrl+W` / `Ctrl+Shift+W` extend / shrink the selection along the syntax tree: word, expression, bracket contents, brackets, call, block, file.
- `Ctrl+Shift+V` Paste from History: the last 30 copied or pasted fragments, with a filter.
- Undo / redo with typing grouped into steps, up to 1000 steps per file.
- Brace matching at the caret. After the caret rests for 300 ms, other usages of the name under it are highlighted, with marks on the right edge. In Python this respects scopes and does not need ty.
- Code folding: markers in the gutter for classes, functions, blocks, multi-line brackets and import groups (other languages fold by syntax or indentation). `Ctrl+-` / `Ctrl+=` collapse / expand, `Ctrl+Shift+-` / `Ctrl+Shift+=` collapse / expand all. Folds move with edits. When a Python file opens, the first import group is folded to its first line, as in PyCharm.

### Files, saving and encodings

- Files are saved automatically when you switch tabs, leave the window or close it. `Ctrl+S` is Save All. Save errors are reported. A tab with unsaved changes is marked `•` and does not close until the changes are written.
- Files open in their own encoding (UTF-8, UTF-8 with BOM, UTF-16 with BOM, the system ANSI code page such as windows-1251) and are saved in it. If a character does not fit the encoding, the file is saved as UTF-8 with a notification. The encoding is shown in the status bar.
- Binary files and files over 20 MB are not opened as text.
- Changes made on disk (terminal, Git, scripts) are picked up about every 1.5 s. An open file without edits is reloaded (`Ctrl+Z` undoes the reload). With unsaved edits, a banner offers Reload from Disk or Keep Mine, and autosave does not overwrite the file. A file deleted on disk closes its tab, or shows a Save Again banner if it has edits. New and deleted files appear in expanded folders of the Project tree.

### Tabs and splits

- `Alt+←/→` switch tabs, `Ctrl+F4` or the middle button close one. Tabs can be dragged.
- The tab context menu has Close, Close Others, Close All, Close Tabs to the Left / Right. Reopen Closed Tab restores the last closed tab.
- Split Right, Split Down, Move to Opposite Group, Unsplit and switching between groups are available through Find Action. An empty group closes itself.
- Settings → Editor Tabs: tab limit (default 10; the least recently used unpinned tab without edits closes first) and Show tabs in: One row / Multiple rows. When tabs do not fit, a `▾` button lists all tabs of the group with a filter. The active tab is scrolled into view.

### Large files

- After an edit, the syntax tree is reparsed only around the change, so highlighting keeps up in a 50,000-line file.
- In files over 20,000 lines, full reparses, usage highlighting and run marks are computed on a background thread. Until a reparse finishes, the previous highlighting is kept and shifted with the text. The first parse when opening such a file also runs in the background.
- In files over 20,000 lines, inlay hints are off, the longest-line width updates at most once a second, and completion without ty takes words from the 2,000 lines around the caret. A notification says so when the file opens.
- In files from 5,000 lines, folding regions are recomputed in the background 300 ms after the last edit instead of every frame. Fold commands always use fresh regions.

## Navigation and search

- Project (`Alt+1`): lazy tree with folders first. `__pycache__` and `.git` are hidden, `.venv` and `node_modules` are marked excluded. Arrow keys navigate, `Enter` or `F4` opens.
- File operations in Project: `Alt+Insert` New File, Python File, Python Package, Directory (a path like `pkg/mod` creates the folders; Python Package `pkg.sub` creates two `__init__.py`), `Shift+F6` Rename, `F5` Copy, `F6` Move, `Delete` moves to the Recycle Bin after confirmation. The right-click menu has the same items plus Copy Path and Open In → Explorer / Terminal (a terminal tab started in that folder). On a folder it also has Find in Directory… (Find in Files limited to the folder; the In Project chip widens it again) and, for a test folder (marked as Test Sources Root or named `test…`), Run Tests in '<folder>'. Open tabs follow renames and moves.
- Mark Directory as (right click on a folder): Sources Root, Test Sources Root, Excluded, and Unmark. Source and test roots go on ty's import path (`extra-paths`) and on `PYTHONPATH` of runs, tests and SSH runs, so `backend/app` can be imported as `app`. Excluded folders are shown in the excluded colour, like `.venv`, and are left out of Go to File, Search Everywhere, Find in Files, the symbol index, TODO and ty. The marks are saved in `.brisk/structure.json`; the first time, they are read from PyCharm's `.idea/*.iml` if the project has one.
- New Python files use the template `<config>/fileTemplates/Python Script.py` with `${NAME}`, `${PROJECT_NAME}`, `${DATE}`, `${TIME}`, `${USER}`.
- Scratch files: `Ctrl+Alt+Shift+Insert`. They show under Scratches and Consoles in the Project tree.
- Structure (`Alt+7`): classes, functions and methods of the current file. The element containing the caret is highlighted and scrolled into view as the caret moves (like Autoscroll from Source).
- Search Everywhere (`Shift Shift`) with tabs All / Classes / Files / Symbols / Actions. Switch tabs with a click, `Tab` or by repeating the shortcut.
- `Ctrl+N` Go to Class, `Ctrl+Shift+N` Go to File, `Ctrl+Alt+Shift+N` Go to Symbol, `Ctrl+Shift+A` Find Action, `Ctrl+E` Recent Files, `Ctrl+G` Go to Line (accepts `line:column`).
- Find Action matches a whole word or the start of words (`sf` finds Show Files, not every title with an s and an f), and an action reachable from several menus is listed once.
- Files are indexed in the background, respecting `.gitignore`, with fuzzy matching. Classes and functions of all `.py` files, and of JS / TS files (classes, interfaces, enums, functions, methods, `const f = () => …` and top-level constants; minified bundles and `.d.ts` are skipped), are indexed by tree-sitter in the background and updated after each save. Structure, breadcrumbs and sticky lines use the same outline. This does not need ty or a TypeScript server.
- Names match by prefix and by word starts: `fbp` finds `find_by_prefix`, `UR` finds `UserRepository`, `Repo.ad` finds member `ad…` of class `Repo…`. Go to Symbol also lists module-level variables and constants (`MAX_RETRIES`).
- `Ctrl+F12` File Structure (with a filter), `Alt+↑/↓` previous / next method.
- `Ctrl+Alt+←/→` Back / Forward. `Ctrl+Shift+E` Recent Locations (press again for changed locations only). `Ctrl+Shift+Backspace` Last Edit Location.
- Bookmarks: `F11` toggles a bookmark, `Ctrl+F11` adds one with a mnemonic (digit or letter), `Ctrl+<digit>` jumps to it, `Shift+F11` and the Bookmarks tool window (`Alt+2`) list them. Bookmarks move with edits and persist between sessions.

### Find in Files

- `Ctrl+Shift+F` Find in Files and `Ctrl+Shift+R` Replace in Files.
- Searches as you type, on all cores, in project files excluding `.gitignore`d and excluded folders. A new search cancels the previous one.
- Match Case (`Alt+C`), Words (`Alt+W`), Regex (`Alt+X`), File mask (`Alt+M`, e.g. `*.py, !test_*`).
- Text selected in the editor is used as the query.
- A result list with highlighted matches and a preview with context. `↑/↓` move, `Enter` opens the file with the match selected.
- `Ctrl+Enter` moves the results to the Find tool window (`Alt+3`), grouped by file with line numbers.
- Replace All (`Alt+A`) asks for confirmation with a second press. `$1` works in regex mode. In open files the replacement can be undone with `Ctrl+Z`.
- Differences from PyCharm: there is no persistent index. A parallel scan is fast enough for typical projects and uses no memory while idle.

### TODO

- The TODO tool window lists `TODO` and `FIXME` comments in all text files of the project (case-insensitive, whole word; in `.txt`, `.md` and `.rst` any line counts), respecting `.gitignore`, grouped by file.
- Click or `Enter` jumps to the line with the word selected. `←/→` or a click on the file collapses or expands it.
- The list refreshes when the window opens, with the refresh button, and for each saved file.

## Code insight (ty and ruff)

### Inspections (ruff)

- Works like the Ruff plugin for PyCharm. Open Python files are checked as you type (0.3 s after you pause; unsaved text goes to ruff through stdin), with the project's settings from `pyproject.toml` or `ruff.toml`.
- Errors (syntax, unknown name) get a red wavy underline, other problems a yellow one. Unused imports and variables are shown as grey text. Deprecated names (reported by ty) are struck through.
- The wavy line runs only under code: on multi-line problems it starts at the first non-blank character of each line. It is one screen pixel thick and pixel-aligned at 100, 125 and 150 % scale.
- A widget in the top-right corner of the editor shows counts ("✗ 1 ⚠ 3"). Clicking it opens Problems.
- Error stripe on the vertical scrollbar: errors, warnings (unused code as weak warnings), `TODO` / `FIXME`, search matches, changed lines, and the caret line. Clicking a mark moves the caret there. Hovering shows what is there, most important first, up to four lines.
- Hovering a problem shows its message. `F2` / `Shift+F2` go to the next / previous problem (errors first). The problem at the caret is described in the status bar.
- `Alt+Enter` offers ruff's fix for the problem (unsafe fixes are marked), fix all problems of that code in the file (e.g. `F401`), fix all auto-fixable problems in the file, and Suppress for line (`# noqa: CODE`). All of these undo with `Ctrl+Z`.
- Problems (`Alt+6`): the File tab shows the current file, the Project tab runs ruff over the whole project (refreshed with the refresh button and after saves).
- ruff is looked up next to the project interpreter, then next to other detected Pythons, then on `PATH`, then the bundled copy (`lib\ruff.exe` next to `brisk.exe`, or `vendor\tools` in a development build). If none is found, Brisk suggests `pip install ruff`.

### Formatting

- `Ctrl+Alt+L` Reformat Code (`ruff format`; with a selection, only the selected lines).
- `Ctrl+Alt+O` Optimize Imports (ruff `F401` and `I001`): removes unused imports and sorts them with the isort profile PyCharm uses. `from __future__` comes first, then standard library, third-party, project and relative imports as separate groups; `from x import a` and `from x import b` are merged. Lines with `# noqa` and imports inside `try` are left alone. Reports what it did, e.g. "Removed 2 unused imports; imports sorted" or "Imports are already optimized".
- With focus in the Project tree, `Ctrl+Alt+O` and `Ctrl+Alt+L` work on the selected file or folder (recursively). Open files change in their editors, closed ones on disk; ruff runs in the background and the status bar reports e.g. "Optimized imports in 2 of 2 file(s)".
- Code → Code Cleanup applies all safe ruff fixes in the file.
- Actions on Save (Settings → Tools → Actions on Save): Reformat code and Optimize imports run on changed `.py` files when saving (`Ctrl+S` and autosave), imports first. A file ruff cannot parse is saved as is. Both are off by default.
- Each of these is one undo step.

### Type checking and navigation (ty)

- Brisk runs ty (Astral's language server, `ty server`) in the background after interpreters are detected, and restarts it when the interpreter changes. ty is looked up the same way as ruff, including the bundled `lib\ty.exe`. If it is not found anywhere, a notification "Code insight is off: ty not found" suggests `pip install ty`, once.
- ty's type errors are shown together with ruff's problems (underlines, the widget, `F2`, Problems → File). Findings that duplicate ruff's are dropped. `Alt+Enter` → Suppress for line writes `# ty: ignore[rule]`.
- `Ctrl+B`, `Ctrl+Click` or a middle click: Go to Declaration (in another file or a library). On a declaration it shows usages, like Go to Declaration or Usages. `Ctrl+Shift+B` Go to Type Declaration.
- `Ctrl+Q` Quick Documentation, also shown when the mouse rests on a name for 0.5 s. ty's Markdown is rendered as in PyCharm: signatures and examples highlighted as Python, inline code, bold, italics and clickable links. The top line links to the definition (`pkg.util:3`); clicking it opens the file there.
- `Ctrl+Shift+P` Type Info (View → Type Info): the type of the name at the caret from ty, such as the variable's type, `(a: int) -> str` for a function or `type[C]` for a class, in a popup and in the status bar.
- `Ctrl+Shift+I` Quick Definition: the source of the definition (a function or class with decorators and body, up to 40 lines; for a variable, its assignment) in a popup with file and line.
- `Alt+F7` Find Usages, as a tree in the Find tool window: groups Import statements, Value write and Usages in code, then files, then lines. The definition itself is not listed.
- Inlay hints from ty: the inferred type after a name (`x: list[int] = f(1)`) and the parameter name before an argument (`f(a=1)`). Hints that add nothing are hidden: `Any`, `Unknown`, types already visible on the right (literals, list / dict / tuple displays, constructor calls, `= int`), type aliases. Types over 40 characters are shortened, with the full type in a tooltip. Hints are drawn in italics on a quiet background. Hints are not part of the text: the caret, selection and clicks skip them. Turn them off with Settings → Editor → Show inlay hints.
- Positions are sent to ty in characters (UTF-32), so Cyrillic and emoji do not shift columns.

### Completion

- Opens by itself on the first letter of a word and after a dot. `Ctrl+Space` opens it manually. `↑/↓`, `Enter`, `Tab` (replaces the rest of the word), `Esc`.
- Filters by prefix and by camel humps (`gcw` → `getcwd`). Functions get `()` with the caret inside. Names already available in the file rank above auto-imports of equal match quality.
- Auto-import: names from modules not yet imported are offered with the module on the right (`OrderedDict  collections`). `Enter` inserts the name and the import, one `Ctrl+Z` removes both.
- The selected item shows its documentation on the right. Kind markers: `c` class, `f` function, `m` method, `v` variable or value, `p` field or property, `▣` module, `k` keyword.
- `Ctrl+P` Parameter Info opens after `(` and updates after `,`, with the active parameter highlighted (by position or by `key=`). Overloaded functions show one line per overload; the ones that match the call are not dimmed. A function without parameters shows `<no parameters>`.
- Postfix completion after `expr.`: `if`, `not`, `for`, `print`, `return`, `par`. For example `x > 1.if` → `if x > 1:`, `items.for` → `for item in items:`, `f(x).par` → `(f(x))`. Works without ty.
- `Alt+Enter` on an unknown name offers Import (`Import 'collections.defaultdict'`, best module first) and Qualify (`Qualify as 'pkg.util.add'`) from ty, next to ruff's fixes. The import is merged into an existing `from … import` or added as a new line, in one undo step. The first import search in a large environment can take a couple of seconds ("Looking for imports…" in the status bar).

### Intentions (`Alt+Enter`)

Next to ruff's fixes, `Alt+Enter` offers PyCharm's intentions. Without ty only the purely syntactic ones are available. Each is one undo step.

- Import `'pkg.mod.name'` from the project's own symbol index, when ty did not already offer the same import.
- Create function `'name'` on a call to an unknown name: `def name(arg, …): pass` before the current top-level statement, with PEP 8 blank lines.
- Add type hint for variable `'x'` (the type from ty's inlay hint).
- Specify return type using annotation: the type from ty, or, when ty cannot infer it, the union of the types of the `return` statements (`X | None`).
- Convert to f-string literal for `'…'.format(…)` and `'…' % (…)`. Not offered when an argument cannot go inside the braces before Python 3.12.
- Invert 'if' condition for an `if` with an `else` (`==` ↔ `!=`, `in` ↔ `not in`, `not x` → `x`).
- Split into 2 'if' statements for `if a and b`, and Merge nested 'if's for an `if` directly inside another without an `else`.

### Without ty

Brisk stays usable while ty is missing or still starting (its first answers can take seconds):

- Completion uses words from the open file (nearest to the caret first), Python keywords and builtins. After a dot it offers names that follow a dot elsewhere in the file. ty's answer replaces the list when it arrives.
- Parameter Info takes the signature from a `def` in the same file (for a class, `__init__` without `self`) or from a table of builtins (`print`, `open`, `range`, `sorted` and others).
- `Ctrl+B`, `Ctrl+Click` and middle click find a `def`, `class` or the first assignment in the same file. Quick Definition also uses the same file.
- Find Usages searches the same file using the syntax tree and scopes: a local `x` in one function is not mixed with `x` in another, and `obj.x` and `f(x=…)` are not counted. The window is marked "file only, without ty".

## JavaScript, TypeScript, HTML and CSS

Brisk's own support, without a TypeScript server or Node: tree-sitter and the file system.

- Structure, breadcrumbs, sticky lines, Go to Class and Go to Symbol cover JS and TS (see Navigation).
- `Ctrl+B` / `Ctrl+Click` on a path in `import`, `export … from`, `require()`, `import()`, an HTML `src` / `href` or a CSS `@import` / `url()` opens the file. Paths resolve as Node and bundlers do: the usual extensions (`./util.js` finds `util.ts`), `index` files, `compilerOptions.paths` and `baseUrl` of `tsconfig.json` / `jsconfig.json` (comments allowed), `@/` for `src/`, packages in `node_modules` by their `package.json` (`types`, `module`, `main`, `exports`). `/x` looks in the project root, `public` and `static`.
- `Ctrl+B` on an imported name opens its module at the export (`export default` for a default import). On any other name it goes to its declaration in the file (`function`, `class`, `const` / `let` / `var` above the caret), else to the project's symbols of that name.
- Typing a path in those places (after a quote or `/`), or `Ctrl+Space` there, lists the folder's entries: folders first, JS / TS modules without their extension in imports. A bare name in an import lists the packages in `node_modules`.

## Refactoring

- `Shift+F6` Rename. A local variable or parameter is renamed in place, with or without ty: all its occurrences in the function are selected and change together as you type (`Esc` to finish). Other names need ty and open a popup with the new name: `Enter` renames in all files, Preview... shows the changes in the Find tool window (Import statements, Usages in code, Usages in comments and strings) with Do Refactor and Cancel. The Search in comments and strings check box (remembered) adds whole-word matches in comments and string literals. Open files change in the editor, closed files on disk. `Ctrl+Z` in any of the changed files, while none of them has been edited since, undoes the rename in all of them.
- `Shift+F6` on the name of a project module (`import greeter`) offers Rename module 'greeter' and its usages, which renames the file (or package folder) and updates imports. Unlike PyCharm, `Ctrl+Z` does not move the file back afterwards; rename it again to restore the old name.
- Renaming or moving a module or package in Project (`Shift+F6`, `F6` on a `.py` file or folder) updates imports in other files through ty's `willRenameFiles`. The status bar shows how many references were updated. When ty offers no edits (ty 0.0.104 does this for any folder rename and for moving a module to another folder), Brisk updates `from …` / `import …` lines and `pkg.mod.attr` references itself.
- Brisk notifies ty about new and deleted `.py` files (ty does not watch the disk), so imports of just-created modules resolve right away.
- `Ctrl+Alt+V` Extract Variable: the selected expression moves into a variable on the line above. Without a selection, an Expressions popup lets you pick one of the expressions around the caret, innermost first. The name is derived from the expression (`get_total(x)` → `total`, `order.price` → `price`) and is selected in all places for editing. If the same expression occurs again in the function, a "Multiple occurrences found" popup offers to replace this one or all.
- `Ctrl+Alt+C` Extract Constant: the same, but into a module-level `UPPER_CASE` constant placed after the docstring and imports.
- `Ctrl+Alt+M` Extract Method: the selected lines (or expression) become a function `extracted` after the current one, or a method when they use `self` / `cls`. Free variables become parameters, variables read after the block are returned, and `await` makes the function `async`. The name is selected in both places for editing; one undo step. A block with `yield`, a `break` / `continue` outside its loop, or a `return` in the middle is refused with a message in the status bar.
- `Ctrl+Alt+N` Inline: a variable assigned once is substituted into all its uses (parenthesized where needed) and removed. A function whose body is a single `return` (plus an optional docstring) is inlined into calls with its arguments; the Inline Function popup offers all calls and removing the function, or, on a call, only this one.
- `Ctrl+Shift+↑/↓` Move Statement: in Python a whole statement (a function with its decorators, an `if` with its `else`, a multi-line call) swaps with its neighbour in the same block. At the edge of a block nothing happens. In other files lines move.
- `Ctrl+Shift+Delete` Unwrap/Remove: for the statements around the caret, innermost first: Unwrap `if`, Unwrap `else`, Remove `else` / `elif` / `except` (not the only `except` of a `try`), and the same for `for`, `while`, `with`, `try`. The body is dedented to the statement's level.
- Limitations: Extract Method does not handle blocks with several exits; Inline works only for single-assignment variables and single-`return` functions.

## Templates and code generation

- Live templates for Python: type an abbreviation at the start of a line and press `Tab`. Available: `main` / `ifmain` (`if __name__ == "__main__":`), `def`, `class`, `for`, `iter`, `while`, `try` / `tryexcept`, `ifn` (`if name is None:`), `ifnn` (`is not None`), `with`, `prop`, `super`. Indentation follows the file's settings.
- Template fields (name, parameters, body) are visited with `Tab` and `Shift+Tab`. The current field is selected for replacement. After the last field the caret goes to `$END$`. `Esc` or `Enter` finishes.
- `Ctrl+J` lists templates, or expands the abbreviation before the caret.
- `Ctrl+Alt+T` Surround With for the selected lines (or the caret line), in PyCharm's order and numbering 1 to 7: if, if / else, while, try / except, try / finally, for, with, then def and region...endregion Comments. The condition is selected. One undo step.
- `Alt+Insert` Generate inside a Python class: Override Methods..., Implement Methods..., `__init__` from class fields (`x: int`, `y: int = 0`; fields without defaults first), `__repr__` and `__eq__` from class fields and `self.x` in `__init__`, Property `'x'` (getter and setter for `self._x`). Methods that already exist are not offered. The new member goes after the method at the caret, otherwise at the end of the class (replacing a lone `pass`).
- `Ctrl+O` Override Methods... / `Ctrl+I` Implement Methods...: methods of base classes (nearest first, then `object`), or only abstract ones (`@abstractmethod`). Base classes come from ty, including library classes; without ty from the same file, its `from … import` lines and the project index. The body is `return super().m(a, key=key)` as in PyCharm, or `pass` for abstract methods. One method at a time.
- Limitations: template fields are single-line (a newline in a field finishes the template). Optimize Imports and ruff fixes are Python only.

## Run

### Interpreter

- The status bar shows the project interpreter ("Python 3.12 (myproject)"). Click it to choose: the project's `.venv` / `venv` / `env` (found at once from `pyvenv.cfg`), `$VIRTUAL_ENV`, conda, `py -0p`, `python` on `PATH` (found in the background). Each row says what it is (system, conda, project venv, venv) and where, with a shortened path; the full path is in the tooltip. The choice is saved.
- Add Interpreter… in the same list: pick a base Python (system or conda) and Brisk creates `.venv` in the project (`venv` if `.venv` exists) with `python -m venv`, in the background, with progress in the status bar. The new environment becomes the project interpreter and ty restarts.
- If the project has `requirements.txt` or `[project].dependencies` in `pyproject.toml`, Brisk offers Install requirements (`pip install -r requirements.txt` or the pyproject dependencies), with a Not now button. The result is shown in the status bar and a notification.

### SSH interpreters

- Add SSH Interpreter… in the interpreter list: pick a host from `~/.ssh/config` or type `user@host` / `user@host:port`, then one of the Pythons Brisk finds there (`python3`, `python3.x`, `~/.venv`, conda, pyenv). The system `ssh` is used (Windows OpenSSH, or the one from Git for Windows) with keys or the ssh agent; passwords are not supported.
- The project goes to `~/brisk-projects/<project>` on the host. Before each run Brisk uploads what changed since the last upload (by size and time, files under 50 MB, the same files the project tree shows) and removes deleted ones.
- Runs of scripts, modules, pytest/unittest, Django, FastAPI and Flask go through `ssh` with the configuration's parameters and environment, `PYTHONPATH` set to the project folder. Stop ends the remote process. A web server's port is forwarded to the same port on this machine, so the printed address opens.
- Paths in the output are mapped back: a traceback line on the host opens the local file.
- Not yet: debugging, coverage and profiling say so instead of running locally; tests run without the test tree; code insight, the Python console and Python Packages use a local interpreter.

### uv and Poetry projects

- A project uses uv when it has `uv.lock` or a `[tool.uv]` table, and Poetry when it has `poetry.lock` or `[tool.poetry]`. uv and Poetry are looked up next to the project interpreter, on `PATH`, and in the folders of their official installers (`%USERPROFILE%\.local\bin`, `%APPDATA%\Python\Scripts`, `%APPDATA%\pypoetry`).
- Add Interpreter… starts with uv sync (creates or updates `.venv` from `pyproject.toml` and `uv.lock`) or poetry install (creates or updates the Poetry environment, running `poetry lock` first when the lock file is older than `pyproject.toml`). The environment becomes the project interpreter. Tools → Sync Python Environment does the same at any time.
- A Poetry environment that lives in Poetry's cache, outside the project, is found through `poetry env info` and shown as "Python 3.12 Poetry (myproject)".
- In Python Packages, when the interpreter is the project's own environment, Install, Uninstall and Upgrade go through the project manager, as in PyCharm: `uv add` / `uv remove` / `uv sync --upgrade-package`, or `poetry add` / `poetry remove` / `poetry update`. `pyproject.toml` and the lock file change with the environment. A development dependency (`[dependency-groups] dev`, `[tool.poetry.group.dev.dependencies]`) is removed from its own group. The toolbar shows "uv · pyproject.toml" or "Poetry · pyproject.toml".
- After `pyproject.toml` is saved with changed dependencies while the lock file is older, a balloon offers to lock and sync (uv sync, or poetry lock and install). Edits that do not affect the lock file, such as `[tool.ruff]` or the description, do not trigger it.

### Running

- `Ctrl+Shift+F10` runs the current file (or the one selected in Project). It creates a temporary configuration named after the file, as PyCharm does, and keeps the last 5. If the file already has a configuration, that one is used.
- `Shift+F10` runs the selected configuration. Current File means the file in the editor. The toolbar widget selects a configuration, `Alt+Shift+F10` selects and runs one. A selected temporary configuration has Save '…' Configuration, which makes it permanent.
- The environment matches PyCharm: `PYTHONUNBUFFERED=1`, `PYTHONIOENCODING=UTF-8`, the project root on `PYTHONPATH`, the venv first on `PATH`. All unsaved files are saved before a run.
- Run Anything (double `Ctrl`, Run → Run Anything, Find Action): `python x.py …`, `python -m mod …`, `pytest …` and `pip …` run with the project interpreter (pytest with a test tree). A configuration name runs that configuration. Anything else (`git status`, `echo %OS%`) runs in a shell with the project environment activated, as in Terminal. Each command gets its own Run tab, and Rerun repeats it. The first item shows the typed command and how it will run, then history and configurations. The project history (last 30) is in `.brisk/run-anything.txt`. `Ctrl`, then `Ctrl+↑/↓` still clones the caret.

### Run configurations

- Kinds: script, module (`-m`), pytest, Unittests, Doctests, Compound, FastAPI, Flask, Django server, Django tests.
- Fields: target, parameters, interpreter options, working directory, environment variables, paths to `.env` files, interpreter (empty means the project interpreter), `-k` for pytest.
- Stored in `.brisk/run.json`. On first open they are imported from PyCharm (`.idea/runConfigurations/*.xml`, `.run/*.run.xml` and `.idea/workspace.xml`): Python, pytest, Compound, FastAPI (`file` → `module:app`, `additionalOptions` → parameters), Flask Server (path or module plus `application`, `flaskDebug` → `FLASK_DEBUG=1`), Django Server (`host` / `port`, `runNoReload` → `--noreload`, extra options) and Django tests (`TARGET`, `OPTIONS`; `manage.py` from `PYCHARM_DJANGO_MANAGE_MODULE` or found in the project).
- Store as project file (the Project file checkbox): the configuration is also written to `.run/<name>.run.xml` in PyCharm's format, for committing to Git. Brisk writes the file on save and deletes it when the box is cleared or the configuration is renamed or deleted. Files in `.run` take precedence over `run.json`, so a file changed or deleted by a teammate (after `git pull`) shows up in the list immediately. Files for other configuration types (JavaScript and so on) are left alone.
- Run/Debug Configurations dialog (Run → Edit Configurations…, the toolbar widget, `Alt+Shift+F10`, Find Action). The list is on the left: `+` or `Alt+Insert` opens Add New Configuration with templates, plus copy, remove and reorder buttons. The form is on the right, with the resulting command line below it. Environment is entered as `KEY=VALUE;KEY2=VALUE2`.
- Dialog keys: `Tab` / `↑↓` move between fields, `Alt+↑↓` between configurations, `Alt+Shift+↑↓` reorder, `Alt+Insert` add, `Ctrl+D` copy, `Alt+Delete` remove, `Enter` OK (saves `.brisk/run.json`), `Esc` cancel. An empty or duplicate name or an empty script cannot be saved.
- `.env` files (Paths to .env files, separated by `;`, relative to the project, `$PROJECT_DIR$/…` or absolute; imported from PyCharm's `ENV_FILES`). Parsed like python-dotenv: `#` comments, `export`, single-quoted (literal) and double-quoted (`\n`, `\"`) values, multi-line values, `${VAR}` and `${VAR:-default}`. Applied for run and debug. The Environment field overrides them, and a later file overrides an earlier one. If a file is missing, the run does not start and the message names the file.
- Compound: a list of other configurations separated by `;`. Run starts them all at once, each in its own Run tab. Nested compounds expand, repeats and cycles are skipped, an unknown name cannot be saved. Imported from PyCharm (`CompoundRunConfigurationType`).
- Before launch (fields Before launch, run and Before launch, command): other configurations run first, one after another, each in its own tab, then a shell command (`cmd /C` on Windows) in the configuration's working directory and environment, so `pip` is the one from its venv. A non-zero exit code or Stop cancels the launch with a message. Applies to Run, Debug, Run with Coverage, Profile and Rerun. A Compound, the configuration itself or an unknown name cannot be saved. Imported from PyCharm's `RunConfigurationTask`.
- Configuration templates (Edit Configuration Templates, the gear above the list or the link below it): Python and pytest templates with interpreter options, working directory, environment, `.env` files, interpreter, When started, Multiple instances, and for Python also Python Console and Terminal. New configurations from `+` and temporary ones from `Ctrl+Shift+F10` start from the template of their kind (their own environment variables win). Existing configurations are not changed. Stored in `.brisk/run.json` under `templates` (non-empty ones only). Imported from `default="true"` configurations in `workspace.xml`.
- When started: Show Run window (the default; the caret stays in the editor), Focus Run window, Show in Services, Don't show. Imported from `activateToolWindowBeforeRun` / `focusToolWindowBeforeRun`.
- Allow multiple instances (Multiple instances): without it, running a configuration that is still running asks Stop and Rerun (`Enter`) or Cancel (`Esc`). With it, a second tab opens next to the first. Imported from `allowRunningInParallel` (`singleton="false"` in older files).
- Run with Python Console (the Python Console checkbox, for scripts and modules; imported from `SHOW_COMMAND_LINE`): runs with `python -i`, so a `>>>` prompt with the script's variables stays in the Run window after it ends.
- Emulate terminal in output console (the Terminal checkbox; imported from `EMULATE_TERMINAL`): the process gets a pseudo-terminal (ConPTY) instead of pipes, so `isatty()` is true and colours, `\r` progress lines and `getpass` work. To keep ConPTY from dropping the last output of a process that exits quickly, the program runs as a child of an interpreter that waits for it, gives the console 0.3 s and exits with its code.

### Web frameworks

- FastAPI and Flask: the target is `module:app`. Brisk runs `python -m uvicorn main:app …` (with `--factory` for a `create_app()` factory) or `python -m flask --app app run …`. Parameters are uvicorn or `flask run` options. The working directory defaults to the project root.
- A new configuration gets the first `app = FastAPI(...)` / `app = Flask(...)` (or `create_app` factory) found in the project. `Ctrl+Shift+F10` on a file with such an app and no `__main__` block creates a temporary "FastAPI (main)" or "Flask (app)" configuration.
- Django: the target is the path to `manage.py`. Brisk runs `python manage.py runserver …` or `python manage.py test …`. Parameters are the address (`127.0.0.1:8001`) and runserver options, or test labels (`polls -v 2`). The working directory defaults to the folder of `manage.py`.
- `manage.py` is found automatically (up to three levels deep, skipping venvs and service folders, only files that mention django). The settings module it sets is added to the configuration's environment as `DJANGO_SETTINGS_MODULE=mysite.settings`, as PyCharm's Django settings do. `Ctrl+Shift+F10` on such a `manage.py` creates a temporary "Django server" configuration.
- If uvicorn, flask or Django is not installed in the venv, Brisk shows a message instead of starting.
- `http://…` addresses in Run output are links that open in the browser (`0.0.0.0` is opened as `127.0.0.1`).

### Django, FastAPI and Flask code

- `Ctrl+B` or `Ctrl+Click` on a template name opens the template: `render(request, "polls/detail.html")`, `template_name = "..."`, `TemplateResponse`, `render_template`, and in templates `{% extends %}`, `{% include %}`, `{% import %}`, `{% from %}`. Templates are looked up in every `templates` folder of the project and in folders given as `Jinja2Templates(directory=...)` or `template_folder=`. When two folders have the file, the Find window lists both.
- On a URL name it opens the route: `reverse("polls:detail")`, `reverse_lazy`, `redirect("polls:index")`, `resolve_url`, `url_for("read_item")` and `{% url 'polls:detail' q.id %}`. Django namespaces come from `app_name` and `include(..., namespace=...)`; a name without the namespace also matches. Flask blueprint endpoints are `blueprint.function`.
- On `include("polls.urls")` it opens the URLconf; on `{% static 'css/site.css' %}` or `url_for('static', filename=...)` the file in a `static` folder.
- Go to Related Symbol (`Ctrl+Alt+Home`): in a template, the views that render it and the templates that extend or include it; in a view function or class, the templates it uses.
- Endpoints tool window (its stripe button appears in projects with routes; also Find Action → Endpoints): every route with its methods (ANY for Django), full path and name. Django routes follow `ROOT_URLCONF` and `include()`, FastAPI routes `APIRouter(prefix=...)` and `include_router(..., prefix=...)`, Flask routes `Blueprint(url_prefix=...)` and `register_blueprint`. `Enter` or a click opens the view. The list is rebuilt after a Python file or template is saved, or with ↻.
- Django and Jinja2 tags in `.html`, `.jinja` and `.j2` files are highlighted: `{% %}` and `{{ }}` with the tag name, strings, numbers and `|filters`, and `{# #}` comments.
- Limitations: routes are read from the source, not by importing the project, so URL patterns built in loops or by third-party routers (Django REST Framework's `DefaultRouter`) are not listed. No completion of template, URL or static names yet, and no code insight for template variables.

### npm and Node.js

- `package.json` shows ▶ next to each script. Run starts it through the project's package manager: pnpm, yarn or bun when `packageManager` or a lock file (`pnpm-lock.yaml`, `yarn.lock`, `bun.lock`, `package-lock.json`) says so, else npm. The tool is taken from PATH; Package manager in the configuration overrides it.
- `Ctrl+Shift+F10` on a `.js`, `.mjs`, `.cjs` or `.ts` file runs it with node. TypeScript runs through the project's `tsx` when `node_modules/.bin` has it, else with node's own type stripping (Node 22.18 and later).
- npm and Node.js configurations take arguments, the working directory (default: the `package.json` or script folder), environment variables, `.env` files, Before launch steps and Node options. The nearest `node_modules/.bin` folders go on PATH, and `FORCE_COLOR=1` keeps colours in the Run window.
- With Store as project file they are written in WebStorm's format (`js.build_tools.npm`, `NodeJSConfigurationType`). Debug, coverage and profiling are Python only for now and say so.
- Nothing from Node ships with Brisk: without Node.js installed, the run says where to get it.

### Run tool window

- Run (`Alt+4`) has one tab per run. The first line is the command as typed by hand (`python app/main.py --x`; Copy command line gives the full one), the last is "Process finished with exit code N in 0.36 s" with a green ✓ or red ✗. stderr is red. Input for `input()` is typed directly in the window (in green) and sent on `Enter`.
- At most five tabs: a new run closes the oldest finished tab. Running and pinned tabs are kept. Running the same configuration again reuses its tab.
- Pin Tab: a pinned tab is not replaced by the next run of the same configuration, which opens a new tab instead. Rerun from a pinned tab reruns in it.
- Toolbar: `Ctrl+F5` Rerun, `Ctrl+F2` Stop (stops the whole process tree), Soft-Wrap, Scroll to End (scrolling up turns it off, scrolling to the bottom turns it on), Clear All, Copy command line, Open in Terminal. Soft-Wrap and Scroll to End are saved per configuration in `.brisk/run-console.json`.
- Open in Terminal opens a new project terminal tab and runs the same command there: change to the working directory, set the configuration's environment variables (`$env:K='V'`; `PATH` and the venv come from the terminal itself), then the interpreter with its arguments, all quoted for PowerShell (POSIX shell on Linux and macOS).
- Stop ends the tab with "Process terminated" rather than the exit code Windows reports for a killed process.
- Traceback links (`File "…", line N`), `file.py:N` and `file.py:N:M` are clickable and open the file at that line and column.
- ANSI colours: 16 colours, 256-colour palette (`38;5;N`) and truecolor (`38;2;R;G;B`). OSC 8 hyperlinks (written by rich, pytest and others) are underlined: `file://…` opens in the editor (with `#L12`, at line 12), others in the browser.
- Search in output (`Ctrl+F` in the Run console): all matches highlighted, the current one brighter, a counter ("3/12"). `Enter` / `Shift+Enter` (or `F3` / `Shift+F3`, `↓` / `↑`) move between matches, scrolling to them (horizontally too when Soft-Wrap is off). `Alt+C` Match Case. Filter shows only matching lines (clicking a line shows the full output at that line). `Esc` closes. A single-line selection becomes the query. A match split by Soft-Wrap is found and highlighted on both parts. The search updates as output arrives (at most every 300 ms during a flood). Clicking in the console returns input to the process.
- Each tab keeps the last 20,000 lines of output.
- Heavy output (a `for` loop printing tens of thousands of lines a second) does not freeze the window: the console redraws at most once a frame and the parser yields every 32 KB, so typing in the editor stays responsive. "Process finished" arrives after all output.
- As in PyCharm without Emulate terminal, the process runs through pipes: no output is lost on exit and stdout and stderr lines are not mixed internally (ordering between the two streams can occasionally differ, as in PyCharm).

### Services

- Services (`Alt+8`), like PyCharm's Run Dashboard. The tree on the left has a "Run · N running" node with all Run tabs (status, name, "running" or "exit code N · 1.2 s", pytest totals) and Rerun and Stop buttons on each row, and a Database node with database consoles. The right side shows the selected run's console (the whole Run panel) or query results.
- `↑↓` select, `Delete` stops, `Esc` returns to the editor. Rerun from Services stays in Services.

### Coverage and profiling

- Run with Coverage (Run → Run with Coverage, Find Action): the selected configuration or current file (script, module, pytest) runs under `coverage run`, with data in `.brisk/coverage.data`. If coverage.py is not installed, a notification offers to install it.
- The Coverage tool window lists project files with the percentage of covered lines and a bar, with the total on top. `Enter` or double click opens the file at the first uncovered line. Rerun and clear buttons.
- In the editor, executed lines get a green bar in the gutter and missed lines a red one. Marks in a file disappear once it is edited.
- Profile (Run → Profile, Find Action): runs under cProfile (`.brisk/profile.pstats`), then opens the Profiler tool window with a table of functions: Name (`function (file:line)`, builtins in grey), Call Count (`total/primitive` for recursion), Own Time and Time in milliseconds. Click a header to sort, click again to reverse (default is Own Time, descending). Double click or `Enter` opens the function in the editor.

## Debug

- Uses debugpy from the project interpreter, or the bundled copy if the interpreter has none (`lib\debugpy` in the portable build, `vendor\debugpy-lib` for a source build, see [building.md](building.md)). Nothing needs to be installed in the project.
- `Shift+F9` debugs the selected configuration, `Ctrl+Shift+F9` the current file. A test file, folder or single test (caret inside `test_…`, target `file::test`) is debugged through `python -m pytest`, and breakpoints in tests work. Configurations are shared with Run.
- `F8` Step Over, `F7` Step Into, `Shift+F8` Step Out, `F9` Resume, `Alt+F9` Run to Cursor, `Ctrl+F2` Stop. The Debug toolbar also has Rerun Debug Session and Pause Program. Stop works while debugpy is still starting.
- When stopped, the line is highlighted and the Debug tool window (`Alt+5`) shows frames (click to switch) and variables, with lists, dicts and objects expandable.
- Threads: with more than one thread, a Thread selector appears above Frames (MainThread, Worker, …). The selected thread shows its own stack and variables, and steps and Resume apply to it.
- Inline values: while stopped, the lines of the current function from `def` to the stop line show `name: value` for the variables they use, in grey italics.
- Return values: after Step Over or Step Out, the top of Locals shows what the call returned, e.g. `↩ add() = 12`.
- `Shift+F7` Smart Step Into: if the line has several calls (`a(b(3))`), a popup lists them (from debugpy's `stepInTargets`) and steps into the chosen one. With a single call it steps in directly.
- Jump to Cursor (Set Next Statement): from the editor context menu or Find Action, or by dragging the execution arrow in the gutter. Execution moves to the caret line without running the lines in between. Only within the current function (debugpy `gotoTargets` / `goto`); otherwise a message is shown and the program stays put.
- Attach to Process… (`Ctrl+Alt+F5` or Find Action): lists running Python processes (pid, script or `-m module`, command line; debugpy's own processes hidden). `Enter` or click attaches (`debugpy --pid` through the adapter, also on Windows). Stop detaches and leaves the process running. Rerun attaches to the same process while it is alive, otherwise asks for another.
- Hover over a variable (or `obj.attr`) while stopped to see its value, e.g. `items = {list} [1, 2, 3, 4]`, with the first 30 elements. If it is not a value (a module, an unknown name), normal documentation is shown.
- Watches, above the variables: `Insert` or + Watch adds an expression, `F2` or double click edits, `Delete` removes. Add to Watches (Find Action) takes the selection or the word at the caret. Watches are evaluated at every stop in the selected frame and saved per project.
- `F2` or double click on a variable is Set Value. `Ctrl+C` is Copy Value (the full value, not the truncated repr). Arrow keys move through the tree, `→` / `←` expand and collapse.
- Variable context menu: View as DataFrame / View as Array, Set Value…, Copy Value, Copy as Expression (e.g. `df['a'][0]`, from debugpy), View Value (the full value in an editor tab, for long strings), Add to Watches / Remove Watch.
- View as DataFrame / View as Array, for pandas DataFrame and Series and numpy arrays: the table opens in a tab. Column headers show the dtype on hover. Click a header to sort ascending, descending or not at all. Filter takes a pandas expression (`price > 2 and city == 'Kyiv'`, evaluated like `df.eval`) or, for arrays, an expression in `x` (`x[:, 0] > 3`). Format takes e.g. `%.3f` for floats. `Ctrl+C` copies the selected row. Rows load in pages of 200 through debugpy's evaluate, so a million-row frame opens in about 0.1 s and sorts in about 0.3 s. Open tables reload at every stop and are marked stale after Resume and at the end of the session. Arrays with more than two dimensions are shown as 2-D.
- On an uncaught exception the debugger stops, the Debugger tab shows a red Exception header with the type and message, and the traceback is printed in Console.
- `Alt+F8` Evaluate Expression: the Console tab has a `>>>` prompt. `↑/↓` browse history, which persists between sessions. `Tab` completes from debugpy in the selected frame (one match is inserted, several open a list that narrows as you type). The expression is prefilled from the selection or the word at the caret. Console also shows program output, with clickable `File "x.py", line N` and `x.py:N` links as in Run.

### Breakpoints

- `Ctrl+F8` or a click on a line number toggles a breakpoint. Right click on a breakpoint sets a condition (`i == 3`); conditional breakpoints are drawn with a hole.
- `Ctrl+Shift+F8` (or the breakpoint button on the Debug toolbar) opens Breakpoints: all breakpoints with an enabled checkbox (a disabled one stays as a grey ring and does not stop), the condition editable in place, `Delete` to remove, `Enter` or double click to go to the line.
- Python Exception Breakpoints in the same window: On raised exceptions (stop even on caught exceptions) and On uncaught exceptions, saved per project.
- + Exception (or `Insert`) adds a breakpoint on an exception class: a builtin like `ValueError` or a full importable name like `mypkg.errors.MyError` (subclasses included), with On raise (even if caught) and On termination (uncaught).
- Mute Breakpoints (Debug toolbar) silences all breakpoints, including exception breakpoints: they turn grey and are not sent to debugpy. Click again to restore.
- Breakpoints move with edits and persist between sessions. A hollow circle means the debugger cannot stop on that line.
- Limitations: debugpy cannot find an exception class declared in the script being run. Compound configurations cannot be debugged; the message suggests debugging the configurations one by one.

## Testing

### pytest

- `Ctrl+Shift+F10` in a test file (`test_*.py`, `*_test.py`) runs pytest: with the caret in a test, only that test ("pytest for test_x.TestA.test_b"); in a `Test…` class, the class; otherwise the whole file ("pytest in test_x.py"). On a folder in Project, all tests in it ("pytest in tests").
- Run marks in the gutter of Python files, in their own column right of the line numbers, separate from breakpoints: `if __name__ == "__main__":`, `def main` (when there is no `__main__` block), and in test files `test*` functions, `Test*` classes and their `test*` methods. A click opens Run '…' / Debug '…' for the script or for pytest on that node (`file::Class::test`).
- After a test run, the mark shows the result: a green check for passed, a red cross for failed (a class shows its worst test; parametrized variants count as one test). The menu of a parametrized test then lists each parameter set ("Run ✓ test_x[1]", "Run ✗ test_x[2]").
- pytest configurations (target, `-k`, parameters) are stored in `.brisk/run.json` and imported from PyCharm (`py.test`, target as path or module name).
- The Run tab is split into a test tree on the left (module → class → test, with status and time, updated during the run) and the console on the right. A summary line: "Tests failed: 1, passed: 3, ignored: 1 of 5 tests – 6 ms".
- A selected test shows its captured output and error, with clickable `file.py:N` links. The root shows the full pytest output.
- When the run ends, the first failed test is selected. Double click, `Enter` or `F4` opens the test source. `←/→` collapse and expand.
- Parametrized tests (`@pytest.mark.parametrize`): the tree has a `test_x` node with parameter sets `[1]`, `[a-b]` and so on below it. The selected or hovered node has a run button; it and `Ctrl+Shift+F10` in the tree run that node (test, parameter set, class or file) as a temporary configuration.
- Rerun Failed Tests reruns only the failed ones.
- Toolbar above the tree: Show Passed and Show Ignored (hide passed / skipped), Sort by Duration (slowest first), Rerun Failed Tests First (the next run starts with the tests that failed last time; the plugin changes the order, the set of tests stays the same).
- Test History: the last 10 runs of each configuration (`.brisk/test-history`). Selecting one opens the saved tree and output in a pinned tab.
- Export Test Results: an HTML report (summary, tree with status and time, errors and output of failed tests) in `.brisk/test-results/<name>.html`, opened in the browser.
- The command is a plain `python -m pytest …`. Progress is reported by a small plugin loaded through `PYTEST_PLUGINS`; nothing is written into the project. If pytest is missing from the venv, Brisk says so before running.

### unittest and doctest

- Configuration kinds Unittests (`python -m unittest`; a folder means `discover`, a file, class or test uses dotted names) and Doctests (`python -m doctest`; `file.py::name` runs one example).
- The test tree, gutter results, Rerun Failed and history work as for pytest. Progress is reported by Brisk's own runner; the command shown in the header is the usual one. `subTest`, `setUpClass` errors and modules that fail to import show up in the tree.
- Imported from PyCharm (`tests` / `Unittests` and `tests` / `Doctests`).
- Default test runner (in Edit Configuration Templates): what `Ctrl+Shift+F10` and gutter marks use. Autodetect (pytest if installed in the interpreter, otherwise unittest), pytest or Unittests. Stored in `.brisk/run.json` as `testRunner`, imported from `PROJECT_TEST_RUNNER` in the `.iml` file.

## Python console and packages

- Python Console (Tools → Python Console) is a REPL in the project interpreter. `Enter` runs a complete statement. For an incomplete one (`def f():`, `for …:`) it adds an indented line, and an empty line ends the block. `Shift+Enter` inserts a new line. `↑/↓` on the first / last line browse history.
- The value of the last expression is printed. Executed commands in the history are highlighted as Python. Tracebacks do not include the console's own frames. `input()` asks in the same field.
- Variables are listed on the right (`name = {type: len} value`) and refresh after each command.
- `Ctrl+C` or Stop while a command runs interrupts it (KeyboardInterrupt, also in `time.sleep()` and `input()`); the session and its variables stay. If the command does not stop within 1.5 s, the process is killed. Stop with no command running ends the console, Rerun restarts it, Clear clears the output.
- Project files named like standard library modules (`types.py`) do not break the console.
- Python Packages lists the packages of the project interpreter with versions (`pip list`, or `uv pip` if the environment has no pip), with a name filter.
- Install takes a name and an optional version (`2.31` becomes `==2.31`, `>=2,<3` is used as is). `Enter` in the field also installs. Uninstall (or `Delete` in the list) asks for confirmation.
- pip runs in the background. Operations are queued and run one at a time. A package row shows a spinner and "Installing…", "Uninstalling…", "Upgrading…" or "Queued", and a package being installed is shown above the list. The result goes to the status bar and a notification, then the list reloads.
- Upgrade: after the list loads, `pip list --outdated` runs in the background. Outdated packages show "1.0 → 1.2" and an Upgrade button (on the hovered or selected row, and in the toolbar).
- PyPI suggestions: while typing a package name, matching PyPI names appear below (exact match first, then prefix, then substring). The name list (about a million names) is downloaded by the project interpreter from PyPI's Simple API on first use and cached for a day (`pypi-names-v2.txt` in the settings folder). It is not kept in memory: each query reads the file in 1 MB chunks in the background, and a stale query is cancelled by a newer one. `BRISK_PYPI_INDEX` sets a different index or file.

## Version control (Git)

- Git is used through the installed `git`, so credential helpers, hooks and LFS work. All calls run in the background.
- Gutter change marks against `HEAD` (added, modified, deleted), including unsaved text. A click on a mark opens a popup with the lines as they are in `HEAD` (changed words marked) and Previous / Next Change, Rollback Lines, Show Diff and Copy. `Ctrl+Alt+Z` with the caret in a change rolls back just those lines.
- File colours by status in the tree and on tabs: modified blue, new green, unversioned red.
- Commit (`Alt+0`, `Ctrl+K`): Changes and Unversioned Files with checkboxes, the message, Amend, Commit (`Ctrl+Enter`) and Commit and Push (`Ctrl+Alt+K`: commit, then the Push dialog). Exactly the checked files are committed. In the diff of a modified file, each change block has a checkbox: unchecked blocks stay out of the commit (the file's checkbox shows `−`), and the file is committed as `HEAD` plus the checked blocks. `↑/↓`, `Space`, `F4`, `Ctrl+D` or double click shows the diff. Create Git Repository… appears if the project is not under Git yet.
- Diff (`Ctrl+D`): side by side in a separate tab. `F7` / `Shift+F7` next / previous difference, `F4` jumps to the source.
- Git log (`Alt+9`): branches (click goes to the commit, double click checks out), commit graph, branch and tag labels, commit details with the file list and a diff on double click. Right click on a commit: Copy Revision Number, Cherry-Pick, Checkout Revision, Reset Current Branch to Here… (Soft, Mixed, Hard, Keep), Revert Commit, Undo Commit (the last commit). Right click on a file of a commit: Show Diff, Show Diff with Working Tree, Open.
- Git log filters in the header: text (message, case-insensitive, or hash prefix; the log rebuilds after 250 ms, `Enter` moves to the list, `Esc` clears), Branch and User. A reset button clears all. The graph is built from the filtered list.
- Branches (`` Ctrl+Shift+` `` or click the branch in the toolbar): search, `Enter` checks out (a remote branch gets a tracking branch), New Branch…. `→` on a branch (or right click in Git log) opens its actions: Checkout, Merge into Current (a conflict opens Conflicts), Show in Log (filters the log by that branch), Delete. A merged branch is deleted at once, an unmerged one only after confirming Force Delete, which shows how many commits will be lost. The current branch cannot be deleted.
- Push (`Ctrl+Shift+K`): the Push Commits dialog lists outgoing commits of the current branch and the files of the selected one (double click for the diff). `Enter` pushes. A branch without an upstream is published and the upstream is set. Without a remote, Brisk explains how to add one.
- Update Project (`Ctrl+T`, pull with merge) reports how many commits and files arrived. Fetch (in VCS Operations) reports how many incoming commits the upstream has.
- Rollback (`Ctrl+Alt+Z`, with confirmation). In the Commit window it rolls back the checked files, or the selected one if none is checked.
- VCS Operations popup: `` Alt+` ``. Items are grouped as in PyCharm and numbered; `1`–`9` run an item at once.
- Compare with Branch… (VCS Operations, or Git: Compare with Branch… in a file's context menu): the file in the chosen branch against the local version, including unsaved edits. Compare with Local in Show History compares the selected commit with the local version.
- Annotate with Git Blame (VCS Operations or right click on the gutter): author and date for each line, shaded lighter for older changes. The tooltip shows hash, author and message, and a click opens that commit's diff. Unsaved lines have no annotation. The column is recomputed after save, commit and checkout.
- Conflicts opens by itself when a merge, pull or rebase stops on conflicts (or via Resolve Conflicts in VCS Operations): Accept Yours, Accept Theirs and Merge…. Merge… is a three-pane dialog (yours, result, theirs) with non-conflicting changes already merged. `»` and `«` take a side (again to append the other side), `×` keeps the base, `↑/↓` and `F7` move between changes, `Alt+→` / `Alt+←`, `Ctrl+Enter` Apply (writes the file and runs `git add`).
- Stash Changes…: a message and Include untracked files (`Alt+U`). Open files are saved first. Unstash Changes…: the stash list with Apply, Pop and Drop (`Enter`, `Shift+Enter`, `Delete`) and the files of the selected stash; clicking a file shows its diff against the commit the stash was made on. A conflict on apply opens Conflicts.
- Show History (VCS Operations, or Git: Show History in the context menu of a file in the tree or editor): the file's commits, following renames, with the selected commit's patch on the right. `Enter`, `Ctrl+D` or double click opens the full diff. With a selection in the editor, Show History for Selection lists only the commits that changed those lines (`git log -L`).
- Success and error notifications. After operations, open files are reloaded from disk; unsaved edits are left alone.

## Database

- A Database tool window like PyCharm Professional and DataGrip. Open it with the button on the right stripe or Find Action → Database.
- PostgreSQL, MySQL / MariaDB and SQLite, with native Rust / C drivers.
- Data Source dialog (`Alt+Insert` new, `Shift+Enter` or `Ctrl+Alt+Shift+S` properties): Name, Driver, Host, Port, User, Password, Database, SSL mode (disable / prefer / require), Save password Forever / Never, Test Connection (`Alt+T`). Pasting a URL such as `postgresql://user:pass@host:5432/db?sslmode=require` or `jdbc:…` into Host fills the form.
- Data sources are stored in the project's `.brisk/dataSources.json`, without passwords. On first open they are imported from PyCharm's `.idea/dataSources.xml`.
- Passwords are stored in `%APPDATA%\Brisk\credentials.json` (`data\credentials.json` next to the exe in the portable build), encrypted with DPAPI for the current Windows user. On Linux and macOS the file has mode 0600.
- Tree: data source → schemas → tables and views → columns with types, PK and NOT NULL. Connects on expand. `Ctrl+F5` refreshes, `Ctrl+F2` disconnects, `Delete` twice removes a data source.
- Console (`Ctrl+Shift+F10`): an SQL file with highlighting. `Ctrl+Enter` runs the selection or the statement at the caret. Results appear in Services (`Alt+8`): a table, `<null>` for nulls, "N rows retrieved … in X ms". Server errors are shown with their code. `Ctrl+F2` or Stop cancels the query. After DDL the tree refreshes itself.
- Edit Data (`F4` or double click on a table): the table's data in a tab. Arrows, `PgUp` / `PgDn`, `Home` / `End` move between cells, `Ctrl+C` copies a value, `Ctrl+F5` reloads.
- Results are limited to 500 rows (PyCharm's page size).
- Differences from PyCharm: no JDBC and no JVM.

## Terminal

- Terminal (`Alt+F12`). The shell starts the first time the window opens: PowerShell through ConPTY on Windows, `$SHELL` on Linux. The ▾ next to `+` opens a tab with another shell found on the machine: PowerShell 7, Command Prompt, Git Bash. Tabs are named after their shell. A middle click closes a Terminal or Run tab.
- It starts in the project root with the project interpreter's environment activated. For a venv (wherever it is), its folder goes first on `PATH` and `VIRTUAL_ENV` and `VIRTUAL_ENV_PROMPT` are set. For conda, the environment folders go on `PATH` and `CONDA_PREFIX` and `CONDA_DEFAULT_ENV` are set. In PowerShell the prompt starts with `(.venv)`, as after `activate`. Until the interpreter is detected, `.venv` / `venv` in the project root is used. A system interpreter activates nothing. Changing the interpreter applies to new tabs.
- Tabs Local, Local (2), …: `Ctrl+Shift+T` new, `Ctrl+F4` close, `Alt+←/→` switch, or the `+` button. A tab closes when its shell exits.
- Override IDE shortcuts: keys go to the shell, except tool window shortcuts (`Alt+<digit>`, `Alt+F12`), `Ctrl+Shift+A`, `Shift+F10` and similar.
- `Esc` returns to the editor. In vim, less, htop and similar programs `Esc` goes to the program.
- `Ctrl+C` copies when there is a selection and interrupts otherwise. `Ctrl+V` and `Shift+Insert` paste (bracketed paste).
- Mouse selection, double click selects a word, triple click a line. The wheel scrolls 10,000 lines of history, or sends arrow keys in full-screen programs.
- 256 colours and truecolor, bold, italic, underline, wide characters.

## Local History

- Find Action → Show Local History.
- A copy of the file is stored on every save, when a file opens with new content, after an external change and before a rollback. Put Label adds a named revision. A folder's history includes deleted files, which can be restored.
- Kept for 5 days, up to 200 revisions per file, in `%APPDATA%/Brisk/LocalHistory`.
- The list shows each revision's age and "+N −M vs now". `Enter` compares with the current text, `Ctrl+Enter` reverts (undo with `Ctrl+Z`).

## Settings

- Settings (`Ctrl+Alt+S`): Dark / Light theme, editor font (from installed monospace fonts such as JetBrains Mono, Cascadia, Consolas, Fira Code), size and line spacing, Show inlay hints (Editor → Appearance, on by default), Actions on Save (Reformat code, Optimize imports; off by default), and the appearance, editor and tab settings described above.
- Changes preview immediately. OK saves, `Esc` or Cancel reverts.
- Keymap (main menu → Keymap…, or Find Action → Keymap): every action with its shortcuts, filtered by name, group or shortcut (type `ctrl+d` to see what uses it). Changed actions are marked. `Enter` on an action: Add Keyboard Shortcut…, Remove `<shortcut>`, Reset to Default. Add Keyboard Shortcut opens a box that records up to two keystrokes (`Ctrl+K, Ctrl+C`); it lists the actions that already use them, and `Enter` takes the shortcut away from them, as PyCharm's Remove does. `Esc` cancels. Double Shift stays as it is.
- Import PyCharm Keymap (in the same list): the active custom keymap of the newest PyCharm in `%APPDATA%\JetBrains` (and the custom keymaps it is based on). PyCharm action ids are mapped to Brisk's; actions Brisk does not have and numpad keys are skipped and counted. A keymap based on Eclipse, VS Code or another built-in keymap brings only its own changes, on top of PyCharm's default. Reset All Shortcuts goes back to PyCharm's defaults.
- Changes are saved in `keymap.json` in the settings folder and apply to all open windows at once.
- Stored in `%APPDATA%/Brisk/settings.json` (in the `data` folder for the portable build, see [building.md](building.md)).
- `Alt+Shift+.` / `Alt+Shift+,` or `Ctrl+Wheel` change the font size in all editors until restart. Reset Font Size and Switch Light / Dark Theme are in Find Action.
- `.editorconfig`: `indent_style`, `indent_size`, `tab_width`, `end_of_line`, `trim_trailing_whitespace` (except on the caret line), `insert_final_newline`, `max_line_length` (right margin). Files are looked up the folder tree until `root = true`. Glob sections support `*`, `**`, `{a,b}`, `[abc]`, `{1..3}`.
- Without `.editorconfig`, the indent is detected from the file (tabs, 2, 4 or 8 spaces). The status bar shows the actual indent ("2 spaces", "Tab").
