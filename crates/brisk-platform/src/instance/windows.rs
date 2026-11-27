//! A named pipe per window, `\\.\pipe\brisk-<pid>`: inbound only, local clients only, with
//! the default security (only this user and administrators may write to it).

use std::io::Write;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, PIPE_ACCESS_INBOUND};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};

fn name(pid: u32) -> String {
    format!(r"\\.\pipe\brisk-{pid}")
}

pub fn serve(pid: u32, on: impl Fn(String) + Send + 'static) {
    let path: Vec<u16> = name(pid).encode_utf16().chain(Some(0)).collect();
    let _ = std::thread::Builder::new().name("instance-pipe".into()).spawn(move || loop {
        // SAFETY: a valid name; the handle is closed below on every path.
        let h = unsafe {
            CreateNamedPipeW(
                path.as_ptr(),
                PIPE_ACCESS_INBOUND,
                PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                0,
                8192,
                0,
                std::ptr::null(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return;
        }
        // SAFETY: a fresh pipe handle; blocks until a client connects.
        let ok = unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) } != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        if ok {
            let mut msg = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let mut n = 0u32;
                // SAFETY: reads into `buf`; stops at the client's close.
                let r = unsafe { ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut n, std::ptr::null_mut()) };
                if r == 0 || n == 0 || msg.len() > 64 * 1024 {
                    break;
                }
                msg.extend_from_slice(&buf[..n as usize]);
            }
            on(String::from_utf8_lossy(&msg).into_owned());
        }
        // SAFETY: the handle from CreateNamedPipeW.
        unsafe {
            DisconnectNamedPipe(h);
            CloseHandle(h);
        }
    });
}

pub fn alive(pid: u32) -> bool {
    std::path::Path::new(&name(pid)).exists() || {
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        // SAFETY: only a query; the handle is closed at once.
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h.is_null() {
            false
        } else {
            let mut code = 0u32;
            // SAFETY: a valid handle.
            let running = unsafe { windows_sys::Win32::System::Threading::GetExitCodeProcess(h, &mut code) } != 0 && code == 259;
            unsafe { CloseHandle(h) };
            running
        }
    }
}

pub fn send(pid: u32, msg: &str) -> bool {
    // The window may take the foreground: this process got it from the user's click.
    // SAFETY: no pointers.
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid) };
    match std::fs::OpenOptions::new().write(true).open(name(pid)) {
        Ok(mut f) => f.write_all(msg.as_bytes()).is_ok(),
        Err(_) => false,
    }
}
