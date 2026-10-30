// Agora app shell: tray icon, dock connection with reconnect, and the plugin
// host. Keys show what their plugins draw; presses go to the plugins.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod foreground;
mod system;

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agora_device::keyimage::{self, Orientation};
use agora_device::worker::{Command, DockWorker, Event};
use agora_device::{discover, Button, Control, Rgb};
use agora_host::{
    face, manifest, ActionInfo, Config, DeviceDesc, HostEvent, HostHandle, KeyInfo, LayoutOp,
    LayoutView, Look, PluginInfo, Source,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// M18 key image size, used for previews before a dock has connected.
const KEY_SIZE: u32 = 64;

/// The connected dock, if any. The connection thread fills and clears it.
/// What Agora remembers between runs, in %APPDATA%\Agora\settings.json.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    brightness: u8,
    ring_color: String,
    ring_on: bool,
    ring_brightness: u8,
    /// Also load the plugins installed for VSD Craft (read only). Off while
    /// Agora is being built and tested.
    use_vsd_craft_plugins: bool,
    /// Start Agora (in the tray) when Windows starts.
    start_with_windows: bool,
    /// Each light's colour (22 ring + 2 front), when painted one by one.
    /// Empty means the whole ring is `ring_color`.
    ring_colors: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            brightness: 80,
            ring_color: "#d9794a".into(),
            ring_on: false,
            ring_brightness: 100,
            use_vsd_craft_plugins: false,
            start_with_windows: false,
            ring_colors: Vec::new(),
        }
    }
}

