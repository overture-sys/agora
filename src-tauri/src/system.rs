//! Agora's system actions: open an app, file or web page, press a hotkey,
//! type text and media keys. Key presses go through Windows' SendInput (via
//! enigo); on other systems they are logged and skipped.

use serde_json::Value;

/// One key in a hotkey, as typed in the editor ("Ctrl+Shift+M").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum K {
    Ctrl,
    Shift,
    Alt,
    Win,
    /// A Windows virtual-key code.
    Vk(u16),
}

/// Reads "Ctrl+Shift+M", "alt + f4", "Win+D" or "Ctrl+Alt+Del". Modifiers
/// can come in any order; the last part is the key.
pub fn parse_hotkey(text: &str) -> Result<Vec<K>, String> {
    let parts: Vec<String> = text
        .split('+')
        .map(|p| p.trim().to_ascii_lowercase())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        return Err("No keys set".into());
    }
    let mut out = Vec::new();
    for p in &parts {
        let k = match p.as_str() {
            "ctrl" | "control" => K::Ctrl,
            "shift" => K::Shift,
            "alt" => K::Alt,
            "win" | "windows" | "meta" | "super" | "cmd" => K::Win,
            "enter" | "return" => K::Vk(0x0D),
            "tab" => K::Vk(0x09),
            "esc" | "escape" => K::Vk(0x1B),
            "space" => K::Vk(0x20),
            "backspace" => K::Vk(0x08),
            "del" | "delete" => K::Vk(0x2E),
            "ins" | "insert" => K::Vk(0x2D),
            "home" => K::Vk(0x24),
            "end" => K::Vk(0x23),
            "pgup" | "pageup" => K::Vk(0x21),
            "pgdn" | "pagedown" => K::Vk(0x22),
            "left" => K::Vk(0x25),
            "up" => K::Vk(0x26),
            "right" => K::Vk(0x27),
            "down" => K::Vk(0x28),
            "prtsc" | "printscreen" => K::Vk(0x2C),
            "pause" => K::Vk(0x13),
            "-" | "minus" => K::Vk(0xBD),
            "=" | "plus" | "equals" => K::Vk(0xBB),
            "," | "comma" => K::Vk(0xBC),
            "." | "period" => K::Vk(0xBE),
            "/" | "slash" => K::Vk(0xBF),
            ";" => K::Vk(0xBA),
            "'" => K::Vk(0xDE),
            "[" => K::Vk(0xDB),
            "]" => K::Vk(0xDD),
            "\\" => K::Vk(0xDC),
            "`" => K::Vk(0xC0),
            f if f.len() > 1
                && f.starts_with('f')
                && f[1..].parse::<u16>().is_ok_and(|n| (1..=24).contains(&n)) =>
            {
                K::Vk(0x70 + f[1..].parse::<u16>().unwrap() - 1)
            }
            c if c.len() == 1 && c.as_bytes()[0].is_ascii_alphanumeric() => {
                K::Vk(c.to_ascii_uppercase().as_bytes()[0] as u16)
            }
            v if v.len() > 2 && v.starts_with("vk") && v[2..].parse::<u16>().is_ok() => {
                K::Vk(v[2..].parse().unwrap())
            }
            other => return Err(format!("Agora doesn't know the key \"{other}\"")),
        };
        out.push(k);
    }
    Ok(out)
}

