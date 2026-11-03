// Agora app shell: tray icon, dock connection with reconnect, and the plugin
// host. Keys show what their plugins draw; presses go to the plugins.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod apps;
mod extra;
mod foreground;
mod more;
mod saver;
mod system;

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agora_device::worker::{Command, DockWorker, Event};
use agora_device::{discover, keyimage, models, Action, Control, DeviceSpec, Rgb};
use agora_host::{
    face, manifest, ActionInfo, Config, DeviceDesc, DialInput, HostEvent, HostHandle, KeyInfo,
    LayoutOp, LayoutView, Look, PageOverview, PluginInfo, Source, BUTTON_BASE, DIAL_BASE,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};

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
    /// The model last plugged in ("M18", "N4"...), so the editor shows the
    /// right keys before the dock connects.
    last_model: String,
    /// "system" (follow Windows), "light" or "dark".
    theme: String,
    /// Screen saver on the dock: "off", or one of [`saver::SAVERS`].
    saver: String,
    /// Minutes the dock sits unused before the screen saver starts.
    saver_minutes: u32,
    /// The GIF for the "gif" screen saver.
    saver_gif: String,
    /// The temple builds itself on the dock when it connects.
    dock_intro: bool,
    /// What the window shows when Agora opens: "update" (the temple, on the
    /// first run and after an update), "agora" (every time), "video" or "off".
    intro: String,
    /// The user's own intro video (MP4 or WebM).
    intro_video: String,
    /// The Agora version whose intro was last played, for "update".
    intro_seen_version: String,
    /// How long a key must be down to count as held, in milliseconds.
    hold_ms: u64,
    /// This Agora's version. Not saved; filled in for the window.
    #[serde(skip_deserializing)]
    app_version: String,
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
            last_model: models::M18.id.into(),
            theme: "system".into(),
            saver: "off".into(),
            saver_minutes: 10,
            saver_gif: String::new(),
            dock_intro: true,
            intro: "update".into(),
            intro_video: String::new(),
            intro_seen_version: String::new(),
            hold_ms: 500,
            app_version: String::new(),
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
            message: String::new(),
            settings: Settings::default(),
            settings_path: PathBuf::from("settings.json"),
        }
    }
}

impl Dock {
    /// The model plugged in, or the last one seen.
    fn current_spec(&self) -> DeviceSpec {
        self.worker
            .as_ref()
            .map(|w| w.spec.clone())
            .unwrap_or_else(|| {
                models::by_id(&self.settings.last_model)
                    .unwrap_or(&models::M18)
                    .spec()
            })
    }

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
    /// The connected dock's name.
    model: Option<String>,
    /// The model the editor shows: the connected one or the last one seen.
    model_id: String,
    model_name: String,
    rows: usize,
    columns: usize,
    buttons: usize,
    dials: usize,
    /// Knobs have a strip screen above them (the N4).
    strip: bool,
    ring_leds: usize,
    /// Tried on a real dock of this model.
    tested: bool,
    message: String,
}

#[derive(Clone, Serialize)]
struct Input {
    /// "key", "button", "dial", "touch", "strip" or "unknown"
    kind: &'static str,
    /// Key index from 0 at the top left; buttons and knobs from 0 on the left.
    index: usize,
    pressed: bool,
    /// Knob steps or swipe direction; 0 for presses.
    turn: i8,
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
    let m = dock.current_spec();
    Status {
        connected: dock.worker.is_some(),
        model: dock.model.clone(),
        model_id: m.id.clone(),
        model_name: m.name.clone(),
        rows: m.rows,
        columns: m.columns,
        buttons: m.buttons,
        dials: m.dials,
        strip: m.has_strip(),
        ring_leds: m.ring_leds,
        tested: m.tested,
        message: message.into(),
    }
}