fn load_settings(path: &std::path::Path) -> Settings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_settings(path: &std::path::Path, settings: &Settings) {
    if let Ok(text) = serde_json::to_string_pretty(settings) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// The 24 colours the ring shows with these settings.
fn ring_lights(s: &Settings) -> Vec<Rgb> {
    if s.ring_colors.len() == 24 {
        s.ring_colors.iter().map(|c| hex_rgb(c)).collect()
    } else {
        vec![hex_rgb(&s.ring_color); 24]
    }
}

fn hex_rgb(color: &str) -> Rgb {
    let hex = color.trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    Rgb(byte(0), byte(2), byte(4))
}

/// Puts the saved brightness and ring on a freshly opened dock.
fn apply_settings(worker: &DockWorker, s: &Settings) {
    worker.send(Command::Brightness(s.brightness));
    if s.ring_on {
        worker.send(Command::Ring(ring_lights(s)));
        worker.send(Command::RingBrightness(s.ring_brightness));
    }
}

struct Dock {
    worker: Option<DockWorker>,
    model: Option<String>,
    orientation: Orientation,
    /// The last connection message, for a window that opens later.
    message: String,
    settings: Settings,
    settings_path: PathBuf,
}

impl Default for Dock {
    fn default() -> Self {
        Self {
            worker: None,
            model: None,
            orientation: Orientation::default(),
            message: String::new(),
            settings: Settings::default(),
            settings_path: PathBuf::from("settings.json"),
        }
    }
}

impl Dock {
    /// Changes a setting, saves it, and sends `cmd` if a dock is connected.
    fn change(
        &mut self,
        edit: impl FnOnce(&mut Settings),
        cmds: Vec<Command>,
    ) -> Result<(), String> {
        edit(&mut self.settings);
        save_settings(&self.settings_path, &self.settings);
        match &self.worker {
            Some(w) => {
                for c in cmds {
                    w.send(c);
                }
                Ok(())
            }
            None => Err("Saved. It goes on the dock when one is connected.".into()),
        }
    }
}

type Shared = Arc<Mutex<Dock>>;

/// The plugin host. Taken out and dropped on quit so plugins are stopped.
type Host = Arc<Mutex<Option<HostHandle>>>;

/// Programs recently in front, newest first, for picking a profile's apps.
type RecentApps = Arc<Mutex<Vec<String>>>;

/// Latest preview of each key for the window, as PNG data URIs.
type Previews = Arc<Mutex<HashMap<usize, String>>>;

#[derive(Clone, Serialize)]
struct Status {
    connected: bool,
    model: Option<String>,
    rows: usize,
    columns: usize,
    message: String,
}

#[derive(Clone, Serialize)]
struct Input {
    /// "key", "button" or "unknown"
    kind: &'static str,
    /// Key index from 0 at the top left, or 0/1/2 for left/middle/right.
    index: usize,
    pressed: bool,
    raw_id: u8,
}

#[derive(Clone, Serialize)]
struct Preview {
    key: usize,
    image: Option<String>,
}

#[derive(Clone, Serialize)]
struct PluginLine {
    plugin: String,
    message: String,
}

fn status(dock: &Dock, message: impl Into<String>) -> Status {
    let layout = dock.worker.as_ref().map(|w| w.layout);
    Status {
        connected: dock.worker.is_some(),
        model: dock.model.clone(),
        rows: layout.map_or(3, |l| l.rows),
        columns: layout.map_or(5, |l| l.columns),
        message: message.into(),
    }
}

fn with_host<T>(host: &State<Host>, f: impl FnOnce(&HostHandle) -> T) -> Result<T, String> {
    let guard = host.lock().unwrap();
    guard
        .as_ref()
        .map(f)
        .ok_or_else(|| "The plugin host isn't running".to_string())
}

#[tauri::command]
fn dock_status(state: State<Shared>) -> Status {
    let dock = state.lock().unwrap();
    status(&dock, dock.message.clone())
}

#[tauri::command]
fn set_brightness(state: State<Shared>, percent: u8) -> Result<(), String> {
    state.lock().unwrap().change(
        |s| s.brightness = percent.min(100),
        vec![Command::Brightness(percent)],
    )
}

#[tauri::command]
fn get_settings(state: State<Shared>) -> Settings {
    state.lock().unwrap().settings.clone()
}

/// `color` is "#rrggbb". Lights the whole ring (and the two front LEDs).
#[tauri::command]
fn set_ring(state: State<Shared>, color: String) -> Result<(), String> {
    let mut dock = state.lock().unwrap();
    let level = dock.settings.ring_brightness;
    let cmds = vec![
        Command::Ring(vec![hex_rgb(&color); 24]),
        Command::RingBrightness(level),
    ];
    dock.change(
        |s| {
            s.ring_color = color.clone();
            s.ring_colors.clear();
            s.ring_on = true;
        },
        cmds,
    )
}

/// Paints the lights one by one: 24 "#rrggbb" colours, ring 0-21 then
/// the two on the front.
#[tauri::command]
fn set_ring_lights(state: State<Shared>, colors: Vec<String>) -> Result<(), String> {
    if colors.len() != 24 {
        return Err("The ring needs 24 colours".into());
    }
    let mut dock = state.lock().unwrap();
    let level = dock.settings.ring_brightness;
    let cmds = vec![
        Command::Ring(colors.iter().map(|c| hex_rgb(c)).collect()),
        Command::RingBrightness(level),
    ];
    dock.change(
        |s| {
            s.ring_colors = colors.clone();
            s.ring_on = true;
        },
        cmds,
    )
}

#[tauri::command]
fn set_ring_brightness(state: State<Shared>, percent: u8) -> Result<(), String> {
    let percent = percent.min(100);
    let mut dock = state.lock().unwrap();
    let cmds = if dock.settings.ring_on {
        vec![Command::RingBrightness(percent)]
    } else {
        vec![]
    };
    dock.change(|s| s.ring_brightness = percent, cmds)
}

#[tauri::command]
fn ring_off(state: State<Shared>) -> Result<(), String> {
    state
        .lock()
        .unwrap()
        .change(|s| s.ring_on = false, vec![Command::RingOff])
}

/// Turns loading VSD Craft's plugins on or off, then restarts the plugin
/// host so the list matches.
#[tauri::command]
fn set_use_vsd_craft_plugins(
    app: AppHandle,
    state: State<Shared>,
    host: State<Host>,
    on: bool,
) -> Result<(), String> {
    {
        let mut dock = state.lock().unwrap();
        dock.settings.use_vsd_craft_plugins = on;
        save_settings(&dock.settings_path, &dock.settings);
    }
    restart_host(&app, &host, &state, &app.state::<Previews>())
}

#[tauri::command]
fn list_actions(host: State<Host>) -> Result<Vec<ActionInfo>, String> {
    with_host(&host, |h| h.actions())
}

#[tauri::command]
fn list_plugins(host: State<Host>) -> Result<Vec<PluginInfo>, String> {
    with_host(&host, |h| h.plugins())
}

#[tauri::command]
fn key_info(host: State<Host>, key: usize) -> Result<Option<KeyInfo>, String> {
    with_host(&host, |h| h.key_info(key))
}

#[tauri::command]
fn place_action(
    host: State<Host>,
    key: usize,
    plugin: String,
    action: String,
) -> Result<(), String> {
    with_host(&host, |h| h.place(key, &plugin, &action))?
}

#[tauri::command]
fn clear_key(host: State<Host>, key: usize) -> Result<(), String> {
    with_host(&host, |h| h.clear(key))
}

#[tauri::command]
fn move_key(host: State<Host>, from: usize, to: usize) -> Result<(), String> {
    with_host(&host, |h| h.move_key(from, to))
}

/// Adds or removes Agora in Windows' list of apps that start at sign-in
/// (HKCU\...\Run). It starts in the tray, without a window.
fn register_autostart(on: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
        let mut cmd = std::process::Command::new("reg");
        if on {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let value = format!("\"{}\" --tray", exe.display());
            cmd.args([
                "add", key, "/v", "Agora", "/t", "REG_SZ", "/d", &value, "/f",
            ]);
        } else {
            cmd.args(["delete", key, "/v", "Agora", "/f"]);
        }
        let out = cmd
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| e.to_string())?;
        // Deleting an entry that isn't there is fine.
        if !out.status.success() && on {
            return Err(format!(
                "Windows didn't accept it: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = on;
        Err("Starting with the system only works on Windows so far".into())
    }
}

#[tauri::command]
fn set_start_with_windows(state: State<Shared>, on: bool) -> Result<(), String> {
    register_autostart(on)?;
    let mut dock = state.lock().unwrap();
    dock.settings.start_with_windows = on;
    save_settings(&dock.settings_path, &dock.settings);
    Ok(())
}

#[tauri::command]
fn get_layout(host: State<Host>) -> Result<Option<LayoutView>, String> {
    with_host(&host, |h| h.layout())
}

/// Profile and page changes from the editor. `id` is the page or profile.
#[tauri::command]
fn layout_op(
    host: State<Host>,
    op: String,
    id: Option<String>,
    name: Option<String>,
    apps: Option<Vec<String>>,
) -> Result<(), String> {
    let id = id.unwrap_or_default();
    let name = name.unwrap_or_default();
    let op = match op.as_str() {
        "go-to-page" => LayoutOp::GoToPage(id),
        "add-page" => LayoutOp::AddPage,
        "remove-page" => LayoutOp::RemovePage(id),
        "rename-page" => LayoutOp::RenamePage(id, name),
        "add-profile" => LayoutOp::AddProfile(name),
        "remove-profile" => LayoutOp::RemoveProfile(id),
        "rename-profile" => LayoutOp::RenameProfile(id, name),
        "switch-profile" => LayoutOp::SwitchProfile(id),
        "set-profile-apps" => LayoutOp::SetProfileApps(id, apps.unwrap_or_default()),
        other => return Err(format!("unknown change {other}")),
    };
    with_host(&host, |h| h.layout_op(op))?
}

/// VSD Craft's own profiles folder (read only).
fn vsd_profiles_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| {
        PathBuf::from(a)
            .join("HotSpot")
            .join("StreamDock")
            .join("profiles")
    })
}

