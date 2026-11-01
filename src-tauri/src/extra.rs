//! More of Agora's own system actions: quick controls and tools, closing
//! the front window, mouse, browser keys, saved passwords, a soundboard
//! and UDP messages. Written for Agora; on Windows they use the system's
//! own calls.

use std::net::{ToSocketAddrs, UdpSocket};

use serde_json::Value;

use crate::system::{self, parse_hotkey};

/// Runs one of the actions in this file. `None` means the action isn't one
/// of these.
pub fn run(action: &str, settings: &Value) -> Option<Result<(), String>> {
    let s = |k: &str| {
        settings
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let b = |k: &str| settings.get(k).and_then(Value::as_bool).unwrap_or(false);
    let n = |k: &str, d: i64| settings.get(k).and_then(Value::as_i64).unwrap_or(d);
    Some(match action {
        "quick-control" => quick_control(&s("what")),
        "quick-tool" => quick_tool(&s("tool")),
        "close-window" => win::close_front_window(),
        "mouse" => win::mouse(
            &s("mode"),
            &s("button"),
            b("double"),
            n("amount", 3) as i32,
            (n("x", 0) as i32, n("y", 0) as i32),
            settings
                .get("relative")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        ),
        "browser" => browser(&s("what")),
        "password" => password(&s("name"), b("enter")),
        "sound" => {
            if s("mode") == "stop" {
                win::stop_sounds()
            } else {
                win::play_sound(&s("path"), n("volume", 100).clamp(0, 100) as u32)
            }
        }
        "udp" => send_udp(&s("host"), n("port", 0), &s("message"), b("hex")),
        _ => return None,
    })
}

fn keys(combo: &str) -> Result<(), String> {
    system::press(&parse_hotkey(combo)?)
}

fn quick_control(what: &str) -> Result<(), String> {
    match what {
        "sleep" => win::sleep(),
        "lock" => win::lock(),
        "screen-off" => win::screen_off(),
        "mic-mute" => win::toggle_mic_mute(),
        "switch-screen" => keys("Win+P"),
        _ => keys("Win+D"),
    }
}

fn quick_tool(tool: &str) -> Result<(), String> {
    match tool {
        "emoji" => keys("Win+."),
        "search" => keys("Win+S"),
        "notifications" => keys("Win+N"),
        "file-explorer" => keys("Win+E"),
        "snip" => keys("Win+Shift+S"),
        "clipboard" => keys("Win+V"),
        "run" => keys("Win+R"),
        "task-manager" => system::open("taskmgr.exe", ""),
        "control-panel" => system::open("control.exe", ""),
        "settings" => system::open("ms-settings:", ""),
        "mail" => system::open("mailto:", ""),
        _ => system::open("calc.exe", ""),
    }
}

fn browser(what: &str) -> Result<(), String> {
    keys(match what {
        "forward" => "VK167",
        "refresh" => "VK168",
        "stop" => "VK169",
        "home" => "VK172",
        "new-tab" => "Ctrl+T",
        "close-tab" => "Ctrl+W",
        "reopen-tab" => "Ctrl+Shift+T",
        "next-tab" => "Ctrl+Tab",
        "previous-tab" => "Ctrl+Shift+Tab",
        _ => "VK166",
    })
}

/// Where a password lives in Windows Credential Manager.
pub fn credential_name(name: &str) -> String {
    format!("Agora/{}", name.trim())
}

fn password(name: &str, enter: bool) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("No password chosen".into());
    }
    let secret = win::read_secret(&credential_name(name))?;
    system::type_text(&secret, enter)
}

/// The bytes of a UDP message: the text as is, or hex pairs ("01 ff 7a").
pub fn udp_bytes(message: &str, hex: bool) -> Result<Vec<u8>, String> {
    if !hex {
        return Ok(message.as_bytes().to_vec());
    }
    let digits: String = message.chars().filter(|c| !c.is_whitespace()).collect();
    if !digits.len().is_multiple_of(2) {
        return Err("Hex needs pairs of digits, like 01 ff".into());
    }
    (0..digits.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&digits[i..i + 2], 16)
                .map_err(|_| format!("\"{}\" isn't hex", &digits[i..i + 2]))
        })
        .collect()
}

