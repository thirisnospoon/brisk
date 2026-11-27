//! The per-user install on Windows: everything under `HKEY_CURRENT_USER`, shortcuts through
//! the shell's own `WScript.Shell` (it handles a redirected Desktop and non-ASCII paths).

use super::{Installed, Options, FILE_TYPES};
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD,
    REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_NOEXPAND, RRF_RT_REG_DWORD, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
};

const APP_KEY: &str = r"Software\Brisk";
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Brisk";
const APP_PATH_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\App Paths\brisk.exe";
const PROG_ID: &str = "Brisk.File";
const CLASSES: &str = r"Software\Classes";
/// The shell verbs: on files, on folders and on a folder's empty background.
const VERBS: &[(&str, &str, &str)] = &[
    (r"*\shell\Brisk", "Edit with Brisk", "%1"),
    (r"Directory\shell\Brisk", "Open Folder in Brisk", "%V"),
    (r"Directory\Background\shell\Brisk", "Open Folder in Brisk", "%V"),
];

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// An open key, closed on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from RegCreateKeyExW and is closed once.
        unsafe { RegCloseKey(self.0) };
    }
}

fn create(sub: &str) -> Result<Key, String> {
    let mut h: HKEY = std::ptr::null_mut();
    // SAFETY: valid UTF-16 strings and out pointer; the handle is closed by `Key`.
    let r = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(sub).as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            std::ptr::null(),
            &mut h,
            std::ptr::null_mut(),
        )
    };
    if r == ERROR_SUCCESS {
        Ok(Key(h))
    } else {
        Err(format!(r"cannot write HKCU\{sub} (error {r})"))
    }
}

impl Key {
    fn set_typed(&self, name: Option<&str>, value: &str, ty: u32) -> Result<(), String> {
        let v = wide(value);
        let n = name.map(wide);
        // SAFETY: the data pointer covers `v` including its terminator.
        let r = unsafe { RegSetValueExW(self.0, n.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()), 0, ty, v.as_ptr().cast(), (v.len() * 2) as u32) };
        if r == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!("cannot set {} (error {r})", name.unwrap_or("(default)")))
        }
    }

    fn set(&self, name: Option<&str>, value: &str) -> Result<(), String> {
        self.set_typed(name, value, REG_SZ)
    }

    fn set_dword(&self, name: &str, value: u32) -> Result<(), String> {
        let n = wide(name);
        // SAFETY: four bytes of `value`.
        let r = unsafe { RegSetValueExW(self.0, n.as_ptr(), 0, REG_DWORD, (&value as *const u32).cast(), 4) };
        if r == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!("cannot set {name} (error {r})"))
        }
    }
}

/// A string value (not expanded) and whether it was `REG_EXPAND_SZ`.
fn get_string(sub: &str, name: Option<&str>) -> Option<(String, bool)> {
    let (s, n) = (wide(sub), name.map(wide));
    let np = n.as_ref().map_or(std::ptr::null(), |n| n.as_ptr());
    let mut size = 0u32;
    let mut ty = 0u32;
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
    // SAFETY: a size query first, then a buffer of that size.
    unsafe {
        if RegGetValueW(HKEY_CURRENT_USER, s.as_ptr(), np, flags, &mut ty, std::ptr::null_mut(), &mut size) != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        if RegGetValueW(HKEY_CURRENT_USER, s.as_ptr(), np, flags, &mut ty, buf.as_mut_ptr().cast(), &mut size) != ERROR_SUCCESS {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some((String::from_utf16_lossy(&buf[..len]), ty == REG_EXPAND_SZ))
    }
}

fn get_dword(sub: &str, name: &str) -> Option<u32> {
    let (s, n) = (wide(sub), wide(name));
    let mut v = 0u32;
    let mut size = 4u32;
    // SAFETY: four bytes out.
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, s.as_ptr(), n.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&mut v as *mut u32).cast(), &mut size) };
    (r == ERROR_SUCCESS).then_some(v)
}