/// VSD Craft profiles made for the M18 that Agora can bring in.
#[tauri::command]
fn list_vsd_profiles() -> Vec<agora_host::vsd_import::VsdProfile> {
    vsd_profiles_dir()
        .map(|d| agora_host::vsd_import::list(&d, "VSDM18"))
        .unwrap_or_default()
}

#[tauri::command]
fn import_vsd_profiles(
    host: State<Host>,
    dirs: Vec<String>,
) -> Result<agora_host::ImportReport, String> {
    let root = vsd_profiles_dir().ok_or("Can't find VSD Craft's profiles")?;
    // Only folders from VSD Craft's profiles folder.
    let dirs: Vec<PathBuf> = dirs
        .into_iter()
        .map(PathBuf::from)
        .filter(|d| d.starts_with(&root))
        .collect();
    if dirs.is_empty() {
        return Err("Pick at least one profile".into());
    }
    with_host(&host, |h| h.import_vsd(dirs))?
}

#[tauri::command]
fn recent_apps(recent: State<RecentApps>) -> Vec<String> {
    recent.lock().unwrap().clone()
}

/// Watches which program is in front and tells the host when it changes,
/// so profiles can follow apps.
fn watch_foreground(host: Host, recent: RecentApps) {
    let mut last = String::new();
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let Some(app) = foreground::app() else {
            continue;
        };
        if app == last {
            continue;
        }
        last = app.clone();
        if app != "agora.exe" {
            let mut list = recent.lock().unwrap();
            list.retain(|a| *a != app);
            list.insert(0, app.clone());
            list.truncate(12);
        }
        if let Some(h) = host.lock().unwrap().as_ref() {
            h.foreground_app(&app);
        }
    }
}

