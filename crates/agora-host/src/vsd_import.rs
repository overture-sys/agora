//! Reads VSD Craft profiles (`%APPDATA%\HotSpot\StreamDock\profiles`) so
//! they can be brought into Agora. Read only: VSD Craft's files are never
//! changed.
//!
//! A profile is a `<id>.sdProfile` folder with a `manifest.json`. Its
//! `Actions` are the first page, keyed "column,row"; `Pages.Pages` names more
//! pages and folders live beside them in `profiles/<id>.sdProfile`. Column 5
//! on the M18 is the three round buttons.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde::Serialize;
use serde_json::{json, Value};

use crate::builtin;

/// A VSD Craft profile that can be imported.
#[derive(Debug, Clone, Serialize)]
pub struct VsdProfile {
    /// Its folder.
    pub dir: String,
    pub name: String,
    pub pages: usize,
    pub keys: usize,
}

/// One key read from VSD Craft, in Agora's terms.
#[derive(Debug, Clone)]
pub(crate) struct ReadKey {
    pub plugin: String,
    pub action: String,
    pub settings: Value,
    pub title: Option<String>,
    pub show_title: Option<bool>,
    pub align: Option<String>,
    pub color: Option<String>,
    pub font_size: Option<f32>,
    pub icon: Option<String>,
    /// For folders: the folder's page.
    pub folder: Option<ReadPage>,
    /// For Scene Shift: the VSD Craft profile folder name it switches to.
    pub switch_to: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ReadPage {
    pub keys: Vec<(usize, ReadKey)>,
}

#[derive(Debug, Clone)]
pub(crate) struct ReadProfile {
    /// The profile folder's name, e.g. "9066A6QM-....sdProfile".
    pub id: String,
    pub name: String,
    pub apps: Vec<String>,
    pub pages: Vec<ReadPage>,
}

fn load(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

fn is_device(m: &Value, device: &str) -> bool {
    m.get("DeviceUUID")
        .and_then(Value::as_str)
        .is_some_and(|d| d.starts_with(device))
}

/// The VSD Craft profiles made for `device` (e.g. "VSDM18"), by name.
pub fn list(root: &Path, device: &str) -> Vec<VsdProfile> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<VsdProfile> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".sdProfile"))
        .filter_map(|e| {
            let m = load(&e.path().join("manifest.json"))?;
            if !is_device(&m, device) {
                return None;
            }
            let extra = page_ids(&m, &e.file_name().to_string_lossy()).len();
            Some(VsdProfile {
                dir: e.path().display().to_string(),
                name: m
                    .get("Name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                pages: 1 + extra,
                keys: m
                    .get("Actions")
                    .and_then(Value::as_object)
                    .map_or(0, |a| a.len()),
            })
        })
        .collect();
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

/// The pages after the first, as folder names under `profiles/`.
fn page_ids(m: &Value, own: &str) -> Vec<String> {
    m.pointer("/Pages/Pages")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .filter(|p| *p != own)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Reads a profile. `known(plugin, action)` says whether Agora has a plugin
/// action; anything Agora can't run is listed in the second value.
pub(crate) fn read(
    dir: &Path,
    columns: usize,
    rows: usize,
    known: &dyn Fn(&str) -> Option<String>,
) -> Result<(ReadProfile, Vec<String>), String> {
    let m = load(&dir.join("manifest.json")).ok_or("That isn't a VSD Craft profile")?;
    let own = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut r = Reader {
        root: dir.to_path_buf(),
        columns,
        rows,
        known,
        skipped: Vec::new(),
        depth: 0,
    };
    let mut pages = vec![r.page(&m, dir)];
    for id in page_ids(&m, &own) {
        let sub = dir.join("profiles").join(&id);
        if let Some(pm) = load(&sub.join("manifest.json")) {
            pages.push(r.page(&pm, &sub));
        }
    }
    let apps = [
        "AppIdentifier",
        "AppIdentifier0",
        "AppIdentifier1",
        "AppIdentifier2",
    ]
    .iter()
    .filter_map(|k| m.get(*k).and_then(Value::as_str))
    .filter(|a| !a.is_empty() && *a != "*")
    .filter_map(|a| a.rsplit(['\\', '/']).next())
    .map(str::to_lowercase)
    .collect();
    let mut skipped = r.skipped;
    skipped.sort();
    skipped.dedup();
    Ok((
        ReadProfile {
            id: own,
            name: m
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or("Imported")
                .to_string(),
            apps,
            pages,
        },
        skipped,
    ))
}

struct Reader<'a> {
    /// The profile's own folder; folders are in `root/profiles`.
    root: PathBuf,
    columns: usize,
    rows: usize,
    known: &'a dyn Fn(&str) -> Option<String>,
    skipped: Vec<String>,
    depth: usize,
}

impl Reader<'_> {
    fn page(&mut self, m: &Value, dir: &Path) -> ReadPage {
        let mut page = ReadPage::default();
        let Some(actions) = m.get("Actions").and_then(Value::as_object) else {
            return page;
        };
        for (pos, a) in actions {
            let Some((c, r)) = pos.split_once(',') else {
                continue;
            };
            let (Ok(c), Ok(r)) = (c.trim().parse::<usize>(), r.trim().parse::<usize>()) else {
                continue;
            };
            let name = a.get("Name").and_then(Value::as_str).unwrap_or("?");
            if c >= self.columns || r >= self.rows {
                // The M18's round buttons; Agora doesn't run actions on them yet.
                self.skipped.push(format!("{name} (round button)"));
                continue;
            }
            if let Some(k) = self.key(a, dir) {
                page.keys.push((r * self.columns + c, k));
            }
        }
        page
    }

    fn key(&mut self, a: &Value, dir: &Path) -> Option<ReadKey> {
        let uuid = a.get("UUID").and_then(Value::as_str).unwrap_or("");
        let name = a.get("Name").and_then(Value::as_str).unwrap_or(uuid);
        let s = a.get("Settings").cloned().unwrap_or_else(|| json!({}));
        let st = |k: &str| s.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let mut folder = None;
        let mut switch_to = None;
        let (plugin, action, settings) = match uuid {
            "com.hotspot.streamdock.page.previous" => {
                builtin_key(builtin::PREVIOUS_PAGE, json!({}))
            }
            "com.hotspot.streamdock.page.next" => builtin_key(builtin::NEXT_PAGE, json!({})),
            "com.hotspot.streamdock.profile.backtoparent" => builtin_key(builtin::BACK, json!({})),
            "com.hotspot.streamdock.profile.rotate" => {
                switch_to = Some(st("ProfileUUID"));
                builtin_key(builtin::SWITCH_PROFILE, json!({ "profile": "" }))
            }
            "com.hotspot.streamdock.profile.openchild" => {
                let sub = self.root.join("profiles").join(st("ProfileUUID"));
                let pm = load(&sub.join("manifest.json"));
                if self.depth < 8 {
                    if let Some(pm) = pm {
                        self.depth += 1;
                        folder = Some(self.page(&pm, &sub));
                        self.depth -= 1;
                    }
                }
                let title = state0(a, "Title").unwrap_or_else(|| "Folder".into());
                builtin_key(builtin::FOLDER, json!({ "name": title }))
            }
            "com.hotspot.streamdock.system.website" => {
                builtin_key("website", json!({ "url": st("path") }))
            }
            "com.hotspot.streamdock.system.open" => {
                builtin_key("open", json!({ "path": st("path"), "args": "" }))
            }
            "com.hotspot.streamdock.system.text" => builtin_key(
                "text",
                json!({ "text": st("pastedText"), "enter": s.get("isSendingEnter").and_then(Value::as_bool).unwrap_or(false) }),
            ),
            "com.hotspot.streamdock.system.multimedia" => {
                let key = match s.get("actionIdx").and_then(Value::as_i64) {
                    Some(0) => "previous",
                    Some(1) => "play-pause",
                    Some(2) => "next",
                    Some(4) => "mute",
                    Some(5) => "volume-up",
                    Some(6) => "volume-down",
                    _ => {
                        self.skipped.push(format!("{name} (this media key)"));
                        return None;
                    }
                };
                builtin_key("media", json!({ "key": key }))
            }
            "com.hotspot.streamdock.system.hotkey"
            | "com.hotspot.streamdock.system.super.hotkey" => {
                let combos = hotkeys(&s);
                match combos.len() {
                    0 => {
                        self.skipped.push(format!("{name} (no keys set)"));
                        return None;
                    }
                    1 => builtin_key("hotkey", json!({ "keys": combos[0] })),
                    _ => builtin_key(
                        builtin::MULTI,
                        json!({ "steps": combos.iter().map(|k| json!({ "action": "hotkey", "settings": { "keys": k } })).collect::<Vec<_>>() }),
                    ),
                }
            }
            "com.hotspot.streamdock.multiactions.routine" => {
                let steps = self.steps(a);
                builtin_key(builtin::MULTI, json!({ "steps": steps }))
            }
            other => match (self.known)(other) {
                Some(plugin) => (plugin, other.to_string(), s.clone()),
                None => {
                    self.skipped.push(name.to_string());
                    return None;
                }
            },
        };
        let state = a
            .get("States")
            .and_then(Value::as_array)
            .and_then(|v| v.first())
            .cloned()
            .unwrap_or_default();
        let get = |k: &str| {
            state
                .get(k)
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|v| !v.is_empty())
        };
        let font_size = match state.get("FontSize") {
            Some(Value::String(v)) => v.parse().ok(),
            Some(Value::Number(n)) => n.as_f64().map(|f| f as f32),
            _ => None,
        };
        Some(ReadKey {
            plugin,
            action,
            settings,
            title: get("Title"),
            show_title: state
                .get("ShowTitle")
                .and_then(Value::as_bool)
                .filter(|v| !v),
            align: get("TitleAlignment")
                .filter(|a| matches!(a.as_str(), "top" | "middle" | "bottom")),
            color: get("TitleColor").filter(|c| c.starts_with('#')),
            font_size,
            icon: get("Image").and_then(|i| image(dir, &i)),
            folder,
            switch_to,
        })
    }

    /// A VSD Craft multi-action's steps, as Agora steps.
    fn steps(&mut self, a: &Value) -> Vec<Value> {
        let mut out = Vec::new();
        for inner in a
            .get("Actions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let uuid = inner.get("UUID").and_then(Value::as_str).unwrap_or("");
            if uuid == "com.hotspot.streamdock.multiactions.delay" {
                let ms = inner
                    .pointer("/Settings/delay")
                    .and_then(Value::as_u64)
                    .unwrap_or(500);
                out.push(json!({ "action": builtin::DELAY, "settings": { "ms": ms } }));
                continue;
            }
            let dir = self.root.clone();
            match self.key(inner, &dir) {
                Some(k) if k.plugin == builtin::PLUGIN && k.action == builtin::MULTI => {
                    out.extend(k.settings["steps"].as_array().cloned().unwrap_or_default());
                }
                Some(k) if k.plugin == builtin::PLUGIN && builtin::step_allowed(&k.action) => {
                    out.push(json!({ "action": k.action, "settings": k.settings }));
                }
                Some(k) => self
                    .skipped
                    .push(format!("{} (inside a multi-action)", k.action)),
                None => {}
            }
        }
        out
    }
}