fn delete_tree(sub: &str) {
    // SAFETY: a valid string; a missing key is not an error worth reporting.
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(sub).as_ptr()) };
    // RegDeleteTreeW empties the key; the key itself goes with a second call.
    let (parent, leaf) = sub.rsplit_once('\\').unwrap_or(("", sub));
    let p = create(parent);
    if let Ok(p) = p {
        // SAFETY: as above, on the parent key.
        unsafe { RegDeleteTreeW(p.0, wide(leaf).as_ptr()) };
        unsafe { windows_sys::Win32::System::Registry::RegDeleteKeyW(p.0, wide(leaf).as_ptr()) };
    }
}

fn delete_value(sub: &str, name: &str) {
    // SAFETY: valid strings.
    unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(sub).as_ptr(), wide(name).as_ptr()) };
}

pub fn default_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Programs").join("Brisk"))
}

pub fn installed() -> Option<Installed> {
    let (dir, _) = get_string(APP_KEY, Some("InstallDir"))?;
    let dir = PathBuf::from(dir);
    if !dir.join("brisk.exe").is_file() {
        return None;
    }
    let version = get_string(APP_KEY, Some("Version")).map(|v| v.0).unwrap_or_default();
    let flag = |n: &str, d: bool| get_dword(APP_KEY, n).map_or(d, |v| v != 0);
    let types = get_string(APP_KEY, Some("FileTypes")).map(|v| v.0);
    let def = Options::default();
    let options = Options {
        start_menu: flag("StartMenu", def.start_menu),
        desktop: flag("Desktop", def.desktop),
        path: flag("Path", def.path),
        context_menu: flag("ContextMenu", def.context_menu),
        file_types: types.map_or(def.file_types, |t| t.split(';').filter(|s| !s.is_empty()).map(str::to_string).collect()),
    };
    Some(Installed { dir, version, options })
}

fn today() -> String {
    let mut st: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    // SAFETY: fills the struct.
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut st) };
    format!("{:04}{:02}{:02}", st.wYear, st.wMonth, st.wDay)
}

