// Agora app shell, phase 1: tray icon, dock connection with reconnect, and a
// live test page. Profiles and the plugin host come in later phases.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agora_device::keyimage::{self, Orientation};
use agora_device::worker::{Command, DockWorker, Event};
use agora_device::{discover, Button, Control, Rgb};
use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};

/// The connected dock, if any. The connection thread fills and clears it.
#[derive(Default)]
struct Dock {
    worker: Option<DockWorker>,
    model: Option<String>,
    orientation: Orientation,
}

type Shared = Arc<Mutex<Dock>>;

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

#[tauri::command]
fn dock_status(state: State<Shared>) -> Status {
    status(&state.lock().unwrap(), "")
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

/// Shows the orientation test face on every key.
#[tauri::command]
fn show_test_faces(state: State<Shared>) -> Result<(), String> {
    let dock = state.lock().unwrap();
    let w = dock.worker.as_ref().ok_or("No dock connected")?;
    for key in 0..w.layout.key_count() {
        let face = keyimage::orientation_face(w.layout.key_size, key);
        let jpeg = keyimage::to_key_jpeg(&face, w.layout.key_size, dock.orientation).map_err(|e| e.to_string())?;
        w.send(Command::SetKeyJpeg(key, jpeg));
    }
    Ok(())
}

/// Puts up to two short lines of text on one key.
#[tauri::command]
fn set_key_text(state: State<Shared>, key: usize, text: String) -> Result<(), String> {
    let dock = state.lock().unwrap();
    let w = dock.worker.as_ref().ok_or("No dock connected")?;
    let lines: Vec<&str> = text.lines().take(2).collect();
    let face = keyimage::text_face(w.layout.key_size, [22, 30, 56], [232, 184, 64], &lines);
    let jpeg = keyimage::to_key_jpeg(&face, w.layout.key_size, dock.orientation).map_err(|e| e.to_string())?;
    w.send(Command::SetKeyJpeg(key, jpeg));
    Ok(())
}

#[tauri::command]
fn clear_keys(state: State<Shared>) -> Result<(), String> {
    send(&state, Command::ClearAll)
}

/// Keeps a dock connected: finds it, opens it, forwards its events to the
/// UI, and tries again every few seconds after it goes away.
fn connection_loop(app: AppHandle, shared: Shared) {
    let mut last_message = String::new();
    let mut report = |app: &AppHandle, dock: &Dock, msg: String| {
        if msg != last_message {
            let _ = app.emit("dock-status", status(dock, msg.clone()));
            last_message = msg;
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
                    report(&app, &dock, format!("{model} connected"));
                }
                forward_events(&app, events);
                let mut dock = shared.lock().unwrap();
                dock.worker = None;
                report(&app, &dock, format!("{model} disconnected. Looking for it again..."));
            }
            Err(agora_device::Error::Disconnected) => {
                let dock = shared.lock().unwrap();
                report(&app, &dock, "No dock found. Plug in the M18 and close VSD Craft.".into());
            }
            Err(e) => {
                let dock = shared.lock().unwrap();
                report(&app, &dock, e.to_string());
            }
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}

/// Sends dock input to the UI until the worker stops.
fn forward_events(app: &AppHandle, events: std::sync::mpsc::Receiver<Event>) {
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

    tauri::Builder::default()
        .manage(shared.clone())
        .invoke_handler(tauri::generate_handler![
            dock_status,
            set_brightness,
            set_ring,
            ring_off,
            show_test_faces,
            set_key_text,
            clear_keys
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
                        // Dropping the worker hands the dock back to its stock screen.
                        app.state::<Shared>().lock().unwrap().worker = None;
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            let handle = app.handle().clone();
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("agora-connect".into())
                .spawn(move || connection_loop(handle, shared))?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps Agora running in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Agora");
}
