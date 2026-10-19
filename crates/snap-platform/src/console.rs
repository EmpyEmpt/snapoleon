//! Console attachment for the Windows GUI-subsystem binary when it is used as a CLI.

/// Attach to the parent process's console and point the standard handles at it,
/// so `println!` from a GUI-subsystem executable shows up in the terminal that
/// launched it. Handles already redirected by the parent (pipes, files) are kept.
/// No-op when there is no parent console and on non-Windows platforms.
pub fn attach() {
    #[cfg(windows)]
    imp::attach();
}

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
    };
    use windows::core::w;

    pub fn attach() {
        // SAFETY: plain Win32 console calls; handles we open stay valid for the process lifetime.
        unsafe {
            if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
                return;
            }
            rebind(STD_OUTPUT_HANDLE, true);
            rebind(STD_ERROR_HANDLE, true);
            rebind(STD_INPUT_HANDLE, false);
        }
    }

    /// A GUI-subsystem process starts with null standard handles unless the
    /// parent redirected them; open the console device in that case.
    unsafe fn rebind(which: STD_HANDLE, output: bool) {
        unsafe {
            let current = GetStdHandle(which).unwrap_or(INVALID_HANDLE_VALUE);
            if !current.is_invalid() && current != HANDLE::default() {
                return;
            }
            let (device, access) = if output { (w!("CONOUT$"), GENERIC_READ.0 | GENERIC_WRITE.0) } else { (w!("CONIN$"), GENERIC_READ.0) };
            if let Ok(handle) = CreateFileW(device, access, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, None) {
                let _ = SetStdHandle(which, handle);
            }
        }
    }
}