/// The dock as the plugin host describes it to plugins.
fn device_desc(m: &DeviceSpec) -> DeviceDesc {
    DeviceDesc {
        id: m.id.clone(),
        name: m.name.clone(),
        rows: m.rows,
        columns: m.columns,
        buttons: m.buttons,
        dials: m.dials,
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
    let mut s = state.lock().unwrap().settings.clone();
    s.app_version = env!("CARGO_PKG_VERSION").into();
    s
}

/// The window played the intro for this version.
#[tauri::command]
fn intro_played(state: State<Shared>) {
    let mut dock = state.lock().unwrap();
    dock.settings.intro_seen_version = env!("CARGO_PKG_VERSION").into();
    save_settings(&dock.settings_path, &dock.settings);
}

#[tauri::command]
fn set_hold_time(state: State<Shared>, host: State<Host>, ms: u64) -> Result<(), String> {
    let ms = ms.clamp(300, 1000);
    {
        let mut dock = state.lock().unwrap();
        dock.settings.hold_ms = ms;
        save_settings(&dock.settings_path, &dock.settings);
    }
    with_host(&host, |h| h.set_hold_time(ms))
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

#[tauri::command]
fn copy_key(host: State<Host>, key: usize) -> Result<(), String> {
    with_host(&host, |h| h.copy(key))?
}

#[tauri::command]
fn paste_key(host: State<Host>, key: usize) -> Result<(), String> {
    with_host(&host, |h| h.paste(key))?
}

#[tauri::command]
fn duplicate_key(host: State<Host>, from: usize, to: usize) -> Result<(), String> {
    with_host(&host, |h| h.duplicate(from, to))?
}

#[tauri::command]
fn move_key_to_page(host: State<Host>, key: usize, page: String) -> Result<usize, String> {
    with_host(&host, |h| h.move_to_page(key, &page))?
}

/// Runs a key's action as if it were tapped on the dock.
#[tauri::command]
fn press_key(host: State<Host>, key: usize) -> Result<(), String> {
    with_host(&host, |h| h.press(key))
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

/// Light, dark or the Windows setting, for the window and its title bar.
#[tauri::command]
fn set_theme(app: AppHandle, state: State<Shared>, theme: String) -> Result<(), String> {
    let native = match theme.as_str() {
        "light" => Some(tauri::Theme::Light),
        "dark" => Some(tauri::Theme::Dark),
        "system" => None,
        _ => return Err("Unknown theme".into()),
    };
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_theme(native);
    }
    let mut dock = state.lock().unwrap();
    if dock.settings.theme != theme {
        dock.settings.theme = theme;
        save_settings(&dock.settings_path, &dock.settings);
    }
    Ok(())
}

#[tauri::command]
fn page_overview(host: State<Host>) -> Result<Option<PageOverview>, String> {
    with_host(&host, |h| h.overview())
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

/// VSD Craft profiles made for the model in use that Agora can bring in.
#[tauri::command]
fn list_vsd_profiles(state: State<Shared>) -> Vec<agora_host::vsd_import::VsdProfile> {
    // Only profiles made for this model; VSD Craft's names for the others
    // aren't known yet.
    let Some(device) = state.lock().unwrap().current_spec().vsd_device else {
        return Vec::new();
    };
    vsd_profiles_dir()
        .map(|d| agora_host::vsd_import::list(&d, &device))
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

#[tauri::command]
fn set_key_hold(host: State<Host>, key: usize, steps: Vec<serde_json::Value>) -> Result<(), String> {
    with_host(&host, |h| h.set_hold(key, steps))
}

#[tauri::command]
fn set_screen_saver(state: State<Shared>, mode: String, minutes: u32, gif: String) -> Result<(), String> {
    if mode != "off" && !saver::SAVERS.iter().any(|(id, _)| *id == mode) {
        return Err("Unknown screen saver".into());
    }
    if !gif.trim().is_empty() {
        saver::local_media(&gif, &["gif"], 40 * 1024 * 1024)?;
    }
    let mut dock = state.lock().unwrap();
    dock.settings.saver = mode;
    dock.settings.saver_minutes = minutes.clamp(1, 240);
    dock.settings.saver_gif = gif;
    save_settings(&dock.settings_path, &dock.settings);
    Ok(())
}

/// Plays a screen saver on the dock now; the next press stops it.
#[tauri::command]
fn preview_screen_saver(app: AppHandle, mode: String, gif: String) -> Result<(), String> {
    if app.state::<Shared>().lock().unwrap().worker.is_none() {
        return Err("No dock connected".into());
    }
    if mode == "intro" {
        saver::start(&app, saver::Show::Intro);
    } else {
        saver::start(&app, saver::Show::Saver { mode, gif });
    }
    Ok(())
}

#[tauri::command]
fn set_intro(state: State<Shared>, intro: String, video: String, dock_intro: bool) -> Result<(), String> {
    if !matches!(intro.as_str(), "update" | "agora" | "video" | "off") {
        return Err("Unknown intro".into());
    }
    if !video.trim().is_empty() {
        check_intro_video(&video)?;
    }
    let mut dock = state.lock().unwrap();
    dock.settings.intro = intro;
    dock.settings.intro_video = video;
    dock.settings.dock_intro = dock_intro;
    save_settings(&dock.settings_path, &dock.settings);
    Ok(())
}

/// Checks the user's intro video is one: a local MP4/WebM file whose
/// first bytes say so, under 300 MB. Anything else is refused, so the
/// setting can't be used to read some other file.
fn check_intro_video(path: &str) -> Result<std::path::PathBuf, String> {
    let real = saver::local_media(path, &["mp4", "webm", "m4v"], 300 * 1024 * 1024)?;
    let mut head = [0u8; 12];
    {
        use std::io::Read;
        let mut f = std::fs::File::open(&real).map_err(|e| e.to_string())?;
        f.read_exact(&mut head).map_err(|_| "That isn't a video".to_string())?;
    }
    let mp4 = &head[4..8] == b"ftyp";
    let webm = head[..4] == [0x1A, 0x45, 0xDF, 0xA3];
    if !(mp4 || webm) {
        return Err("That isn't an MP4 or WebM video".into());
    }
    Ok(real)
}

/// The user's intro video, as bytes the window turns into a video.
#[tauri::command]
fn intro_video(state: State<Shared>) -> Result<tauri::ipc::Response, String> {
    let path = state.lock().unwrap().settings.intro_video.clone();
    let real = check_intro_video(&path)?;
    std::fs::read(real)
        .map(tauri::ipc::Response::new)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn audio_outputs() -> Result<Vec<more::Output>, String> {
    more::win::outputs()
}

#[tauri::command]
fn audio_apps() -> Vec<String> {
    more::win::audio_apps()
}

/// A program's own icon as a PNG data URI, for an Open key.
#[tauri::command]
fn app_icon(path: String) -> Option<String> {
    let path = path.trim().trim_matches('"');
    let img = if apps::is_shell_path(path) {
        apps::icon(path, 128)?
    } else {
        more::win::app_icon(path)?
    };
    png_data_uri(&image::DynamicImage::ImageRgba8(img))
}

/// The apps installed on this PC (Start > All apps), for an Open key's
/// "Installed app" picker. Kept after the first look; `refresh` looks again.
#[tauri::command]
async fn installed_apps(refresh: bool) -> Vec<apps::App> {
    static CACHE: Mutex<Option<Vec<apps::App>>> = Mutex::new(None);
    if !refresh {
        if let Some(list) = CACHE.lock().unwrap().clone() {
            return list;
        }
    }
    let list = tauri::async_runtime::spawn_blocking(|| {
        apps::list(|img| png_data_uri(&image::DynamicImage::ImageRgba8(img)))
    })
    .await
    .unwrap_or_default();
    *CACHE.lock().unwrap() = Some(list.clone());
    list
}

/// The snippets of the Snippets key last pressed, for its pop-up.
fn snippet_state() -> &'static Mutex<(Vec<String>, bool)> {
    static S: std::sync::OnceLock<Mutex<(Vec<String>, bool)>> = std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

#[tauri::command]
fn snippet_list() -> Vec<String> {
    snippet_state().lock().unwrap().0.clone()
}

/// Hides the pop-up, lets the window before it come back to the front,
/// then types the snippet there.
#[tauri::command]
fn pick_snippet(app: AppHandle, index: Option<usize>) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("snippets") {
        let _ = w.hide();
    }
    let (list, enter) = snippet_state().lock().unwrap().clone();
    let front = SNIPPET_FRONT.load(std::sync::atomic::Ordering::Relaxed);
    let Some(text) = index.and_then(|i| list.get(i).cloned()) else {
        return Ok(());
    };
    std::thread::spawn(move || {
        foreground::bring_back(front);
        std::thread::sleep(Duration::from_millis(150));
        let _ = system::type_text(&text, enter);
    });
    Ok(())
}

/// The window that was in front when the Snippets key was pressed.
static SNIPPET_FRONT: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Shows the Snippets pop-up beside the mouse.
fn show_snippets(app: &AppHandle, settings: &serde_json::Value) -> Result<(), String> {
    let list: Vec<String> = settings
        .get("snippets")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .take(20)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if list.is_empty() {
        return Err("Add snippets in the key's settings".into());
    }
    let enter = settings.get("enter").and_then(|v| v.as_bool()).unwrap_or(false);
    let rows = list.len();
    *snippet_state().lock().unwrap() = (list, enter);
    SNIPPET_FRONT.store(foreground::front_window(), std::sync::atomic::Ordering::Relaxed);
    let (x, y) = foreground::cursor();
    let height = (rows as f64 * 36.0 + 20.0).min(600.0);
    let app2 = app.clone();
    app.run_on_main_thread(move || {
        let window = match app2.get_webview_window("snippets") {
            Some(w) => w,
            None => match WebviewWindowBuilder::new(&app2, "snippets", WebviewUrl::App("index.html".into()))
                .initialization_script("window.__agoraSnippets = true;")
                .title("Agora snippets")
                .decorations(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .visible(false)
                .inner_size(320.0, height)
                .build()
            {
                Ok(w) => w,
                Err(_) => return,
            },
        };
        let _ = window.set_size(tauri::LogicalSize::new(320.0, height));
        let _ = window.set_position(tauri::PhysicalPosition::new(x + 8, y + 8));
        let _ = window.emit("snippets-changed", ());
        let _ = window.show();
        let _ = window.set_focus();
    })
    .map_err(|e| e.to_string())
}

/// Starts the screen saver once the dock has sat unused long enough.
fn watch_idle(app: AppHandle) {
    loop {
        std::thread::sleep(Duration::from_secs(2));
        let (mode, minutes, gif, connected) = {
            let shared = app.state::<Shared>();
            let dock = shared.lock().unwrap();
            (
                dock.settings.saver.clone(),
                dock.settings.saver_minutes.max(1),
                dock.settings.saver_gif.clone(),
                dock.worker.is_some(),
            )
        };
        if mode == "off"
            || !connected
            || saver::active()
            || DOCK_ASLEEP.load(std::sync::atomic::Ordering::Relaxed)
            || saver::idle() < Duration::from_secs(minutes as u64 * 60)
        {
            continue;
        }
        saver::start(&app, saver::Show::Saver { mode, gif });
    }
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
    if action == "dock-sleep" {
        // Dark until the next press; the saved settings stay as they are.
        DOCK_ASLEEP.store(true, std::sync::atomic::Ordering::Relaxed);
        return match &dock.worker {
            Some(w) => {
                w.send(Command::Brightness(0));
                w.send(Command::RingOff);
                Ok(())
            }
            None => Err("No dock connected".into()),
        };
    }
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
/// then the Node 20 that VSD Craft ships, then node on PATH. Stream Dock
/// plugins are built for VSD Craft's Node 20, and some carry add-ons that
/// only load on the Node they were built for.
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
    // Otherwise the host runs plain "node" from PATH.
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
    model: &DeviceSpec,
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
        device: device_desc(model),
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
    let (use_vsd, model) = {
        let dock = shared.lock().unwrap();
        (dock.settings.use_vsd_craft_plugins, dock.current_spec())
    };
    let (h, events) = start_host(app, use_vsd, &model)?;
    h.set_hold_time(shared.lock().unwrap().settings.hold_ms);
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

/// Opens one of Agora's own pages in the browser: the issue tracker or Ko-fi.
#[tauri::command]
fn save_password(name: String, secret: String) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Give the password a name first".into());
    }
    extra::win::save_secret(&extra::credential_name(&name), &secret)
}

#[tauri::command]
fn password_saved(name: String) -> bool {
    !name.trim().is_empty() && extra::win::has_secret(&extra::credential_name(&name))
}

#[tauri::command]
fn forget_password(name: String) -> Result<(), String> {
    extra::win::delete_secret(&extra::credential_name(&name))
}

#[tauri::command]
fn open_link(which: String) -> Result<(), String> {
    let url = match which.as_str() {
        "issues" => "https://gitlab.com/overture-group1/agora/-/issues",
        "kofi" => "https://ko-fi.com/overturesys",
        _ => return Err("Unknown link".into()),
    };
    open_url(url);
    Ok(())
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

/// Hides a plugin and clears its keys, so the window can offer Undo before
/// the folder is really deleted.
#[tauri::command]
fn hide_plugin(host: State<Host>, uuid: String) -> Result<(), String> {
    with_host(&host, |h| h.set_hidden(&uuid, true))
}

#[tauri::command]
fn undo(host: State<Host>) -> Result<bool, String> {
    with_host(&host, |h| h.undo())
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

/// How many times each key's face has been replaced, so a press bounce
/// stops as soon as the key gets a newer face.
fn face_gens() -> &'static Mutex<HashMap<usize, u64>> {
    static GENS: std::sync::OnceLock<Mutex<HashMap<usize, u64>>> = std::sync::OnceLock::new();
    GENS.get_or_init(Default::default)
}

/// Sizes for the press bounce: the icon sinks in, then springs back
/// slightly past full size. About a quarter of a second in all.
const BOUNCE: [f32; 7] = [0.88, 0.82, 0.86, 0.95, 1.03, 1.01, 1.0];
const BOUNCE_FRAME: Duration = Duration::from_millis(35);

/// Plays the press bounce on a built-in key, on its own thread.
fn press_bounce(shared: &Shared, key: usize, f: face::Face) {
    let (model, connected) = {
        let dock = shared.lock().unwrap();
        (dock.current_spec(), dock.worker.is_some())
    };
    let Some(screen) = model.keys.get(key).copied() else {
        return;
    };
    if !connected || saver::active() {
        return;
    }
    let start = face_gens().lock().unwrap().get(&key).copied().unwrap_or(0);
    let shared = shared.clone();
    let _ = std::thread::Builder::new()
        .name("agora-bounce".into())
        .spawn(move || {
            for zoom in BOUNCE {
                let begun = Instant::now();
                let img = face::render_scaled(&f, screen.width, zoom);
                let Ok(jpeg) = model.encode(&img, screen) else {
                    return;
                };
                {
                    let gens = face_gens().lock().unwrap();
                    if gens.get(&key).copied().unwrap_or(0) != start {
                        return;
                    }
                    match shared.lock().unwrap().worker.as_ref() {
                        Some(w) => {
                            w.send(Command::SetKeyImage(key, jpeg));
                        }
                        None => return,
                    }
                }
                std::thread::sleep(BOUNCE_FRAME.saturating_sub(begun.elapsed()));
            }
        });
}

/// Carries out what the host asks: key faces to the dock and the window,
/// browser pages, HTML plugins, status and logs.
fn host_events(app: AppHandle, shared: Shared, previews: Previews, events: Receiver<HostEvent>) {
    for ev in events {
        match ev {
            HostEvent::Face { key, face: f } => {
                let (model, connected) = {
                    let dock = shared.lock().unwrap();
                    (dock.current_spec(), dock.worker.is_some())
                };
                // Screen keys get their own size; a knob's face goes on its
                // strip segment (the N4); buttons have no screen.
                let mut img = face::render(&f, model.key_size());
                let mut send = None;
                if let Some(screen) = model.keys.get(key) {
                    if screen.width != model.key_size() {
                        img = face::render(&f, screen.width);
                    }
                    send = model
                        .encode(&img, *screen)
                        .ok()
                        .map(|j| Command::SetKeyImage(key, j));
                } else if let Some((d, seg)) = key
                    .checked_sub(DIAL_BASE)
                    .and_then(|d| model.strip.get(d).map(|g| (d, *g)))
                {
                    img = face::render(&f, seg.height);
                    let wide = keyimage::letterbox(&img, seg.width, seg.height);
                    send = model
                        .encode(&wide, seg)
                        .ok()
                        .map(|j| Command::SetStripImage(d, j));
                }
                {
                    // A new face ends any press bounce still playing on it.
                    let mut gens = face_gens().lock().unwrap();
                    *gens.entry(key).or_default() += 1;
                    // While a show plays on the dock, faces wait; it redraws
                    // every key when it ends.
                    if let (Some(cmd), true, false) = (send, connected, saver::active()) {
                        if let Some(w) = shared.lock().unwrap().worker.as_ref() {
                            w.send(cmd);
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
            HostEvent::Pressed { key, face: f } => press_bounce(&shared, key, f),
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
            HostEvent::Run { context, action, settings } => {
                let result = match action.as_str() {
                    "brightness" | "ring" | "dock-sleep" => {
                        run_dock_action(&app, &shared, &action, &settings)
                    }
                    "web-request" | "run-command" => {
                        let app = app.clone();
                        let _ = std::thread::Builder::new().name("agora-action".into()).spawn(move || {
                            let (ok, text) = match more::run_with_result(&action, &settings) {
                                Ok(text) => (true, text),
                                Err(e) => (false, Some(e)),
                            };
                            if let Some(h) = app.state::<Host>().lock().unwrap().as_ref() {
                                h.action_result(&context, ok, text);
                            }
                        });
                        Ok(())
                    }
                    "snippets" => show_snippets(&app, &settings),
                    _ => more::run(&action, &settings)
                        .or_else(|| extra::run(&action, &settings))
                        .unwrap_or_else(|| system::run(&action, &settings, open_url)),
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
            let dock = discover::open(&api, first)?;
            Ok(DockWorker::start(dock)?)
        });
        match opened {
            Ok((worker, events)) => {
                let m = worker.spec.clone();
                let model = m.name.clone();
                {
                    let mut dock = shared.lock().unwrap();
                    apply_settings(&worker, &dock.settings);
                    dock.worker = Some(worker);
                    dock.model = Some(model.clone());
                    if dock.settings.last_model != m.id {
                        dock.settings.last_model = m.id.clone();
                        save_settings(&dock.settings_path, &dock.settings);
                    }
                    let msg = if m.tested {
                        format!("{model} connected")
                    } else {
                        format!("{model} connected. Agora hasn't been tried on this model yet; tell us what works.")
                    };
                    report(&app, &mut dock, msg);
                }
                // Put every key's face back on the freshly opened dock.
                tell_host(&|h| {
                    h.set_device(device_desc(&m));
                    h.device_connected(true);
                    h.redraw_all();
                });
                saver::touch();
                if shared.lock().unwrap().settings.dock_intro {
                    saver::start(&app, saver::Show::Intro);
                }
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
                    "No dock found. Plug in your Stream Dock and close VSD Craft.".into(),
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

/// Set by Dock sleep; the next press wakes the dock instead of running a key.
static DOCK_ASLEEP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Puts the dock's brightness and ring back as they were saved.
fn wake_dock(app: &AppHandle) {
    let shared = app.state::<Shared>();
    let dock = shared.lock().unwrap();
    if let Some(w) = &dock.worker {
        let s = &dock.settings;
        w.send(Command::Brightness(s.brightness));
        if s.ring_on {
            w.send(Command::Ring(ring_lights(s)));
            w.send(Command::RingBrightness(s.ring_brightness));
        }
    }
}

/// Sends dock input to the plugins and the window until the worker stops.
fn forward_events(app: &AppHandle, host: &Host, events: Receiver<Event>) {
    let mut swallow_release = false;
    for ev in events {
        match ev {
            Event::Input(i) => {
                saver::touch();
                // The first press after a screen saver only stops it.
                if matches!(i.action, Action::Press | Action::Turn(_) | Action::Swipe(_))
                    && saver::stop(app)
                {
                    swallow_release = i.action == Action::Press;
                    continue;
                }
                if i.action == Action::Press
                    && DOCK_ASLEEP.swap(false, std::sync::atomic::Ordering::Relaxed)
                {
                    wake_dock(app);
                    swallow_release = true;
                    continue;
                }
                if swallow_release && i.action == Action::Release {
                    swallow_release = false;
                    continue;
                }
                let pressed = i.action == Action::Press;
                let turn = match i.action {
                    Action::Turn(t) | Action::Swipe(t) => t,
                    _ => 0,
                };
                let (kind, index) = match i.control {
                    Control::Key(k) => ("key", k),
                    Control::Button(b) => ("button", b),
                    Control::Dial(d) => ("dial", d),
                    Control::Touch(t) => ("touch", t),
                    Control::Strip => ("strip", 0),
                    Control::Unknown(_) => ("unknown", 0),
                };
                if let Some(h) = host.lock().unwrap().as_ref() {
                    match (i.control, i.action) {
                        (Control::Key(k), Action::Press | Action::Release) => h.key(k, pressed),
                        (Control::Button(b), Action::Press | Action::Release) => {
                            h.key(BUTTON_BASE + b, pressed)
                        }
                        (Control::Dial(d), Action::Turn(t)) => h.dial(d, DialInput::Turn(t.into())),
                        (Control::Dial(d), Action::Press) => h.dial(d, DialInput::Press),
                        (Control::Dial(d), Action::Release) => h.dial(d, DialInput::Release),
                        (Control::Touch(t), Action::Press) => h.dial(t, DialInput::Tap),
                        (Control::Strip, Action::Swipe(s)) => h.swipe(s.into()),
                        _ => {}
                    }
                }
                let _ = app.emit(
                    "dock-input",
                    Input {
                        kind,
                        index,
                        pressed,
                        turn,
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
            copy_key,
            paste_key,
            duplicate_key,
            move_key_to_page,
            press_key,
            intro_played,
            set_hold_time,
            move_key,
            open_plugins_folder,
            open_plugin_folder,
            remove_plugin,
            show_plugin,
            key_previews,
            get_layout,
            page_overview,
            set_theme,
            open_link,
            hide_plugin,
            undo,
            layout_op,
            set_start_with_windows,
            set_ring_lights,
            recent_apps,
            list_vsd_profiles,
            import_vsd_profiles,
            save_password,
            password_saved,
            forget_password,
            set_key_settings,
            set_key_look,
            set_key_hold,
            set_screen_saver,
            preview_screen_saver,
            set_intro,
            intro_video,
            audio_outputs,
            audio_apps,
            app_icon,
            installed_apps,
            snippet_list,
            pick_snippet
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
            let idle_app = app.handle().clone();
            std::thread::Builder::new()
                .name("agora-idle".into())
                .spawn(move || watch_idle(idle_app))?;
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