fn send_udp(host: &str, port: i64, message: &str, hex: bool) -> Result<(), String> {
    let host = host.trim();
    if host.is_empty() || !(1..=65535).contains(&port) {
        return Err("Set the address and port to send to".into());
    }
    let bytes = udp_bytes(message, hex)?;
    let addr = (host, port as u16)
        .to_socket_addrs()
        .map_err(|e| format!("Can't find {host}: {e}"))?
        .next()
        .ok_or_else(|| format!("Can't find {host}"))?;
    let bind = if addr.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let sock = UdpSocket::bind(bind).map_err(|e| e.to_string())?;
    let _ = sock.set_broadcast(true);
    sock.send_to(&bytes, addr)
        .map(|_| ())
        .map_err(|e| format!("Couldn't send to {addr}: {e}"))
}

#[cfg(windows)]
pub mod win {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows_sys::Win32::Media::Multimedia::mciSendStringW;
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
        CRED_TYPE_GENERIC,
    };
    use windows_sys::Win32::System::Power::SetSuspendState;
    use windows_sys::Win32::System::Shutdown::LockWorkStation;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, PostMessageW, SendMessageTimeoutW, HWND_BROADCAST,
        SC_MONITORPOWER, SMTO_ABORTIFHUNG, WM_CLOSE, WM_SYSCOMMAND,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn sleep() -> Result<(), String> {
        // Not hibernate; wake events allowed.
        if !unsafe { SetSuspendState(false, false, false) } {
            return Err("Windows wouldn't go to sleep".into());
        }
        Ok(())
    }

    pub fn lock() -> Result<(), String> {
        if unsafe { LockWorkStation() } == 0 {
            return Err("Windows wouldn't lock".into());
        }
        Ok(())
    }

    pub fn screen_off() -> Result<(), String> {
        let mut out = 0usize;
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SYSCOMMAND,
                SC_MONITORPOWER as WPARAM,
                2 as LPARAM,
                SMTO_ABORTIFHUNG,
                1000,
                &mut out,
            );
        }
        Ok(())
    }

    pub fn close_front_window() -> Result<(), String> {
        unsafe {
            let w: HWND = GetForegroundWindow();
            if w.is_null() {
                return Err("No window in front".into());
            }
            let mut buf = [0u16; 64];
            let n = GetClassNameW(w, buf.as_mut_ptr(), buf.len() as i32);
            let class = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
            // The taskbar and desktop aren't windows to close.
            if matches!(class.as_str(), "Shell_TrayWnd" | "Progman" | "WorkerW") {
                return Err("The desktop is in front".into());
            }
            PostMessageW(w, WM_CLOSE, 0, 0);
        }
        Ok(())
    }

    pub fn toggle_mic_mute() -> Result<(), String> {
        use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
        use windows::Win32::Media::Audio::{
            eCapture, eConsole, IMMDeviceEnumerator, MMDeviceEnumerator,
        };
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
        };
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let e: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| e.to_string())?;
            let mic = e
                .GetDefaultAudioEndpoint(eCapture, eConsole)
                .map_err(|_| "No microphone found".to_string())?;
            let vol: IAudioEndpointVolume =
                mic.Activate(CLSCTX_ALL, None).map_err(|e| e.to_string())?;
            let muted = vol.GetMute().map_err(|e| e.to_string())?.as_bool();
            vol.SetMute(!muted, std::ptr::null())
                .map_err(|e| e.to_string())
        }
    }

    pub fn mouse(
        mode: &str,
        button: &str,
        double: bool,
        amount: i32,
        to: (i32, i32),
        relative: bool,
    ) -> Result<(), String> {
        use enigo::{Axis, Button, Coordinate, Direction, Enigo, Mouse, Settings};
        let mut e = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
        let err = |e: enigo::InputError| e.to_string();
        match mode {
            "scroll-up" => e.scroll(-amount.max(1), Axis::Vertical).map_err(err),
            "scroll-down" => e.scroll(amount.max(1), Axis::Vertical).map_err(err),
            "scroll-left" => e.scroll(-amount.max(1), Axis::Horizontal).map_err(err),
            "scroll-right" => e.scroll(amount.max(1), Axis::Horizontal).map_err(err),
            "move" => {
                let c = if relative {
                    Coordinate::Rel
                } else {
                    Coordinate::Abs
                };
                e.move_mouse(to.0, to.1, c).map_err(err)
            }
            _ => {
                let b = match button {
                    "right" => Button::Right,
                    "middle" => Button::Middle,
                    _ => Button::Left,
                };
                e.button(b, Direction::Click).map_err(err)?;
                if double {
                    e.button(b, Direction::Click).map_err(err)?;
                }
                Ok(())
            }
        }
    }

    fn mci(cmd: &str) -> Result<(), String> {
        let w = wide(cmd);
        let r =
            unsafe { mciSendStringW(w.as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut()) };
        if r != 0 {
            return Err(format!("Windows couldn't play that sound (error {r})"));
        }
        Ok(())
    }

    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    pub fn play_sound(path: &str, volume: u32) -> Result<(), String> {
        let path = path.trim().trim_matches('"');
        if path.is_empty() {
            return Err("No sound file chosen".into());
        }
        if !std::path::Path::new(path).is_file() {
            return Err(format!("Can't find {path}"));
        }
        // Each press gets its own alias, so sounds can overlap.
        let alias = format!(
            "agora{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 16
        );
        let _ = mci(&format!("close {alias}"));
        mci(&format!("open \"{path}\" type mpegvideo alias {alias}"))?;
        let _ = mci(&format!("setaudio {alias} volume to {}", volume * 10));
        mci(&format!("play {alias}"))
    }

    pub fn stop_sounds() -> Result<(), String> {
        for i in 0..16 {
            let _ = mci(&format!("close agora{i}"));
        }
        Ok(())
    }

    pub fn read_secret(target: &str) -> Result<String, String> {
        let t = wide(target);
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        unsafe {
            if CredReadW(t.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) == 0 {
                return Err("That password isn't saved yet".into());
            }
            let c = &*cred;
            let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .collect();
            let secret = String::from_utf16_lossy(&units);
            CredFree(cred as *const _);
            Ok(secret)
        }
    }

    pub fn has_secret(target: &str) -> bool {
        let t = wide(target);
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        unsafe {
            if CredReadW(t.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) == 0 {
                return false;
            }
            CredFree(cred as *const _);
        }
        true
    }

    pub fn save_secret(target: &str, secret: &str) -> Result<(), String> {
        let mut t = wide(target);
        let mut user = wide("Agora");
        let mut blob: Vec<u8> = secret
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let cred = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: t.as_mut_ptr(),
            Comment: std::ptr::null_mut(),
            LastWritten: unsafe { std::mem::zeroed() },
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: std::ptr::null_mut(),
            TargetAlias: std::ptr::null_mut(),
            UserName: user.as_mut_ptr(),
        };
        let ok = unsafe { CredWriteW(&cred, 0) } != 0;
        blob.iter_mut().for_each(|b| *b = 0);
        if !ok {
            return Err("Windows didn't save the password".into());
        }
        Ok(())
    }

    pub fn delete_secret(target: &str) -> Result<(), String> {
        let t = wide(target);
        unsafe { CredDeleteW(t.as_ptr(), CRED_TYPE_GENERIC, 0) };
        Ok(())
    }
}