#[tauri::command]
fn set_key_settings(
    host: State<Host>,
    key: usize,
    settings: serde_json::Value,
) -> Result<(), String> {
    with_host(&host, |h| h.set_settings(key, settings))
}

#[tauri::command]
fn set_key_look(host: State<Host>, key: usize, look: Look) -> Result<(), String> {
    with_host(&host, |h| h.set_look(key, look))
}

/// Brightness and light ring keys: change the dock's saved settings.
fn run_dock_action(
    app: &AppHandle,
    shared: &Shared,
    action: &str,
    settings: &serde_json::Value,
) -> Result<(), String> {
    let mut dock = shared.lock().unwrap();
    let cur = dock.settings.clone();
    let mode = settings.get("mode").and_then(|v| v.as_str()).unwrap_or("");
    let result = if action == "brightness" {
        let value = settings
            .get("value")
            .and_then(|v| v.as_u64())
            .unwrap_or(50)
            .min(100) as u8;
        let level = match mode {
            "down" => cur.brightness.saturating_sub(10),
            "set" => value,
            _ => cur.brightness.saturating_add(10).min(100),
        };
        dock.change(|s| s.brightness = level, vec![Command::Brightness(level)])
    } else {
        let color = settings
            .get("color")
            .and_then(|v| v.as_str())
            .unwrap_or(&cur.ring_color)
            .to_string();
        let on = match mode {
            "on" | "color" => true,
            "off" => false,
            _ => !cur.ring_on,
        };
        // "Set a colour" fills the ring; otherwise it comes back as it was,
        // painted lights included.
        let mut next = cur.clone();
        if mode == "color" {
            next.ring_color = color;
            next.ring_colors.clear();
        }
        let cmds = if on {
            vec![
                Command::Ring(ring_lights(&next)),
                Command::RingBrightness(cur.ring_brightness),
            ]
        } else {
            vec![Command::RingOff]
        };
        dock.change(
            |s| {
                s.ring_on = on;
                s.ring_color = next.ring_color.clone();
                s.ring_colors = next.ring_colors.clone();
            },
            cmds,
        )
    };
    drop(dock);
    let _ = app.emit("settings-changed", ());
    result
}

#[tauri::command]
fn key_previews(previews: State<Previews>) -> Vec<Preview> {
    let map = previews.lock().unwrap();
    map.iter()
        .map(|(&key, image)| Preview {
            key,
            image: Some(image.clone()),
        })
        .collect()
}

/// Agora's data folder: %APPDATA%\Agora on Windows.
fn data_dir(app: &AppHandle) -> PathBuf {
    match std::env::var_os("APPDATA") {
        Some(appdata) if cfg!(windows) => PathBuf::from(appdata).join("Agora"),
        _ => app
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| PathBuf::from("agora-data")),
    }
}

/// Node.js for Node plugins: AGORA_NODE, then node\node.exe next to Agora,
/// then node on PATH, then the Node 20 that VSD Craft ships.
fn find_node() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("AGORA_NODE")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
    {
        return Some(p);
    }
    let exe = if cfg!(windows) { "node.exe" } else { "node" };
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("node")))
    {
        if dir.join(exe).is_file() {
            return Some(dir.join(exe));
        }
    }
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file()))
        .unwrap_or(false);
    if on_path {
        // The host runs plain "node" from PATH.
        return None;
    }
    std::env::var_os("ProgramFiles(x86)")
        .map(|pf| {
            PathBuf::from(pf)
                .join("VSD Craft")
                .join("node")
                .join("node20.exe")
        })
        .filter(|p| p.is_file())
}