/// Splits program arguments on spaces, keeping "quoted parts" together.
pub fn split_args(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A web address from what the user typed; adds https:// if left out.
pub fn web_address(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || t == "https://" || t == "http://" {
        return None;
    }
    if t.starts_with("https://") || t.starts_with("http://") {
        Some(t.into())
    } else if t.contains("://") {
        None
    } else {
        Some(format!("https://{t}"))
    }
}

/// Opens an app, file or folder. Programs get their arguments; anything
/// else opens with whatever Windows uses for it.
pub fn open(path: &str, args: &str) -> Result<(), String> {
    let path = path.trim().trim_matches('"');
    if path.is_empty() {
        return Err("No app or file chosen".into());
    }
    let p = std::path::Path::new(path);
    // "shell:AppsFolder\..." names a Start menu app; only Explorer opens those.
    let program = !path.to_ascii_lowercase().starts_with("shell:")
        && p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    let result = if program {
        let mut cmd = std::process::Command::new(p);
        cmd.args(split_args(args));
        if let Some(dir) = p.parent().filter(|d| d.is_dir()) {
            cmd.current_dir(dir);
        }
        cmd.spawn()
    } else {
        #[cfg(windows)]
        let cmd = std::process::Command::new("explorer.exe").arg(p).spawn();
        #[cfg(not(windows))]
        let cmd = std::process::Command::new("xdg-open").arg(p).spawn();
        cmd
    };
    result
        .map(|_| ())
        .map_err(|e| format!("Couldn't open {path}: {e}"))
}

#[cfg(windows)]
mod input {
    use super::K;
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};

    fn key(k: K) -> Key {
        match k {
            K::Ctrl => Key::Control,
            K::Shift => Key::Shift,
            K::Alt => Key::Alt,
            K::Win => Key::Meta,
            K::Vk(vk) => Key::Other(vk as u32),
        }
    }

    fn enigo() -> Result<Enigo, String> {
        Enigo::new(&Settings::default()).map_err(|e| e.to_string())
    }

    pub fn hotkey(keys: &[K]) -> Result<(), String> {
        let mut e = enigo()?;
        for k in keys {
            e.key(key(*k), Direction::Press)
                .map_err(|e| e.to_string())?;
        }
        for k in keys.iter().rev() {
            e.key(key(*k), Direction::Release)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn text(text: &str, enter: bool) -> Result<(), String> {
        let mut e = enigo()?;
        e.text(text).map_err(|e| e.to_string())?;
        if enter {
            e.key(Key::Return, Direction::Click)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn media(name: &str) -> Result<(), String> {
        let k = match name {
            "next" => Key::MediaNextTrack,
            "previous" => Key::MediaPrevTrack,
            "volume-up" => Key::VolumeUp,
            "volume-down" => Key::VolumeDown,
            "mute" => Key::VolumeMute,
            _ => Key::MediaPlayPause,
        };
        enigo()?.key(k, Direction::Click).map_err(|e| e.to_string())
    }
}

#[cfg(not(windows))]
mod input {
    use super::K;

    pub fn hotkey(_: &[K]) -> Result<(), String> {
        Err("Hotkeys only work on Windows so far".into())
    }

    pub fn text(_: &str, _: bool) -> Result<(), String> {
        Err("Typing text only works on Windows so far".into())
    }

    pub fn media(_: &str) -> Result<(), String> {
        Err("Media keys only work on Windows so far".into())
    }
}

/// Runs open, website, hotkey, text or media. Brightness and the ring are
/// dock settings, done by the caller.
pub fn run(action: &str, settings: &Value, open_url: impl Fn(&str)) -> Result<(), String> {
    let s = |k: &str| {
        settings
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    match action {
        "open" => open(&s("path"), &s("args")),
        "website" => {
            let url = web_address(&s("url")).ok_or("No web address set")?;
            open_url(&url);
            Ok(())
        }
        "hotkey" => input::hotkey(&parse_hotkey(&s("keys"))?),
        "text" => {
            let enter = settings
                .get("enter")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            input::text(&s("text"), enter)
        }
        "media" => input::media(&s("key")),
        other => Err(format!("Unknown action {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys_parse() {
        assert_eq!(
            parse_hotkey("Ctrl+Shift+M").unwrap(),
            vec![K::Ctrl, K::Shift, K::Vk(b'M' as u16)]
        );
        assert_eq!(
            parse_hotkey(" alt + f4 ").unwrap(),
            vec![K::Alt, K::Vk(0x73)]
        );
        assert_eq!(
            parse_hotkey("Win+d").unwrap(),
            vec![K::Win, K::Vk(b'D' as u16)]
        );
        assert_eq!(
            parse_hotkey("ctrl+alt+del").unwrap(),
            vec![K::Ctrl, K::Alt, K::Vk(0x2E)]
        );
        assert!(parse_hotkey("ctrl+banana").is_err());
        assert_eq!(
            parse_hotkey("Ctrl+VK173").unwrap(),
            vec![K::Ctrl, K::Vk(173)]
        );
        assert!(parse_hotkey("").is_err());
    }

    #[test]
    fn args_split_on_spaces_outside_quotes() {
        assert_eq!(
            split_args(r#"--profile "My Work" -x"#),
            vec!["--profile", "My Work", "-x"]
        );
        assert_eq!(split_args(r#"a "" b"#), vec!["a", "", "b"]);
        assert!(split_args("  ").is_empty());
    }

    #[test]
    fn web_addresses() {
        assert_eq!(web_address("example.com").unwrap(), "https://example.com");
        assert_eq!(web_address("http://x.org").unwrap(), "http://x.org");
        assert!(web_address("file:///c:/x").is_none());
        assert!(web_address("https://").is_none());
    }
}