#[cfg(not(windows))]
pub mod win {
    const NO: &str = "This only works on Windows so far";
    pub fn sleep() -> Result<(), String> {
        Err(NO.into())
    }
    pub fn lock() -> Result<(), String> {
        Err(NO.into())
    }
    pub fn screen_off() -> Result<(), String> {
        Err(NO.into())
    }
    pub fn close_front_window() -> Result<(), String> {
        Err(NO.into())
    }
    pub fn toggle_mic_mute() -> Result<(), String> {
        Err(NO.into())
    }
    pub fn mouse(_: &str, _: &str, _: bool, _: i32, _: (i32, i32), _: bool) -> Result<(), String> {
        Err(NO.into())
    }
    pub fn play_sound(_: &str, _: u32) -> Result<(), String> {
        Err(NO.into())
    }
    pub fn stop_sounds() -> Result<(), String> {
        Ok(())
    }
    pub fn read_secret(_: &str) -> Result<String, String> {
        Err(NO.into())
    }
    pub fn has_secret(_: &str) -> bool {
        false
    }
    pub fn save_secret(_: &str, _: &str) -> Result<(), String> {
        Err(NO.into())
    }
    pub fn delete_secret(_: &str) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn udp_messages() {
        assert_eq!(udp_bytes("hi", false).unwrap(), b"hi");
        assert_eq!(udp_bytes("01 ff 7A", true).unwrap(), vec![1, 255, 122]);
        assert!(udp_bytes("0", true).is_err());
        assert!(udp_bytes("zz", true).is_err());
    }

    #[test]
    fn udp_reaches_a_local_listener() {
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        rx.set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let port = rx.local_addr().unwrap().port() as i64;
        send_udp("127.0.0.1", port, "ping", false).unwrap();
        let mut buf = [0u8; 16];
        let (n, _) = rx.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"ping");
        assert!(send_udp("", 9, "x", false).is_err());
    }

    #[test]
    fn passwords_are_named_for_agora() {
        assert_eq!(credential_name(" Work email "), "Agora/Work email");
    }
}