fn start_host(
    app: &AppHandle,
    use_vsd_craft: bool,
) -> Result<(HostHandle, Receiver<HostEvent>), String> {
    let data = data_dir(app);
    let own = data.join("plugins");
    let _ = std::fs::create_dir_all(&own);
    let mut plugin_dirs = vec![(own, Source::Agora)];
    if let Some(vsd) = manifest::vsd_craft_plugin_dir().filter(|_| use_vsd_craft) {
        // Read only: plugins installed for VSD Craft, used as they are.
        plugin_dirs.push((vsd, Source::VsdCraft));
    }
    let config = Config {
        plugin_dirs,
        data_dir: data,
        node: find_node(),
        device: DeviceDesc {
            id: "M18".into(),
            name: "Stream Dock M18".into(),
            rows: 3,
            columns: 5,
        },
    };
    let (tx, rx) = std::sync::mpsc::channel();
    Ok((HostHandle::start(config, tx)?, rx))
}

/// Starts the plugin host (stopping a running one first) and its event thread.
fn restart_host(
    app: &AppHandle,
    host: &Host,
    shared: &Shared,
    previews: &Previews,
) -> Result<(), String> {
    let old = host.lock().unwrap().take();
    drop(old);
    previews.lock().unwrap().clear();
    let use_vsd = shared.lock().unwrap().settings.use_vsd_craft_plugins;
    let (h, events) = start_host(app, use_vsd)?;
    if shared.lock().unwrap().worker.is_some() {
        h.device_connected(true);
    }
    *host.lock().unwrap() = Some(h);
    let (handle, shared, previews) = (app.clone(), shared.clone(), previews.clone());
    std::thread::Builder::new()
        .name("agora-host-events".into())
        .spawn(move || host_events(handle, shared, previews, events))
        .map_err(|e| e.to_string())?;
    let _ = app.emit("host-restarted", ());
    Ok(())
}

fn png_data_uri(img: &image::DynamicImage) -> Option<String> {
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    ))
}

/// Opens an http(s) address in the default browser. The host has already
/// checked the scheme.
fn open_url(url: &str) {
    #[cfg(windows)]
    let cmd = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let cmd = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = std::process::Command::new("xdg-open").arg(url).spawn();
    let _ = cmd;
}

/// Shows a folder in Explorer (or the system file manager).
fn open_folder(dir: &std::path::Path) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!("{} doesn't exist", dir.display()));
    }
    #[cfg(windows)]
    let cmd = std::process::Command::new("explorer.exe").arg(dir).spawn();
    #[cfg(target_os = "macos")]
    let cmd = std::process::Command::new("open").arg(dir).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = std::process::Command::new("xdg-open").arg(dir).spawn();
    cmd.map(|_| ()).map_err(|e| e.to_string())
}

fn plugin_dir(host: &State<Host>, uuid: &str) -> Result<(PathBuf, String), String> {
    let plugins = with_host(host, |h| h.plugins())?;
    let p = plugins
        .into_iter()
        .find(|p| p.uuid == uuid)
        .ok_or("That plugin isn't loaded")?;
    Ok((PathBuf::from(p.dir), p.source))
}

#[tauri::command]
fn open_plugins_folder(app: AppHandle) -> Result<(), String> {
    open_folder(&data_dir(&app).join("plugins"))
}

#[tauri::command]
fn open_plugin_folder(host: State<Host>, uuid: String) -> Result<(), String> {
    open_folder(&plugin_dir(&host, &uuid)?.0)
}

/// Removes a plugin. One in Agora's folder goes to the Recycle Bin. One
/// from VSD Craft's folder is only hidden: Agora never changes VSD Craft's
/// files. Returns "deleted" or "hidden".
#[tauri::command]
fn remove_plugin(host: State<Host>, uuid: String) -> Result<String, String> {
    let (dir, source) = plugin_dir(&host, &uuid)?;
    if source != "Agora" {
        with_host(&host, |h| h.set_hidden(&uuid, true))?;
        return Ok("hidden".into());
    }
    with_host(&host, |h| h.forget(&uuid))?;
    trash::delete(&dir)
        .map_err(|e| format!("Agora stopped the plugin but couldn't delete its folder: {e}"))?;
    Ok("deleted".into())
}

#[tauri::command]
fn show_plugin(host: State<Host>, uuid: String) -> Result<(), String> {
    with_host(&host, |h| h.set_hidden(&uuid, false))
}

fn html_plugin_label(plugin: &str) -> String {
    let safe: String = plugin
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("plugin-{safe}")
}