fn builtin_key(action: &str, settings: Value) -> (String, String, Value) {
    (builtin::PLUGIN.into(), action.into(), settings)
}

fn state0(a: &Value, k: &str) -> Option<String> {
    a.pointer(&format!("/States/0/{k}"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|v| !v.is_empty())
}

/// A key's own picture from the page's Images folder, as a data URI.
fn image(dir: &Path, name: &str) -> Option<String> {
    if name.contains('/') || name.contains('\\') {
        return None; // one of VSD Craft's own pictures
    }
    let ext = name.rsplit('.').next()?.to_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    };
    let path = dir.join("Images").join(name);
    let bytes = std::fs::read(&path).ok().filter(|b| b.len() < 2_000_000)?;
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// VSD Craft's hotkey list as Agora hotkey text, one per combination set.
fn hotkeys(s: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for h in s
        .get("Hotkeys")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let vk = h.get("VKeyCode").and_then(Value::as_i64).unwrap_or(-1);
        if vk <= 0 {
            continue;
        }
        let on = |k: &str| h.get(k).and_then(Value::as_bool).unwrap_or(false);
        let mut parts = Vec::new();
        if on("KeyCtrl") || on("RKeyCtrl") {
            parts.push("Ctrl".to_string());
        }
        if on("KeyShift") || on("RKeyShift") {
            parts.push("Shift".into());
        }
        if on("KeyOption") || on("RKeyOption") {
            parts.push("Alt".into());
        }
        if on("KeyCmd") || on("RKeyCmd") {
            parts.push("Win".into());
        }
        parts.push(vk_name(vk as u16));
        out.push(parts.join("+"));
    }
    out
}

