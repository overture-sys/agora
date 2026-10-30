// Agora app shell: tray icon, dock connection with reconnect, and the plugin
// host. Keys show what their plugins draw; presses go to the plugins.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agora_device::keyimage::{self, Orientation};
use agora_device::worker::{Command, DockWorker, Event};
use agora_device::{discover, Button, Control, Rgb};
use agora_host::{face, manifest, ActionInfo, Config, DeviceDesc, HostEvent, HostHandle, KeyInfo, PluginInfo, Source};
use base64::Engine;
use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// M18 key image size, used for previews before a dock has connected.
const KEY_SIZE: u32 = 64;

/// The connected dock, if any. The connection thread fills and clears it.
#[derive(Default)]
struct Dock {
    worker: Option<DockWorker>,
    model: Option<String>,
    orientation: Orientation,
    /// The last connection message, for a window that opens later.
    message: String,
}

type Shared = Arc<Mutex<Dock>>;

/// The plugin host. Taken out and dropped on quit so plugins are stopped.
type Host = Arc<Mutex<Option<HostHandle>>>;

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

fn send(state: &State<Shared>, cmd: Command) -> Result<(), String> {
    let dock = state.lock().unwrap();
    match &dock.worker {
        Some(w) if w.send(cmd) => Ok(()),
        _ => Err("No dock connected".into()),
    }
}

fn with_host<T>(host: &State<Host>, f: impl FnOnce(&HostHandle) -> T) -> Result<T, String> {
    let guard = host.lock().unwrap();
    guard.as_ref().map(f).ok_or_else(|| "The plugin host isn't running".to_string())
}

#[tauri::command]
fn dock_status(state: State<Shared>) -> Status {
    let dock = state.lock().unwrap();
    status(&dock, dock.message.clone())
}

#[tauri::command]
fn set_brightness(state: State<Shared>, percent: u8) -> Result<(), String> {
    send(&state, Command::Brightness(percent))
}

/// `color` is "#rrggbb". Lights the whole ring.
#[tauri::command]
fn set_ring(state: State<Shared>, color: String) -> Result<(), String> {
    let hex = color.trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    send(&state, Command::Ring(vec![Rgb(byte(0), byte(2), byte(4)); 24]))
}

#[tauri::command]
fn ring_off(state: State<Shared>) -> Result<(), String> {
    send(&state, Command::RingReset)
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
fn place_action(host: State<Host>, key: usize, plugin: String, action: String) -> Result<(), String> {
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
fn key_previews(previews: State<Previews>) -> Vec<Preview> {
    let map = previews.lock().unwrap();
    map.iter().map(|(&key, image)| Preview { key, image: Some(image.clone()) }).collect()
}

/// Agora's data folder: %APPDATA%\Agora on Windows.
fn data_dir(app: &AppHandle) -> PathBuf {
    match std::env::var_os("APPDATA") {
        Some(appdata) if cfg!(windows) => PathBuf::from(appdata).join("Agora"),
        _ => app.path().app_data_dir().unwrap_or_else(|_| PathBuf::from("agora-data")),
    }
}

/// Node.js for Node plugins: AGORA_NODE, then node\node.exe next to Agora,
/// then whatever is on PATH (the host's default).
fn find_node() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("AGORA_NODE").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let bundled = exe_dir.join("node").join(if cfg!(windows) { "node.exe" } else { "node" });
    bundled.is_file().then_some(bundled)
}

fn start_host(app: &AppHandle) -> Result<(HostHandle, Receiver<HostEvent>), String> {
    let data = data_dir(app);
    let own = data.join("plugins");
    let _ = std::fs::create_dir_all(&own);
    let mut plugin_dirs = vec![(own, Source::Agora)];
    if let Some(vsd) = manifest::vsd_craft_plugin_dir() {
        // Read only: plugins installed for VSD Craft, used as they are.
        plugin_dirs.push((vsd, Source::VsdCraft));
    }
    let config = Config {
        plugin_dirs,
        data_dir: data,
        node: find_node(),
        device: DeviceDesc { id: "M18".into(), name: "Stream Dock M18".into(), rows: 3, columns: 5 },
    };
    let (tx, rx) = std::sync::mpsc::channel();
    Ok((HostHandle::start(config, tx)?, rx))
}

fn png_data_uri(img: &image::DynamicImage) -> Option<String> {
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)))
}

/// Opens an http(s) address in the default browser. The host has already
/// checked the scheme.
fn open_url(url: &str) {
    #[cfg(windows)]
    let cmd = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    #[cfg(target_os = "macos")]
    let cmd = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = std::process::Command::new("xdg-open").arg(url).spawn();
    let _ = cmd;
}

fn html_plugin_label(plugin: &str) -> String {
    let safe: String = plugin.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
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
                    (dock.worker.as_ref().map_or(KEY_SIZE, |w| w.layout.key_size), dock.orientation)
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
                        let built = WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(parsed))
                            .title(format!("Agora plugin {plugin}"))
                            .visible(false)
                            .skip_taskbar(true)
                            .build();
                        if let Err(e) = built {
                            let _ = app.emit("plugin-log", PluginLine { plugin, message: format!("Couldn't start: {e}") });
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
                let _ = app.emit("plugin-status", PluginLine { plugin, message: status });
            }
            HostEvent::Log { plugin, message } => {
                let _ = app.emit("plugin-log", PluginLine { plugin, message });
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
                report(&app, &mut dock, format!("{model} disconnected. Looking for it again..."));
            }
            Err(agora_device::Error::Disconnected) => {
                let mut dock = shared.lock().unwrap();
                report(&app, &mut dock, "No dock found. Plug in the M18 and close VSD Craft.".into());
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
                let _ = app.emit("dock-input", Input { kind, index, pressed: i.pressed, raw_id: i.raw_id });
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

    tauri::Builder::default()
        .manage(shared.clone())
        .manage(host.clone())
        .manage(previews.clone())
        .invoke_handler(tauri::generate_handler![
            dock_status,
            set_brightness,
            set_ring,
            ring_off,
            list_actions,
            list_plugins,
            key_info,
            place_action,
            clear_key,
            move_key,
            key_previews
        ])
        .setup(move |app| {
            let show = MenuItem::with_id(app, "show", "Show Agora", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit (give the dock back)", true, None::<&str>)?;
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
            match start_host(&handle) {
                Ok((h, events)) => {
                    *host.lock().unwrap() = Some(h);
                    let (handle, shared, previews) = (handle.clone(), shared.clone(), previews.clone());
                    std::thread::Builder::new()
                        .name("agora-host-events".into())
                        .spawn(move || host_events(handle, shared, previews, events))?;
                }
                Err(e) => eprintln!("plugin host didn't start: {e}"),
            }

            let (handle, shared, host) = (app.handle().clone(), shared.clone(), host.clone());
            std::thread::Builder::new()
                .name("agora-connect".into())
                .spawn(move || connection_loop(handle, shared, host))?;
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