/// Carries out what the host asks: key faces to the dock and the window,
/// browser pages, HTML plugins, status and logs.
fn host_events(app: AppHandle, shared: Shared, previews: Previews, events: Receiver<HostEvent>) {
    for ev in events {
        match ev {
            HostEvent::Face { key, face: f } => {
                let (size, orientation) = {
                    let dock = shared.lock().unwrap();
                    (
                        dock.worker.as_ref().map_or(KEY_SIZE, |w| w.layout.key_size),
                        dock.orientation,
                    )
                };
                let img = face::render(&f, size);
                {
                    let dock = shared.lock().unwrap();
                    if let Some(w) = dock.worker.as_ref() {
                        if key < w.layout.key_count() {
                            if let Ok(jpeg) = keyimage::to_key_jpeg(&img, size, orientation) {
                                w.send(Command::SetKeyJpeg(key, jpeg));
                            }
                        }
                    }
                }
                let empty = f == face::Face::default();
                let image = if empty { None } else { png_data_uri(&img) };
                {
                    let mut map = previews.lock().unwrap();
                    match &image {
                        Some(uri) => map.insert(key, uri.clone()),
                        None => map.remove(&key),
                    };
                }
                let _ = app.emit("key-preview", Preview { key, image });
            }
            HostEvent::OpenUrl(url) => open_url(&url),
            HostEvent::StartHtmlPlugin { plugin, url } => {
                let label = html_plugin_label(&plugin);
                if app.get_webview_window(&label).is_none() {
                    if let Ok(parsed) = url.parse() {
                        let built =
                            WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(parsed))
                                .title(format!("Agora plugin {plugin}"))
                                .visible(false)
                                .skip_taskbar(true)
                                .build();
                        if let Err(e) = built {
                            let _ = app.emit(
                                "plugin-log",
                                PluginLine {
                                    plugin,
                                    message: format!("Couldn't start: {e}"),
                                },
                            );
                        }
                    }
                }
            }
            HostEvent::StopHtmlPlugin { plugin } => {
                if let Some(w) = app.get_webview_window(&html_plugin_label(&plugin)) {
                    let _ = w.destroy();
                }
            }
            HostEvent::PluginStatus { plugin, status } => {
                let _ = app.emit(
                    "plugin-status",
                    PluginLine {
                        plugin,
                        message: status,
                    },
                );
            }
            HostEvent::Log { plugin, message } => {
                let _ = app.emit("plugin-log", PluginLine { plugin, message });
            }
            HostEvent::PageChanged => {
                let _ = app.emit("page-changed", ());
            }
            HostEvent::Run { action, settings } => {
                let result = match action.as_str() {
                    "brightness" | "ring" => run_dock_action(&app, &shared, &action, &settings),
                    _ => system::run(&action, &settings, open_url),
                };
                if let Err(message) = result {
                    let _ = app.emit(
                        "plugin-log",
                        PluginLine {
                            plugin: "Agora".into(),
                            message,
                        },
                    );
                }
            }
        }
    }
}

