//! Which app is in front, for profiles that follow apps.

/// The file name of the program that owns the window in front, such as
/// "obs64.exe". `None` if Windows won't say (e.g. an elevated app).
#[cfg(windows)]
pub fn app() -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId,
    };

    // SAFETY: plain Win32 calls; the buffer outlives the call and its size is passed.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(process);
        if ok == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit(['\\', '/']).next().map(str::to_lowercase)
    }
}

#[cfg(not(windows))]
pub fn app() -> Option<String> {
    None
}

/// The window in front now, to bring back later (0 if none).
#[cfg(windows)]
pub fn front_window() -> isize {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize }
}

/// Puts a window from [`front_window`] back in front.
#[cfg(windows)]
pub fn bring_back(w: isize) {
    if w != 0 {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(w as _);
        }
    }
}

/// Where the mouse is, in screen pixels.
#[cfg(windows)]
pub fn cursor() -> (i32, i32) {
    let mut p = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

#[cfg(not(windows))]
pub fn front_window() -> isize {
    0
}

#[cfg(not(windows))]
pub fn bring_back(_: isize) {}

#[cfg(not(windows))]
pub fn cursor() -> (i32, i32) {
    (200, 200)
}