pub fn register(dir: &Path, version: &str, o: &Options, size_kb: u64) -> Result<(), String> {
    let exe = dir.join("brisk.exe");
    let exe_s = exe.display().to_string();
    let dir_s = dir.display().to_string();
    let uninstaller = dir.join("uninstall.exe").display().to_string();

    let app = create(APP_KEY)?;
    app.set(Some("InstallDir"), &dir_s)?;
    app.set(Some("Version"), version)?;
    app.set_dword("StartMenu", o.start_menu.into())?;
    app.set_dword("Desktop", o.desktop.into())?;
    app.set_dword("Path", o.path.into())?;
    app.set_dword("ContextMenu", o.context_menu.into())?;
    app.set(Some("FileTypes"), &o.file_types.join(";"))?;

    // Apps & features.
    let u = create(UNINSTALL_KEY)?;
    u.set(Some("DisplayName"), "Brisk")?;
    u.set(Some("DisplayVersion"), version)?;
    u.set(Some("Publisher"), "Brisk")?;
    u.set(Some("DisplayIcon"), &format!("{exe_s},0"))?;
    u.set(Some("InstallLocation"), &dir_s)?;
    u.set(Some("UninstallString"), &format!("\"{uninstaller}\""))?;
    u.set(Some("QuietUninstallString"), &format!("\"{uninstaller}\" --silent"))?;
    u.set(Some("ModifyPath"), &format!("\"{uninstaller}\" --modify"))?;
    u.set(Some("URLInfoAbout"), "https://github.com/thirisnospoon/brisk")?;
    u.set(Some("HelpLink"), "https://github.com/thirisnospoon/brisk/issues")?;
    u.set(Some("InstallDate"), &today())?;
    u.set_dword("EstimatedSize", size_kb.min(u32::MAX as u64) as u32)?;
    u.set_dword("NoRepair", 1)?;

    // `brisk` in Win+R and Start's search.
    let ap = create(APP_PATH_KEY)?;
    ap.set(None, &exe_s)?;
    ap.set(Some("Path"), &dir_s)?;

    // The app and its file type.
    let open = format!("\"{exe_s}\" \"%1\"");
    let icon = format!("{exe_s},0");
    let pid = create(&format!(r"{CLASSES}\{PROG_ID}"))?;
    pid.set(None, "Brisk file")?;
    create(&format!(r"{CLASSES}\{PROG_ID}\DefaultIcon"))?.set(None, &icon)?;
    create(&format!(r"{CLASSES}\{PROG_ID}\shell\open\command"))?.set(None, &open)?;
    let apps = format!(r"{CLASSES}\Applications\brisk.exe");
    create(&apps)?.set(Some("FriendlyAppName"), "Brisk")?;
    create(&format!(r"{apps}\shell\open\command"))?.set(None, &open)?;
    delete_tree(&format!(r"{apps}\SupportedTypes"));
    let supported = create(&format!(r"{apps}\SupportedTypes"))?;
    for (ext, _) in FILE_TYPES {
        let key = format!(r"{CLASSES}\.{ext}\OpenWithProgids");
        if o.file_types.iter().any(|t| t == ext) {
            create(&key)?.set(Some(PROG_ID), "")?;
            supported.set(Some(&format!(".{ext}")), "")?;
        } else {
            delete_value(&key, PROG_ID);
        }
    }

    // Explorer's context menu.
    for (key, label, arg) in VERBS {
        let key = format!(r"{CLASSES}\{key}");
        if o.context_menu {
            let k = create(&key)?;
            k.set(None, label)?;
            k.set(Some("Icon"), &icon)?;
            create(&format!(r"{key}\command"))?.set(None, &format!("\"{exe_s}\" \"{arg}\""))?;
        } else {
            delete_tree(&key);
        }
    }

    set_path(dir, o.path)?;
    shortcuts(&exe, dir, o.start_menu, o.desktop)?;
    notify_shell();
    Ok(())
}

pub fn set_version(version: &str) -> Result<(), String> {
    create(APP_KEY)?.set(Some("Version"), version)?;
    create(UNINSTALL_KEY)?.set(Some("DisplayVersion"), version)
}

pub fn unregister() -> Result<(), String> {
    let dir = installed().map(|i| i.dir);
    for (ext, _) in FILE_TYPES {
        delete_value(&format!(r"{CLASSES}\.{ext}\OpenWithProgids"), PROG_ID);
    }
    for (key, _, _) in VERBS {
        delete_tree(&format!(r"{CLASSES}\{key}"));
    }
    for key in [format!(r"{CLASSES}\{PROG_ID}"), format!(r"{CLASSES}\Applications\brisk.exe"), APP_PATH_KEY.into(), UNINSTALL_KEY.into(), APP_KEY.into()] {
        delete_tree(&key);
    }
    if let Some(dir) = &dir {
        set_path(dir, false)?;
    }
    shortcuts(Path::new(""), Path::new(""), false, false)?;
    notify_shell();
    Ok(())
}

/// Adds `dir` to the user's PATH or takes it out, and tells running programs.
fn set_path(dir: &Path, on: bool) -> Result<(), String> {
    let (cur, expand) = get_string("Environment", Some("Path")).unwrap_or_default();
    let want = dir.display().to_string();
    let same = |p: &str| p.trim_end_matches('\\').eq_ignore_ascii_case(want.trim_end_matches('\\'));
    let mut parts: Vec<&str> = cur.split(';').filter(|p| !p.is_empty() && !same(p)).collect();
    if on {
        parts.push(&want);
    }
    let new = parts.join(";");
    if new == cur {
        return Ok(());
    }
    create("Environment")?.set_typed(Some("Path"), &new, if expand || new.contains('%') { REG_EXPAND_SZ } else { REG_SZ })?;
    let env = wide("Environment");
    let mut out = 0usize;
    // SAFETY: a broadcast with a string parameter that outlives the call; a hung window is
    // skipped after the timeout.
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE};
        SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, 0, env.as_ptr() as isize, SMTO_ABORTIFHUNG, 2000, &mut out);
    }
    Ok(())
}