/// Keeps a dock connected: finds it, opens it, forwards its events, and
/// tries again every few seconds after it goes away.
fn connection_loop(app: AppHandle, shared: Shared, host: Host) {
    let mut last_message = String::new();
    let mut report = |app: &AppHandle, dock: &mut Dock, msg: String| {
        dock.message = msg.clone();
        if msg != last_message {
            let _ = app.emit("dock-status", status(dock, msg.clone()));
            last_message = msg;
        }
    };
    let tell_host = |f: &dyn Fn(&HostHandle)| {
        if let Some(h) = host.lock().unwrap().as_ref() {
            f(h);
        }
    };
    loop {
        let opened = discover::api().and_then(|api| {
            let found = discover::find(&api);
            let first = found.first().ok_or(agora_device::Error::Disconnected)?;
            let model = first.model.to_string();
            let dock = discover::open_m18(&api, first)?;
            Ok((model, DockWorker::start(dock)?))
        });
        match opened {
            Ok((model, (worker, events))) => {
                {
                    let mut dock = shared.lock().unwrap();
                    apply_settings(&worker, &dock.settings);
                    dock.worker = Some(worker);
                    dock.model = Some(model.clone());
                    report(&app, &mut dock, format!("{model} connected"));
                }
                // Put every key's face back on the freshly opened dock.
                tell_host(&|h| {
                    h.device_connected(true);
                    h.redraw_all();
                });
                forward_events(&app, &host, events);
                tell_host(&|h| h.device_connected(false));
                let mut dock = shared.lock().unwrap();
                dock.worker = None;
                report(
                    &app,
                    &mut dock,
                    format!("{model} disconnected. Looking for it again..."),
                );
            }
            Err(agora_device::Error::Disconnected) => {
                let mut dock = shared.lock().unwrap();
                report(
                    &app,
                    &mut dock,
                    "No dock found. Plug in the M18 and close VSD Craft.".into(),
                );
            }
            Err(e) => {
                let mut dock = shared.lock().unwrap();
                report(&app, &mut dock, e.to_string());
            }
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}

/// Sends dock input to the plugins and the window until the worker stops.
fn forward_events(app: &AppHandle, host: &Host, events: Receiver<Event>) {
    for ev in events {
        match ev {
            Event::Input(i) => {
                let (kind, index) = match i.control {
                    Control::Key(k) => ("key", k),
                    Control::Button(Button::Left) => ("button", 0),
                    Control::Button(Button::Middle) => ("button", 1),
                    Control::Button(Button::Right) => ("button", 2),
                    Control::Unknown(_) => ("unknown", 0),
                };
                if kind == "key" {
                    if let Some(h) = host.lock().unwrap().as_ref() {
                        h.key(index, i.pressed);
                    }
                }
                let _ = app.emit(
                    "dock-input",
                    Input {
                        kind,
                        index,
                        pressed: i.pressed,
                        raw_id: i.raw_id,
                    },
                );
            }
            Event::Error(e) => {
                let _ = app.emit("dock-error", e);
            }
            Event::Stopped => break,
        }
    }
}

fn main() {
    let shared: Shared = Arc::default();
    let host: Host = Arc::default();
    let previews: Previews = Arc::default();
    let recent: RecentApps = Arc::default();

    tauri::Builder::default()
        // A second copy (e.g. opened while the sign-in one runs in the
        // tray) shows the first one's window instead of fighting over the dock.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(shared.clone())
        .manage(host.clone())
        .manage(previews.clone())
        .manage(recent.clone())
        .invoke_handler(tauri::generate_handler![
            dock_status,
            set_brightness,
            set_ring,
            set_ring_brightness,
            ring_off,
            get_settings,
            set_use_vsd_craft_plugins,
            list_actions,
            list_plugins,
            key_info,
            place_action,
            clear_key,
            move_key,
            open_plugins_folder,
            open_plugin_folder,
            remove_plugin,
            show_plugin,
            key_previews,
            get_layout,
            layout_op,
            set_start_with_windows,
            set_ring_lights,
            recent_apps,
            list_vsd_profiles,
            import_vsd_profiles,
            set_key_settings,
            set_key_look
        ])
        .setup(move |app| {
            let show = MenuItem::with_id(app, "show", "Show Agora", true, None::<&str>)?;
            let quit =
                MenuItem::with_id(app, "quit", "Quit (give the dock back)", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().expect("app icon"))
                .tooltip("Agora")
                .menu(&menu)
                .on_menu_event(|app, ev| match ev.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => {
                        // Stop the plugins and save, then hand the dock back
                        // to its stock screen by dropping the worker.
                        let stopped = app.state::<Host>().lock().unwrap().take();
                        drop(stopped);
                        app.state::<Shared>().lock().unwrap().worker = None;
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            let handle = app.handle().clone();
            {
                let mut dock = shared.lock().unwrap();
                dock.settings_path = data_dir(&handle).join("settings.json");
                dock.settings = load_settings(&dock.settings_path);
                // Keeps the entry pointing at this agora.exe if it moved.
                if dock.settings.start_with_windows {
                    let _ = register_autostart(true);
                }
            }
            // Started by Windows at sign-in: stay in the tray.
            if !std::env::args().any(|a| a == "--tray") {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                }
            }
            if let Err(e) = restart_host(&handle, &host, &shared, &previews) {
                eprintln!("plugin host didn't start: {e}");
            }

            let (handle, shared, host) = (app.handle().clone(), shared.clone(), host.clone());
            std::thread::Builder::new()
                .name("agora-connect".into())
                .spawn(move || connection_loop(handle, shared, host))?;
            let watched = app.state::<Host>().inner().clone();
            std::thread::Builder::new()
                .name("agora-foreground".into())
                .spawn(move || watch_foreground(watched, recent))?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the main window keeps Agora running in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Agora");
}