/// A Windows virtual-key code as the name Agora's hotkey box uses.
pub fn vk_name(vk: u16) -> String {
    let named: HashMap<u16, &str> = [
        (0x0D, "Enter"),
        (0x09, "Tab"),
        (0x1B, "Esc"),
        (0x20, "Space"),
        (0x08, "Backspace"),
        (0x2E, "Del"),
        (0x2D, "Ins"),
        (0x24, "Home"),
        (0x23, "End"),
        (0x21, "PgUp"),
        (0x22, "PgDn"),
        (0x25, "Left"),
        (0x26, "Up"),
        (0x27, "Right"),
        (0x28, "Down"),
        (0x2C, "PrtSc"),
        (0x13, "Pause"),
        (0xBD, "-"),
        (0xBB, "="),
        (0xBC, ","),
        (0xBE, "."),
        (0xBF, "/"),
        (0xBA, ";"),
        (0xDE, "'"),
        (0xDB, "["),
        (0xDD, "]"),
        (0xDC, "\\"),
        (0xC0, "`"),
    ]
    .into_iter()
    .collect();
    if let Some(n) = named.get(&vk) {
        return n.to_string();
    }
    match vk {
        0x30..=0x39 | 0x41..=0x5A => (vk as u8 as char).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => format!("VK{vk}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys_read_like_the_editor_writes_them() {
        let s = json!({ "Hotkeys": [
            { "KeyCtrl": true, "KeyShift": true, "VKeyCode": 77 },
            { "VKeyCode": -1 },
            { "KeyOption": true, "VKeyCode": 0x73 },
        ] });
        assert_eq!(hotkeys(&s), vec!["Ctrl+Shift+M", "Alt+F4"]);
        assert_eq!(vk_name(0xBD), "-");
        assert_eq!(vk_name(0xAD), "VK173");
    }

    #[test]
    fn reads_pages_folders_and_round_buttons() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("AAA.sdProfile");
        std::fs::create_dir_all(p.join("profiles/FOLD.sdProfile")).unwrap();
        std::fs::create_dir_all(p.join("profiles/PAGE2.sdProfile")).unwrap();
        std::fs::write(
            p.join("manifest.json"),
            "\u{feff}".to_string()
                + &json!({
                    "Name": "Mine", "DeviceUUID": "VSDM18", "AppIdentifier": "C:\\Apps\\OBS64.exe",
                    "Pages": { "Current": "PAGE2.sdProfile", "Pages": ["PAGE2.sdProfile"] },
                    "Actions": {
                        "0,0": { "UUID": "com.hotspot.streamdock.system.website", "Name": "Website", "Settings": { "path": "https://x.org" }, "States": [{ "Title": "X" }] },
                        "1,0": { "UUID": "com.hotspot.streamdock.profile.openchild", "Settings": { "ProfileUUID": "FOLD.sdProfile" }, "States": [{ "Title": "Tools" }] },
                        "5,1": { "UUID": "com.hotspot.streamdock.page.next", "Name": "Next page" },
                        "2,2": { "UUID": "com.example.plugin.act", "Name": "Thing", "Settings": { "a": 1 } },
                        "3,2": { "UUID": "com.missing.act", "Name": "Missing" }
                    }
                })
                .to_string(),
        )
        .unwrap();
        std::fs::write(
            p.join("profiles/FOLD.sdProfile/manifest.json"),
            json!({ "DeviceUUID": "VSDM18", "Actions": { "0,0": { "UUID": "com.hotspot.streamdock.profile.backtoparent" }, "1,0": { "UUID": "com.hotspot.streamdock.system.multimedia", "Settings": { "actionIdx": 5 } } } }).to_string(),
        )
        .unwrap();
        std::fs::write(
            p.join("profiles/PAGE2.sdProfile/manifest.json"),
            json!({ "DeviceUUID": "VSDM18", "Actions": { "4,2": { "UUID": "com.hotspot.streamdock.page.previous" } } }).to_string(),
        )
        .unwrap();

        let listed = list(dir.path(), "VSDM18");
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].name.as_str(), listed[0].pages), ("Mine", 2));

        let known =
            |a: &str| (a == "com.example.plugin.act").then(|| "com.example.plugin".to_string());
        let (prof, skipped) = read(&p, 5, 3, &known).unwrap();
        assert_eq!(prof.apps, vec!["obs64.exe"]);
        assert_eq!(prof.pages.len(), 2);
        let keys: HashMap<usize, &ReadKey> =
            prof.pages[0].keys.iter().map(|(k, v)| (*k, v)).collect();
        assert_eq!(keys[&0].action, "website");
        assert_eq!(keys[&0].title.as_deref(), Some("X"));
        assert_eq!(keys[&1].action, builtin::FOLDER);
        let folder = keys[&1].folder.as_ref().unwrap();
        assert_eq!(folder.keys.len(), 2);
        assert_eq!(keys[&12].plugin, "com.example.plugin");
        assert_eq!(prof.pages[1].keys[0].0, 14);
        assert!(skipped.contains(&"Missing".to_string()));
        assert!(skipped.iter().any(|s| s.contains("round button")));
    }
}