/// Creates or removes the Start menu and Desktop shortcuts.
fn shortcuts(exe: &Path, dir: &Path, start_menu: bool, desktop: bool) -> Result<(), String> {
    // Paths go through the environment, never into the script text.
    let script = r#"
$sh = New-Object -ComObject WScript.Shell
foreach ($p in @(@('Programs', $env:BRISK_START), @('Desktop', $env:BRISK_DESKTOP))) {
  $lnk = Join-Path ([Environment]::GetFolderPath($p[0])) 'Brisk.lnk'
  if ($p[1] -eq '1') {
    $s = $sh.CreateShortcut($lnk)
    $s.TargetPath = $env:BRISK_EXE
    $s.WorkingDirectory = $env:BRISK_DIR
    $s.IconLocation = "$($env:BRISK_EXE),0"
    $s.Description = 'Brisk IDE'
    $s.Save()
  } elseif (Test-Path -LiteralPath $lnk) {
    Remove-Item -LiteralPath $lnk -Force
  }
}
"#;
    let ps =
        std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join(r"System32\WindowsPowerShell\v1.0\powershell.exe")).unwrap_or_else(|| "powershell".into());
    let out = crate::process::command(ps)
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .env("BRISK_EXE", exe)
        .env("BRISK_DIR", dir)
        .env("BRISK_START", if start_menu { "1" } else { "0" })
        .env("BRISK_DESKTOP", if desktop { "1" } else { "0" })
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("cannot start PowerShell for the shortcuts: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("cannot create the shortcuts: {}", String::from_utf8_lossy(&out.stderr).lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim()))
    }
}

/// Explorer picks up the new file types and menu entries without a restart.
fn notify_shell() {
    // SAFETY: no items; the event means "associations changed".
    unsafe {
        use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
        SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, std::ptr::null(), std::ptr::null());
    }
}

/// The full path of a process's executable.
pub fn exe_of(pid: u32) -> Option<PathBuf> {
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: a query handle, closed below; the buffer length is passed in and out.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0;
        windows_sys::Win32::Foundation::CloseHandle(h);
        ok.then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

pub fn delete_on_reboot(path: &Path) {
    let p: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    // SAFETY: a valid path; null target means delete.
    unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(
            p.as_ptr(),
            std::ptr::null(),
            windows_sys::Win32::Storage::FileSystem::MOVEFILE_DELAY_UNTIL_REBOOT,
        );
    }
}

pub fn set_manifest(exe: &Path, xml: &str) -> Result<(), String> {
    use windows_sys::Win32::System::LibraryLoader::{BeginUpdateResourceW, EndUpdateResourceW, UpdateResourceW};
    let p: Vec<u16> = exe.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    // RT_MANIFEST is 24 and the manifest's name is 1, both passed as integer resources;
    // 0x0409 is the language GPUI's manifest was compiled with, so this replaces it.
    // SAFETY: the update handle is ended on every path; the data outlives the calls.
    unsafe {
        let h = BeginUpdateResourceW(p.as_ptr(), 0);
        if h.is_null() {
            return Err(format!("cannot open {} for resource update: {}", exe.display(), std::io::Error::last_os_error()));
        }
        let ok = UpdateResourceW(h, std::ptr::without_provenance(24), std::ptr::without_provenance(1), 0x0409, xml.as_ptr() as *const _, xml.len() as u32) != 0;
        if EndUpdateResourceW(h, if ok { 0 } else { 1 }) == 0 || !ok {
            return Err(format!("cannot write the manifest: {}", std::io::Error::last_os_error()));
        }
    }
    Ok(())
}
