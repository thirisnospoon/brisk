//! Can this machine run the engine's VM? Windows Hypervisor Platform (WHP) must be turned on
//! and the hypervisor running; WHP lets an unelevated process create VMs, so administrator
//! rights are needed only to turn the feature on.

/// What stands between the machine and a running VM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Virtualization {
    /// WHP is on and the hypervisor runs: VMs can start.
    Ready,
    /// The Windows feature "Windows Hypervisor Platform" is off: [`enable`] turns it on.
    FeatureOff,
    /// The feature is installed but the hypervisor is not running yet: Windows needs a
    /// restart (or `hypervisorlaunchtype` is off in the boot configuration).
    RestartPending,
    /// Virtualization (Intel VT-x / AMD-V) is off in the firmware (BIOS / UEFI) settings;
    /// only the user can turn it on there.
    FirmwareOff,
    /// Not Windows, or a Windows without WHP.
    Unsupported(String),
}

impl Virtualization {
    /// One sentence for the setup card.
    pub fn describe(&self) -> String {
        match self {
            Virtualization::Ready => "Virtualization is ready.".into(),
            Virtualization::FeatureOff => "Containers need the Windows feature \"Windows Hypervisor Platform\", which is off.".into(),
            Virtualization::RestartPending => "Windows Hypervisor Platform is installed, but Windows has to restart before it works.".into(),
            Virtualization::FirmwareOff => {
                "Virtualization is turned off in this computer's BIOS / UEFI settings (Intel VT-x or AMD-V / SVM). Turn it on there, then start Windows again."
                    .into()
            }
            Virtualization::Unsupported(why) => format!("Containers cannot run here: {why}"),
        }
    }
}

/// The command [`enable`] runs elevated: turns on Windows Hypervisor Platform without
/// restarting (the user chooses when).
pub fn enable_command() -> (&'static str, &'static str) {
    ("dism.exe", "/online /enable-feature /featurename:HypervisorPlatform /all /norestart /quiet")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnableOutcome {
    /// Turned on; Windows must restart before VMs can start.
    RestartRequired,
    /// Turned on and usable without a restart (it was already on).
    Enabled,
    /// The user said No in the UAC prompt.
    Cancelled,
    Failed(String),
}

/// Maps DISM's exit code.
fn outcome_of(code: u32) -> EnableOutcome {
    match code {
        0 => EnableOutcome::Enabled,
        3010 => EnableOutcome::RestartRequired,
        1223 => EnableOutcome::Cancelled,
        c => EnableOutcome::Failed(format!("DISM exited with code {c}")),
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{CloseHandle, FreeLibrary, GetLastError};
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, IsProcessorFeaturePresent, WaitForSingleObject, INFINITE, PF_VIRT_FIRMWARE_ENABLED};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// `WHvGetCapability(WHvCapabilityCodeHypervisorPresent)` through a dynamically loaded
    /// WinHvPlatform.dll: `None` when the DLL (the feature) is missing.
    fn hypervisor_present() -> Option<bool> {
        type GetCapability = unsafe extern "system" fn(code: i32, buf: *mut c_void, size: u32, written: *mut u32) -> i32;
        unsafe {
            let dll = LoadLibraryExW(wide("WinHvPlatform.dll").as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
            if dll.is_null() {
                return None;
            }
            let present = GetProcAddress(dll, c"WHvGetCapability".as_ptr().cast()).map(|f| {
                let f: GetCapability = std::mem::transmute(f);
                let mut value: u64 = 0;
                let mut written: u32 = 0;
                let hr = f(0, (&mut value as *mut u64).cast(), 8, &mut written);
                hr == 0 && value != 0
            });
            FreeLibrary(dll);
            present
        }
    }

    pub fn check() -> Virtualization {
        match hypervisor_present() {
            Some(true) => Virtualization::Ready,
            // With a hypervisor running, the firmware flag is not visible to Windows; without
            // one it tells whether the CPU's virtualization is turned on in the firmware.
            _ if unsafe { IsProcessorFeaturePresent(PF_VIRT_FIRMWARE_ENABLED) } == 0 => Virtualization::FirmwareOff,
            Some(false) => Virtualization::RestartPending,
            None => Virtualization::FeatureOff,
        }
    }

    pub fn enable() -> EnableOutcome {
        let (program, params) = enable_command();
        let (verb, file, params) = (wide("runas"), wide(program), wide(params));
        unsafe {
            let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
            info.fMask = SEE_MASK_NOCLOSEPROCESS;
            info.lpVerb = verb.as_ptr();
            info.lpFile = file.as_ptr();
            info.lpParameters = params.as_ptr();
            info.nShow = SW_HIDE;
            if ShellExecuteExW(&mut info) == 0 {
                return outcome_of(GetLastError());
            }
            if info.hProcess.is_null() {
                return EnableOutcome::Failed("DISM did not start".into());
            }
            WaitForSingleObject(info.hProcess, INFINITE);
            let mut code: u32 = 0;
            let ok = GetExitCodeProcess(info.hProcess, &mut code) != 0;
            CloseHandle(info.hProcess);
            if ok {
                outcome_of(code)
            } else {
                EnableOutcome::Failed("cannot read DISM's exit code".into())
            }
        }
    }
}

/// Whether VMs can start now, and if not, why. Fast (no process is started).
pub fn check() -> Virtualization {
    #[cfg(windows)]
    {
        win::check()
    }
    #[cfg(not(windows))]
    {
        Virtualization::Unsupported("the engine is built for Windows only".into())
    }
}

/// Turns on Windows Hypervisor Platform: one UAC prompt, then DISM (tens of seconds). Blocks:
/// call it off the UI thread, and only when the user asked for it.
pub fn enable() -> EnableOutcome {
    #[cfg(windows)]
    {
        win::enable()
    }
    #[cfg(not(windows))]
    {
        EnableOutcome::Failed("not Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dism_exit_codes() {
        assert_eq!(outcome_of(0), EnableOutcome::Enabled);
        assert_eq!(outcome_of(3010), EnableOutcome::RestartRequired);
        assert_eq!(outcome_of(1223), EnableOutcome::Cancelled);
        assert!(matches!(outcome_of(5), EnableOutcome::Failed(_)));
    }

    #[test]
    fn enable_turns_on_only_the_hypervisor_platform() {
        let (program, args) = enable_command();
        assert_eq!(program, "dism.exe");
        assert!(args.contains("/featurename:HypervisorPlatform") && args.contains("/norestart"));
    }

    /// On the development machine (WHP on) this reports Ready; elsewhere it must not panic.
    #[test]
    fn check_does_not_panic() {
        let v = check();
        eprintln!("virtualization: {v:?}");
        assert!(!v.describe().is_empty());
    }
}
