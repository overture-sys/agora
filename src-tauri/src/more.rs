//! More of Agora's own actions: audio outputs and per-app volume, window
//! tools, web requests, PowerShell commands, and app icons for Open keys.
//! Written for Agora; nothing here comes from another app.

// Windows' COM method names keep their own spelling.
#![allow(non_snake_case)]

use serde::Serialize;
use serde_json::Value;

/// Actions that finish at once. Web requests and commands go through
/// [`run_with_result`] instead, on their own thread.
pub fn run(action: &str, settings: &Value) -> Option<Result<(), String>> {
    let s = |k: &str| settings.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    Some(match action {
        "audio-output" => {
            if s("mode") == "set" {
                win::set_output(&s("device"))
            } else {
                win::next_output()
            }
        }
        "app-volume" => win::app_volume(
            &s("app"),
            &s("what"),
            settings.get("step").and_then(Value::as_i64).unwrap_or(10).clamp(1, 100) as f32 / 100.0,
        ),
        "window" => win::window(&s("what")),
        _ => return None,
    })
}

/// Runs a web request or command. Ok(text) is shown on the key when
/// there is text; Err is shown as the reason it failed.
pub fn run_with_result(action: &str, settings: &Value) -> Result<Option<String>, String> {
    let s = |k: &str| settings.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    match action {
        "web-request" => web_request(&s("method"), &s("url"), &s("body"), &s("content_type")).map(|_| None),
        "run-command" => {
            let out = run_command(&s("command"))?;
            let show = settings.get("show_output").and_then(Value::as_bool).unwrap_or(true);
            Ok(if show && !out.trim().is_empty() { Some(out) } else { None })
        }
        _ => Err(format!("Unknown action {action}")),
    }
}

#[cfg(windows)]
fn hidden(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hidden(_: &mut std::process::Command) {}

/// Waits for a process, giving up (and stopping it) after `secs`.
fn finish(mut child: std::process::Child, secs: u64) -> Result<std::process::Output, String> {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().map_err(|e| e.to_string()),
            Ok(None) if std::time::Instant::now() < end => {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            Ok(None) => {
                let _ = child.kill();
                return Err("Took too long".into());
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Only http and https addresses, so nothing else can sneak in as a curl
/// option or another kind of address.
pub fn check_url(url: &str) -> Result<(), String> {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) || u.len() < 10 {
        return Err("The address must start with http:// or https://".into());
    }
    if u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("The address has spaces in it".into());
    }
    Ok(())
}

/// Sends the request with the curl that comes with Windows 10 and 11.
fn web_request(method: &str, url: &str, body: &str, content_type: &str) -> Result<(), String> {
    check_url(url)?;
    let method = match method.to_ascii_uppercase().as_str() {
        m @ ("GET" | "POST" | "PUT" | "PATCH" | "DELETE") => m.to_string(),
        _ => "POST".into(),
    };
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let mut cmd = std::process::Command::new(if cfg!(windows) { "curl.exe" } else { "curl" });
    cmd.args(["-s", "-S", "-o", null, "-w", "%{http_code}", "-m", "15", "-X", &method]);
    let has_body = !body.is_empty() && method != "GET";
    if has_body {
        let ct = if content_type.trim().is_empty() { "application/json" } else { content_type.trim() };
        let ct: String = ct.chars().filter(|c| !c.is_control()).collect();
        cmd.args(["-H", &format!("Content-Type: {ct}"), "--data-binary", "@-"]);
    }
    cmd.arg(url.trim())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    hidden(&mut cmd);
    let mut child = cmd.spawn().map_err(|_| "curl isn't available".to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        if has_body {
            let _ = stdin.write_all(body.as_bytes());
        }
    }
    let out = finish(child, 20)?;
    let code: u16 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0);
    match code {
        200..=399 => Ok(()),
        0 => Err("No answer".into()),
        c => Err(format!("HTTP {c}")),
    }
}

/// Runs a PowerShell command (a shell command elsewhere) and returns what
/// it printed.
fn run_command(command: &str) -> Result<String, String> {
    if command.trim().is_empty() {
        return Err("No command".into());
    }
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("powershell.exe");
        c.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &format!("[Console]::OutputEncoding=[Text.Encoding]::UTF8; {command}"),
        ]);
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.args(["-c", command]);
        c
    };
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    hidden(&mut cmd);
    let child = cmd.spawn().map_err(|e| e.to_string())?;
    let out = finish(child, 60)?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    if out.status.success() {
        Ok(text)
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("The command failed");
        Err(line.trim().chars().take(80).collect())
    }
}

#[derive(Serialize, Clone)]
pub struct Output {
    pub id: String,
    pub name: String,
    pub default: bool,
}

