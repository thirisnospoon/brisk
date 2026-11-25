//! Every long-lived child Brisk starts (ty, ruff, debugpy, runs, terminals, consoles, git)
//! is put into one Job Object created with `KILL_ON_JOB_CLOSE`. Only Brisk holds the job
//! handle, so when Brisk exits for any reason (quit, crash, `taskkill`, project switch)
//! Windows kills the whole job, grandchildren included. Brisk itself is not in the job, so
//! a browser or a new Brisk window it opens is unaffected.
//!
//! Elsewhere `adopt` is a no-op: Unix children already get their own process group and are
//! signalled on stop.

/// Put a freshly spawned child into the job.
pub fn adopt(child: &std::process::Child) {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        imp::assign(child.as_raw_handle());
    }
    #[cfg(not(windows))]
    let _ = child;
}

/// Same for a child known only by pid (the ConPTY shell of a terminal).
pub fn adopt_pid(pid: u32) {
    #[cfg(windows)]
    imp::assign_pid(pid);
    #[cfg(not(windows))]
    let _ = pid;
}

/// Kill a child and every process it started (`Child::kill` stops only the child, leaving
/// e.g. debugpy's launcher or a console's subprocesses running). The caller holds the
/// child's handle, so its pid cannot have been reused yet.
pub fn kill_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    if matches!(child.try_wait(), Ok(None)) {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .status();
    }
    let _ = child.kill();
}

/// Number of live processes in the job (for the memory/handle smoke). `None` off Windows or
/// before any child was adopted.
pub fn live_children() -> Option<u32> {
    #[cfg(windows)]
    return imp::active();
    #[cfg(not(windows))]
    None
}

/// This process's working set and private bytes, in bytes.
pub fn memory() -> Option<(u64, u64)> {
    #[cfg(windows)]
    return imp::memory();
    #[cfg(not(windows))]
    None
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::*;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

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

    pub fn assign(process: *mut c_void) {
        if let Some(j) = job() {
            // Fails only if the child already exited or sits in a job that forbids nesting.
            unsafe { AssignProcessToJobObject(j, process as HANDLE) };
        }
    }

    pub fn assign_pid(pid: u32) {
        let Some(j) = job() else { return };
        unsafe {
            let p = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if !p.is_null() {
                AssignProcessToJobObject(j, p);
                CloseHandle(p);
            }
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

    pub fn active() -> Option<u32> {
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
}

#[cfg(test)]
mod tests {
    #[test]
    fn adopted_child_is_counted() {
        #[cfg(windows)]
        {
            let mut c = std::process::Command::new("cmd").args(["/C", "ping -n 3 127.0.0.1 >NUL"]).spawn().unwrap();
            super::adopt(&c);
            assert!(super::live_children().unwrap() >= 1);
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// The grandchild (ping under cmd) dies too: it holds the stdout pipe, so EOF comes only
    /// once it is gone.
    #[test]
    fn kill_tree_takes_grandchildren() {
        #[cfg(windows)]
        {
            use std::io::Read;
            let mut c = std::process::Command::new("cmd")
                .args(["/C", "ping -n 30 127.0.0.1"])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(500));
            let mut out = c.stdout.take().unwrap();
            let t = std::time::Instant::now();
            super::kill_tree(&mut c);
            let _ = c.wait();
            let mut sink = Vec::new();
            let _ = out.read_to_end(&mut sink);
            assert!(t.elapsed() < std::time::Duration::from_secs(10), "grandchild survived");
        }
    }
}
