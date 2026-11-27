# Building

## From source

```sh
cargo build --release -p brisk-app
```

The binary is `target\release\brisk.exe`. Pass a project folder as the argument:

```sh
target\release\brisk.exe <project folder>
```

Or build and run in one step:

```sh
cargo run --release -p brisk-app -- examples/demo_project
```

`examples/demo_project` is a small Python project for trying things out. Its SQLite data source `data/shop.db` is generated:

```sh
cargo run -p brisk-db --example make_demo
```

## Bundled tools

Brisk prefers ty, ruff and debugpy from the project interpreter or `PATH`. For machines that have none, a build can carry its own copies. Two scripts fetch them from PyPI with `pip download`; both need a Python with pip.

```powershell
powershell -ExecutionPolicy Bypass -File scripts\fetch-tools.ps1     # [-Ty 0.0.105] [-Ruff 0.17.5]
powershell -ExecutionPolicy Bypass -File scripts\fetch-debugpy.ps1   # [-Version 1.8.22]
```

- `scripts\fetch-tools.ps1` puts `ty.exe` and `ruff.exe` from the Windows x64 wheels into `vendor\tools`, each with its license from the wheel (`ty.LICENSE.txt`, `ruff.LICENSE.txt`). A tool already present at the requested version is not downloaded again.
- `scripts\fetch-debugpy.ps1` unpacks the pure-Python debugpy wheel into `vendor\debugpy-lib`.

Development builds look for the tools in these folders. The portable package copies them into `lib\`.

## Portable Windows package

```powershell
cargo xtask package-portable   # --no-build packages an already built exe
```

`scripts\package-portable.ps1 [-NoBuild]` does the same (it calls the xtask). The task builds the release binary (unless `-NoBuild`), runs both fetch scripts, and writes `dist\Brisk-<version>-windows-x64.zip` (about 39 MB). The version comes from `Cargo.toml`.

The zip contains:

- `brisk.exe`
- `README.md`, `LICENSE-MIT`, `LICENSE-APACHE`
- `THIRD-PARTY-NOTICES.txt`, the licenses of the Rust crates and icons compiled into `brisk.exe`
- `lib\debugpy`, the debugger, with its license files
- `lib\ty.exe` and `lib\ruff.exe` with their licenses, so code insight and inspections work without installing anything
- `portable`, a marker file that switches on portable mode

`THIRD-PARTY-NOTICES.txt` is committed to the repository. After changing dependencies, regenerate it with `scripts\third-party-notices.ps1`, which reads `cargo metadata` and the license files in the crate sources.

Unpack it anywhere (a USB stick, a folder without admin rights) and run `brisk.exe`. Nothing is installed.

While the `portable` file sits next to the exe, settings, database consoles and saved passwords are kept in a `data` folder next to the exe instead of `%APPDATA%\Brisk`. Delete `portable` to go back to `%APPDATA%\Brisk`.

Saved database passwords are encrypted with DPAPI for the current Windows user. On another computer they have to be entered again.

## Windows installer

```powershell
cargo xtask package-setup   # --no-build uses the exes already in target\release
```

This builds the portable zip as above, then `crates\brisk-setup` (the installer, a small GPUI app), and writes `dist\Brisk-<version>-setup.exe` (about 43 MB). The zip rides at the end of the setup exe. The setup's manifest is swapped for `crates\brisk-setup\setup.manifest`, which asks for no elevation: without it Windows guesses from the name "setup" that it needs administrator rights.

The setup installs for the current user only, with no administrator rights:

- Files go to `%LOCALAPPDATA%\Programs\Brisk` (Customize can pick another folder), without the `portable` marker, so settings live in `%APPDATA%\Brisk`.
- Everything is registered under `HKCU`: Apps & features (with Modify and Uninstall), App Paths, "Open with" for the chosen file types, "Edit with Brisk" and "Open Folder in Brisk" in Explorer, PATH, and Start menu and Desktop shortcuts.
- `uninstall.exe` in the install folder is the same setup without the zip. It runs from a temporary copy so that it can delete the folder. `--modify` changes the options. `--silent` works for both (`--delete-settings` also removes `%APPDATA%\Brisk`). The setup takes `--silent --dir <folder>`.

The in-app updater updates an installed copy in place from the same zip and keeps the version in Apps & features current.

A file or folder opened from Explorer goes to a running Brisk window when one has its project open. A file goes to the deepest such project. Anything else starts a new window.

### Signing

Set `BRISK_SIGN` to a signing command, with `{file}` where the path goes. The xtasks then sign `brisk.exe` before it is zipped, and the setup after the zip is appended. For example, with a certificate in the store:

```powershell
$env:BRISK_SIGN = 'signtool sign /fd sha256 /tr http://time.certum.pl /td sha256 /a {file}'
cargo xtask package-setup
```

The setup finds its zip in front of the signature, so signing does not break it. `uninstall.exe` is cut from the setup before the signature, so it is unsigned. Without `BRISK_SIGN`, nothing is signed.

## Size, startup and memory budget

```powershell
cargo xtask budget
```

`scripts\budget.ps1` takes the same options in PowerShell form (`-Build`, `-SizeOnly`, …) and calls the xtask. Options: `--build` (build first), `--package` (rebuild the zip), `--runs 3`, `--no-membudget`, `--size-only` (sizes only, as in CI), `--out results.json`, and `--exe`, `--smoke-exe`, `--zip`, `--project` to measure something other than the defaults (`target\release\brisk.exe`, `target\smoke\brisk.exe`, the newest `dist\*.zip`, `examples\demo_project`).

`BRISK_SMOKE_MEMBUDGET` is a smoke, so it runs on the build with the `smoke` feature; the release exe has no smokes. `-Build` builds both, the smoke one with `--profile smoke` (release without LTO, see [testing.md](testing.md#smoke-tests)).

One run takes under a minute on an already built `target\release\brisk.exe`. It measures:

- the size of `brisk.exe` and of the zip in `dist\`;
- the time from process start to the first drawn window frame: the median of `-Runs` launches on `examples\demo_project`, the first (cold) launch separately, and a breakdown by step (settings, GPUI init, window creation, Workspace, Git, first frame);
- the working set after 5 s of idle;
- the result of `BRISK_SMOKE_MEMBUDGET` on a temporary copy of the project, and the peak memory of ty and ruff during it.

Limits are set in `$Limits` at the top of the script:

| Metric | Limit |
|---|---|
| `brisk.exe` | 26 MB |
| zip | 39 MB |
| start to first frame (median) | 400 ms |
| working set at idle | 110 MB |
| working set at the end of `BRISK_SMOKE_MEMBUDGET` | 200 MB |

The cold start, the startup breakdown, private bytes and the ty and ruff peaks are reported but not checked.

The script exits with 1 when a limit is exceeded and with 2 when something cannot be measured. Every launch gets a fresh `APPDATA` in a temporary folder, so the machine's own settings and history do not affect the result.

Inside the app, the measurements are made by `BRISK_BUDGET=<report.json>`; see [testing.md](testing.md#budget-probe).

## Icon and version resource

The icon is `assets/brisk.ico`. It is drawn by `assets/make_icon.py`, which needs Pillow:

```sh
python assets/make_icon.py
```

On Windows, `crates/brisk-app/build.rs` compiles `crates/brisk-app/brisk.rc` into the executable with the `embed-resource` crate. The resource file holds the icon (resource ID 1, which GPUI uses as the window and taskbar icon) and the version information. The version numbers in `brisk.rc` are written out by hand and are not read from `Cargo.toml`.