#[cfg(windows)]
pub mod win {
    use super::Output;
    use std::ffi::c_void;
    use windows::core::{Interface, GUID, HRESULT, PCWSTR};
    use windows::Win32::Media::Audio::{
        eConsole, eMultimedia, eRender, ERole, IAudioSessionControl2, IAudioSessionManager2,
        IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
    };

    // Windows' own control for the default sound device. It isn't in the
    // published headers; this is its well-known shape, written out here.
    #[windows::core::interface("f8679f50-850a-41cf-9c72-430f290290c8")]
    unsafe trait IPolicyConfig: windows::core::IUnknown {
        fn GetMixFormat(&self, a: PCWSTR, b: *mut c_void) -> HRESULT;
        fn GetDeviceFormat(&self, a: PCWSTR, b: i32, c: *mut c_void) -> HRESULT;
        fn ResetDeviceFormat(&self, a: PCWSTR) -> HRESULT;
        fn SetDeviceFormat(&self, a: PCWSTR, b: *mut c_void, c: *mut c_void) -> HRESULT;
        fn GetProcessingPeriod(&self, a: PCWSTR, b: i32, c: *mut i64, d: *mut i64) -> HRESULT;
        fn SetProcessingPeriod(&self, a: PCWSTR, b: *mut i64) -> HRESULT;
        fn GetShareMode(&self, a: PCWSTR, b: *mut c_void) -> HRESULT;
        fn SetShareMode(&self, a: PCWSTR, b: *mut c_void) -> HRESULT;
        fn GetPropertyValue(&self, a: PCWSTR, b: *const c_void, c: *mut c_void) -> HRESULT;
        fn SetPropertyValue(&self, a: PCWSTR, b: *const c_void, c: *mut c_void) -> HRESULT;
        fn SetDefaultEndpoint(&self, id: PCWSTR, role: ERole) -> HRESULT;
        fn SetEndpointVisibility(&self, a: PCWSTR, b: i32) -> HRESULT;
    }

    const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

    fn err(e: windows::core::Error) -> String {
        e.message().to_string()
    }

