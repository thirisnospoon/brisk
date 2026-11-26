use std::ffi::c_void;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// HANDLE is a raw pointer (not Send); the job lives for the whole process, so keep it as usize.
static JOB: OnceLock<usize> = OnceLock::new();

fn job() -> Option<HANDLE> {
    let h = *JOB.get_or_init(|| unsafe {
        let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if h.is_null() {
            return 0;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        SetInformationJobObject(
            h,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        h as usize
    });
    (h != 0).then_some(h as HANDLE)
}

pub fn no_window(c: &mut Command) -> &mut Command {
    c.creation_flags(CREATE_NO_WINDOW)
}

pub fn adopt(child: &Child) {
    if let Some(j) = job() {
        // Fails only if the child already exited or sits in a job that forbids nesting.
        unsafe { AssignProcessToJobObject(j, child.as_raw_handle() as HANDLE) };
    }
}

pub fn adopt_pid(pid: u32) {
    let Some(j) = job() else { return };
    unsafe {
        let p = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
        if !p.is_null() {
            AssignProcessToJobObject(j, p);
            CloseHandle(p);
        }
    }
}

pub fn kill_tree(child: &mut Child) {
    if matches!(child.try_wait(), Ok(None)) {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
}

pub fn memory() -> Option<(u64, u64)> {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let ok = GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, c.cb);
        (ok != 0).then_some((c.WorkingSetSize as u64, c.PrivateUsage as u64))
    }
}

pub fn live_children() -> Option<u32> {
    let h = *JOB.get()?;
    if h == 0 {
        return None;
    }
    unsafe {
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
        let ok = QueryInformationJobObject(
            h as HANDLE,
            JobObjectBasicAccountingInformation,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
            std::ptr::null_mut(),
        );
        (ok != 0).then_some(info.ActiveProcesses)
    }
}

pub fn processes() -> Vec<super::ProcessInfo> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..len]);
            let working_set = memory_of(e.th32ProcessID).map_or(0, |m| m.0);
            out.push(super::ProcessInfo { pid: e.th32ProcessID, parent: e.th32ParentProcessID, name, working_set });
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    out
}

pub fn memory_of(pid: u32) -> Option<(u64, u64)> {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut c: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let ok = GetProcessMemoryInfo(h, &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, c.cb);
        CloseHandle(h);
        (ok != 0).then_some((c.WorkingSetSize as u64, c.PrivateUsage as u64))
    }
}

pub fn start_unix_ms(child: &Child) -> Option<u64> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetProcessTimes;
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: the handle is the child's own and stays open for the call.
    let ok = unsafe { GetProcessTimes(child.as_raw_handle() as HANDLE, &mut created, &mut exited, &mut kernel, &mut user) };
    if ok == 0 {
        return None;
    }
    // 100 ns ticks since 1601-01-01.
    let ticks = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
    ticks.checked_sub(116_444_736_000_000_000).map(|t| t / 10_000)
}

pub fn has_window(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindow, GetWindowThreadProcessId, IsWindowVisible, GW_OWNER};
    struct Search {
        pid: u32,
        found: bool,
    }
    unsafe extern "system" fn visit(h: HWND, l: LPARAM) -> BOOL {
        // SAFETY: `l` is the `Search` below, alive for the whole EnumWindows call.
        let s = unsafe { &mut *(l as *mut Search) };
        let mut owner = 0u32;
        unsafe { GetWindowThreadProcessId(h, &mut owner) };
        if owner == s.pid && unsafe { IsWindowVisible(h) } != 0 && unsafe { GetWindow(h, GW_OWNER) }.is_null() {
            s.found = true;
            return 0;
        }
        1
    }
    let mut s = Search { pid, found: false };
    unsafe { EnumWindows(Some(visit), &mut s as *mut Search as LPARAM) };
    s.found
}