    fn enumerator() -> Result<IMMDeviceEnumerator, String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(err)
        }
    }

    unsafe fn take_string(p: windows::core::PWSTR) -> String {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const c_void));
        s
    }

    pub fn outputs() -> Result<Vec<Output>, String> {
        use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
        let e = enumerator()?;
        unsafe {
            let default = e
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .ok()
                .and_then(|d| d.GetId().ok())
                .map(|p| take_string(p))
                .unwrap_or_default();
            let list = e.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE).map_err(err)?;
            let mut out = Vec::new();
            for i in 0..list.GetCount().map_err(err)? {
                let Ok(d) = list.Item(i) else { continue };
                let Ok(id) = d.GetId().map(|p| take_string(p)) else { continue };
                let name = d
                    .OpenPropertyStore(STGM_READ)
                    .and_then(|s| s.GetValue(&PKEY_Device_FriendlyName))
                    .map(|v| v.to_string())
                    .unwrap_or_else(|_| "Sound output".into());
                out.push(Output { default: id == default, id, name });
            }
            Ok(out)
        }
    }

    pub fn set_output(id: &str) -> Result<(), String> {
        if id.is_empty() {
            return Err("Pick an output in the key's settings".into());
        }
        let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let policy: IPolicyConfig =
                CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL).map_err(err)?;
            for role in [eConsole, eMultimedia, windows::Win32::Media::Audio::eCommunications] {
                policy.SetDefaultEndpoint(PCWSTR(wide.as_ptr()), role).ok().map_err(err)?;
            }
        }
        Ok(())
    }

    pub fn next_output() -> Result<(), String> {
        let list = outputs()?;
        if list.len() < 2 {
            return Err("Only one sound output".into());
        }
        let at = list.iter().position(|o| o.default).unwrap_or(0);
        set_output(&list[(at + 1) % list.len()].id)
    }

    /// The file name of a running program, such as "spotify.exe".
    fn process_name(pid: u32) -> Option<String> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return None;
            }
            let mut buf = [0u16; 520];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
            CloseHandle(h);
            if ok == 0 {
                return None;
            }
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            path.rsplit(['\\', '/']).next().map(|s| s.to_ascii_lowercase())
        }
    }

    /// Every app playing (or able to play) sound now, with its volume control.
    fn sessions() -> Result<Vec<(String, ISimpleAudioVolume)>, String> {
        let e = enumerator()?;
        let mut out = Vec::new();
        unsafe {
            let list = e.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE).map_err(err)?;
            for i in 0..list.GetCount().map_err(err)? {
                let Ok(dev) = list.Item(i) else { continue };
                let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
                let Ok(all) = mgr.GetSessionEnumerator() else { continue };
                for j in 0..all.GetCount().unwrap_or(0) {
                    let Ok(ctl) = all.GetSession(j) else { continue };
                    let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() else { continue };
                    let pid = ctl2.GetProcessId().unwrap_or(0);
                    if pid == 0 {
                        continue;
                    }
                    let (Some(name), Ok(vol)) = (process_name(pid), ctl.cast::<ISimpleAudioVolume>()) else {
                        continue;
                    };
                    out.push((name, vol));
                }
            }
        }
        Ok(out)
    }

    pub fn audio_apps() -> Vec<String> {
        let mut names: Vec<String> = sessions().unwrap_or_default().into_iter().map(|(n, _)| n).collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn app_volume(app: &str, what: &str, step: f32) -> Result<(), String> {
        let want = app.trim().to_ascii_lowercase();
        if want.is_empty() {
            return Err("Pick an app in the key's settings".into());
        }
        let want = if want.ends_with(".exe") { want } else { format!("{want}.exe") };
        let mut found = false;
        for (name, vol) in sessions()? {
            if name != want {
                continue;
            }
            found = true;
            unsafe {
                match what {
                    "mute" => {
                        let m = vol.GetMute().map_err(err)?.as_bool();
                        vol.SetMute(!m, std::ptr::null()).map_err(err)?;
                    }
                    "down" | "up" => {
                        let v = vol.GetMasterVolume().map_err(err)?;
                        let v = if what == "up" { v + step } else { v - step };
                        vol.SetMasterVolume(v.clamp(0.0, 1.0), std::ptr::null()).map_err(err)?;
                        if what == "up" {
                            let _ = vol.SetMute(false, std::ptr::null());
                        }
                    }
                    _ => {}
                }
            }
        }
        if found {
            Ok(())
        } else {
            Err(format!("{app} isn't playing sound"))
        }
    }

    pub fn window(what: &str) -> Result<(), String> {
        use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
        use windows_sys::Win32::Graphics::Gdi::{
            EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow, HDC, HMONITOR, MONITORINFO,
            MONITOR_DEFAULTTONEAREST,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetClassNameW, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, IsZoomed,
            SetWindowPos, ShowWindow, GWL_EXSTYLE, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE,
            SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, WS_EX_TOPMOST,
        };
        unsafe {
            let w: HWND = GetForegroundWindow();
            if w.is_null() {
                return Err("No window in front".into());
            }
            let mut buf = [0u16; 64];
            let n = GetClassNameW(w, buf.as_mut_ptr(), buf.len() as i32);
            let class = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
            if matches!(class.as_str(), "Shell_TrayWnd" | "Progman" | "WorkerW") {
                return Err("The desktop is in front".into());
            }
            let work = |m: HMONITOR| -> RECT {
                let mut info: MONITORINFO = std::mem::zeroed();
                info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                GetMonitorInfoW(m, &mut info);
                info.rcWork
            };
            let here = MonitorFromWindow(w, MONITOR_DEFAULTTONEAREST);
            match what {
                "maximize" => {
                    ShowWindow(w, if IsZoomed(w) != 0 { SW_RESTORE } else { SW_MAXIMIZE });
                }
                "minimize" => {
                    ShowWindow(w, SW_MINIMIZE);
                }
                "on-top" => {
                    let top = GetWindowLongPtrW(w, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0;
                    SetWindowPos(
                        w,
                        if top { HWND_NOTOPMOST } else { HWND_TOPMOST },
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
                "snap-left" | "snap-right" | "snap-top" | "snap-bottom" => {
                    if IsZoomed(w) != 0 {
                        ShowWindow(w, SW_RESTORE);
                    }
                    let r = work(here);
                    let (rw, rh) = (r.right - r.left, r.bottom - r.top);
                    let (x, y, cw, ch) = match what {
                        "snap-left" => (r.left, r.top, rw / 2, rh),
                        "snap-right" => (r.left + rw / 2, r.top, rw - rw / 2, rh),
                        "snap-top" => (r.left, r.top, rw, rh / 2),
                        _ => (r.left, r.top + rh / 2, rw, rh - rh / 2),
                    };
                    SetWindowPos(w, std::ptr::null_mut(), x, y, cw, ch, SWP_NOZORDER | SWP_NOACTIVATE);
                }
                _ => {
                    // Over to the next screen, keeping its place and size.
                    unsafe extern "system" fn add(m: HMONITOR, _: HDC, _: *mut RECT, list: LPARAM) -> i32 {
                        (*(list as *mut Vec<HMONITOR>)).push(m);
                        1
                    }
                    let mut list: Vec<HMONITOR> = Vec::new();
                    EnumDisplayMonitors(
                        std::ptr::null_mut(),
                        std::ptr::null(),
                        Some(add),
                        &mut list as *mut Vec<HMONITOR> as LPARAM,
                    );
                    if list.len() < 2 {
                        return Err("Only one screen".into());
                    }
                    let at = list.iter().position(|m| *m == here).unwrap_or(0);
                    let (from, to) = (work(here), work(list[(at + 1) % list.len()]));
                    let zoomed = IsZoomed(w) != 0;
                    if zoomed {
                        ShowWindow(w, SW_RESTORE);
                    }
                    let mut wr: RECT = std::mem::zeroed();
                    GetWindowRect(w, &mut wr);
                    let fw = (from.right - from.left).max(1) as f32;
                    let fh = (from.bottom - from.top).max(1) as f32;
                    let tw = (to.right - to.left) as f32;
                    let th = (to.bottom - to.top) as f32;
                    let (sx, sy) = (tw / fw, th / fh);
                    let x = to.left + ((wr.left - from.left) as f32 * sx) as i32;
                    let y = to.top + ((wr.top - from.top) as f32 * sy) as i32;
                    let cw = (((wr.right - wr.left) as f32) * sx) as i32;
                    let ch = (((wr.bottom - wr.top) as f32) * sy) as i32;
                    SetWindowPos(w, std::ptr::null_mut(), x, y, cw, ch, SWP_NOZORDER | SWP_NOACTIVATE);
                    if zoomed {
                        ShowWindow(w, SW_MAXIMIZE);
                    }
                }
            }
        }
        Ok(())
    }

    /// A program's own icon, 128 pixels, as RGBA.
    pub fn app_icon(path: &str) -> Option<image::RgbaImage> {
        use windows_sys::Win32::Graphics::Gdi::{
            DeleteObject, GetDC, GetDIBits, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
            DIB_RGB_COLORS,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DestroyIcon, GetIconInfo, PrivateExtractIconsW, HICON, ICONINFO,
        };
        const SIZE: i32 = 128;
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let mut icon: HICON = std::ptr::null_mut();
            let mut id = 0u32;
            let got = PrivateExtractIconsW(wide.as_ptr(), 0, SIZE, SIZE, &mut icon, &mut id, 1, 0);
            if got == 0 || got == u32::MAX || icon.is_null() {
                return None;
            }
            let mut info: ICONINFO = std::mem::zeroed();
            if GetIconInfo(icon, &mut info) == 0 {
                DestroyIcon(icon);
                return None;
            }
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: SIZE,
                biHeight: -SIZE,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let mut px = vec![0u8; (SIZE * SIZE * 4) as usize];
            let dc = GetDC(std::ptr::null_mut());
            let lines = GetDIBits(
                dc,
                info.hbmColor,
                0,
                SIZE as u32,
                px.as_mut_ptr() as *mut _,
                &mut bmi,
                DIB_RGB_COLORS,
            );
            ReleaseDC(std::ptr::null_mut(), dc);
            DeleteObject(info.hbmColor as _);
            DeleteObject(info.hbmMask as _);
            DestroyIcon(icon);
            if lines == 0 {
                return None;
            }
            // BGRA to RGBA; old icons without alpha are made opaque.
            let no_alpha = px.chunks(4).all(|p| p[3] == 0);
            for p in px.chunks_mut(4) {
                p.swap(0, 2);
                if no_alpha {
                    p[3] = 255;
                }
            }
            image::RgbaImage::from_raw(SIZE as u32, SIZE as u32, px)
        }
    }
}

#[cfg(not(windows))]
pub mod win {
    use super::Output;
    const NO: &str = "Only on Windows";
    pub fn outputs() -> Result<Vec<Output>, String> {
        Err(NO.into())
    }
    pub fn set_output(_: &str) -> Result<(), String> {
        Err(NO.into())
    }
    pub fn next_output() -> Result<(), String> {
        Err(NO.into())
    }
    pub fn audio_apps() -> Vec<String> {
        Vec::new()
    }
    pub fn app_volume(_: &str, _: &str, _: f32) -> Result<(), String> {
        Err(NO.into())
    }
    pub fn window(_: &str) -> Result<(), String> {
        Err(NO.into())
    }
    pub fn app_icon(_: &str) -> Option<image::RgbaImage> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_addresses_are_requested() {
        assert!(check_url("https://example.com/hook").is_ok());
        assert!(check_url("http://192.168.1.5:8123/api").is_ok());
        assert!(check_url("-o evil https://x.com").is_err());
        assert!(check_url("file:///etc/passwd").is_err());
        assert!(check_url("https://a b.com").is_err());
    }

    #[test]
    fn commands_print_and_fail() {
        if cfg!(windows) {
            return;
        }
        assert_eq!(run_command("echo hi").unwrap().trim(), "hi");
        assert!(run_command("exit 3").is_err());
        assert!(run_command("  ").is_err());
    }
}
