//! The host's state and the SDK event routing. One task owns all state;
//! sockets, plugin processes and the app send it messages.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use crate::face::{self, Face, Overlay};
use crate::launch::{self, Running};
use crate::manifest::{self, Kind, Plugin};
use crate::{server, Config};

/// The dock as plugins see it.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceDesc {
    pub id: String,
    pub name: String,
    pub rows: usize,
    pub columns: usize,
}

/// Things the app should do or show.
#[derive(Debug, Clone)]
pub enum HostEvent {
    /// Redraw a key.
    Face { key: usize, face: Face },
    /// Open a web page in the browser (http/https only; checked by the host).
    OpenUrl(String),
    /// Start an HTML plugin in a hidden browser view at this address.
    StartHtmlPlugin { plugin: String, url: String },
    StopHtmlPlugin { plugin: String },
    /// A plugin started, stopped, crashed or couldn't start.
    PluginStatus { plugin: String, status: String },
    /// Plugin output (logMessage, stdout, stderr).
    Log { plugin: String, message: String },
}

/// One action in the action list.
#[derive(Debug, Clone, Serialize)]
pub struct ActionInfo {
    pub plugin: String,
    pub plugin_name: String,
    pub category: String,
    pub action: String,
    pub name: String,
    pub tooltip: String,
    /// The action list icon as a data URI, if the plugin has one.
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginInfo {
    pub uuid: String,
    pub name: String,
    pub version: String,
    pub source: String,
    pub kind: String,
    pub status: String,
}

/// What's on a key, for the editor.
#[derive(Debug, Clone, Serialize)]
pub struct KeyInfo {
    pub key: usize,
    pub plugin: String,
    pub plugin_name: String,
    pub action: String,
    pub name: String,
    /// Settings panel address, if the action has one.
    pub settings_url: Option<String>,
}

/// One action placed on a key. Saved in the layout file.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Instance {
    context: String,
    plugin: String,
    action: String,
    #[serde(default = "empty_object")]
    settings: Value,
    #[serde(default)]
    state: usize,
    /// Per-state images and titles set by the plugin. Not saved: plugins
    /// redraw on willAppear.
    #[serde(skip)]
    images: HashMap<usize, String>,
    #[serde(skip)]
    titles: HashMap<usize, String>,
}

fn empty_object() -> Value {
    json!({})
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Layout {
    #[serde(default)]
    keys: BTreeMap<usize, Instance>,
    #[serde(default)]
    global_settings: HashMap<String, Value>,
}

pub(crate) type ConnId = u64;

pub(crate) enum Msg {
    SocketOpen(ConnId, mpsc::UnboundedSender<String>),
    SocketText(ConnId, String),
    SocketClosed(ConnId),
    PluginExited(String, String),
    Restart(String),
    Place(usize, String, String, oneshot::Sender<Result<(), String>>),
    Clear(usize),
    Move(usize, usize),
    Key(usize, bool),
    DeviceConnected(bool),
    Actions(oneshot::Sender<Vec<ActionInfo>>),
    Plugins(oneshot::Sender<Vec<PluginInfo>>),
    KeyInfo(usize, oneshot::Sender<Option<KeyInfo>>),
    /// Redraw keys whose face changed; `true` sends every face again.
    AllFaces(bool),
    Shutdown(oneshot::Sender<()>),
}

/// The app's handle on a running host. Methods block briefly; call them from
/// normal threads, not from inside an async task.
pub struct HostHandle {
    tx: mpsc::UnboundedSender<Msg>,
    pub port: u16,
    runtime: Option<tokio::runtime::Runtime>,
}

impl HostHandle {
    /// Finds plugins, starts the local server and the host task. Events go to `events`.
    pub fn start(config: Config, events: std::sync::mpsc::Sender<HostEvent>) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("agora-host")
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let (plugins, problems) = manifest::discover(&config.plugin_dirs);
        for p in problems {
            let _ = events.send(HostEvent::Log { plugin: "agora".into(), message: format!("Skipped plugin: {p}") });
        }
        let plugin_dirs: HashMap<String, PathBuf> = plugins.iter().map(|p| (p.uuid.clone(), p.dir.clone())).collect();
        let (tx, rx) = mpsc::unbounded_channel();
        let port = runtime.block_on(server::bind(Arc::new(plugin_dirs), tx.clone()))?;
        let layout = load_layout(&config.data_dir);
        let state = State {
            config,
            port,
            plugins: plugins.into_iter().map(|p| (p.uuid.clone(), p)).collect(),
            layout,
            conns: HashMap::new(),
            plugin_conn: HashMap::new(),
            pi_conn: HashMap::new(),
            running: HashMap::new(),
            status: HashMap::new(),
            restarts: HashMap::new(),
            device_connected: false,
            dirty: false,
            events,
            tx: tx.clone(),
            image_cache: HashMap::new(),
            shown: HashMap::new(),
        };
        runtime.spawn(state.run(rx));
        tx.send(Msg::AllFaces(true)).ok();
        Ok(Self { tx, port, runtime: Some(runtime) })
    }

    fn ask<T>(&self, make: impl FnOnce(oneshot::Sender<T>) -> Msg) -> Option<T> {
        let (otx, orx) = oneshot::channel();
        self.tx.send(make(otx)).ok()?;
        orx.blocking_recv().ok()
    }

    pub fn place(&self, key: usize, plugin: &str, action: &str) -> Result<(), String> {
        self.ask(|o| Msg::Place(key, plugin.into(), action.into(), o)).unwrap_or(Err("host stopped".into()))
    }

    pub fn clear(&self, key: usize) {
        let _ = self.tx.send(Msg::Clear(key));
    }

    /// Moves the action on `from` to `to`. Whatever was on `to` swaps to `from`.
    pub fn move_key(&self, from: usize, to: usize) {
        let _ = self.tx.send(Msg::Move(from, to));
    }

    pub fn key(&self, key: usize, pressed: bool) {
        let _ = self.tx.send(Msg::Key(key, pressed));
    }

    pub fn device_connected(&self, connected: bool) {
        let _ = self.tx.send(Msg::DeviceConnected(connected));
    }

    /// Asks the host to send every key's face again (e.g. after a reconnect).
    pub fn redraw_all(&self) {
        let _ = self.tx.send(Msg::AllFaces(true));
    }

    pub fn actions(&self) -> Vec<ActionInfo> {
        self.ask(Msg::Actions).unwrap_or_default()
    }

    pub fn plugins(&self) -> Vec<PluginInfo> {
        self.ask(Msg::Plugins).unwrap_or_default()
    }

    pub fn key_info(&self, key: usize) -> Option<KeyInfo> {
        self.ask(|o| Msg::KeyInfo(key, o)).flatten()
    }

    /// Stops every plugin and saves the layout.
    pub fn shutdown(&mut self) {
        let _ = self.ask(Msg::Shutdown);
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_timeout(Duration::from_secs(2));
        }
    }
}

impl Drop for HostHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

enum Role {
    New,
    Plugin(String),
    Pi(String),
}

struct Conn {
    tx: mpsc::UnboundedSender<String>,
    role: Role,
}

struct State {
    config: Config,
    port: u16,
    plugins: BTreeMap<String, Plugin>,
    layout: Layout,
    conns: HashMap<ConnId, Conn>,
    plugin_conn: HashMap<String, ConnId>,
    pi_conn: HashMap<String, ConnId>,
    running: HashMap<String, Running>,
    status: HashMap<String, String>,
    restarts: HashMap<String, Vec<Instant>>,
    device_connected: bool,
    dirty: bool,
    events: std::sync::mpsc::Sender<HostEvent>,
    tx: mpsc::UnboundedSender<Msg>,
    image_cache: HashMap<(String, String), Option<String>>,
    /// Last face sent per key, so repeats don't go to the device again.
    shown: HashMap<usize, Face>,
}

fn layout_path(dir: &std::path::Path) -> PathBuf {
    dir.join("layout.json")
}

fn load_layout(dir: &std::path::Path) -> Layout {
    std::fs::read_to_string(layout_path(dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn new_context() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    format!("{:016X}{:08X}", t as u64, N.fetch_add(1, Ordering::Relaxed) as u32 ^ std::process::id())
}

impl State {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Msg>) {
        // Start every plugin that has a key placed.
        let needed: Vec<String> = self.layout.keys.values().map(|i| i.plugin.clone()).collect();
        for p in needed {
            self.ensure_running(&p);
        }
        let mut save_tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    let Some(msg) = msg else { break };
                    if let Msg::Shutdown(done) = msg {
                        self.stop_all();
                        self.save();
                        let _ = done.send(());
                        break;
                    }
                    self.handle(msg);
                }
                _ = save_tick.tick() => {
                    if self.dirty {
                        self.save();
                    }
                }
            }
        }
    }

    fn emit(&self, ev: HostEvent) {
        let _ = self.events.send(ev);
    }

    fn log(&self, plugin: &str, message: impl Into<String>) {
        self.emit(HostEvent::Log { plugin: plugin.into(), message: message.into() });
    }

    fn set_status(&mut self, plugin: &str, status: &str) {
        self.status.insert(plugin.into(), status.into());
        self.emit(HostEvent::PluginStatus { plugin: plugin.into(), status: status.into() });
    }

    fn save(&mut self) {
        self.dirty = false;
        let dir = &self.config.data_dir;
        let _ = std::fs::create_dir_all(dir);
        let Ok(text) = serde_json::to_string_pretty(&self.layout) else { return };
        // Write to a temp file, keep the previous file as a backup, then swap.
        let path = layout_path(dir);
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            if path.exists() {
                let _ = std::fs::copy(&path, path.with_extension("json.bak"));
            }
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::SocketOpen(id, tx) => {
                self.conns.insert(id, Conn { tx, role: Role::New });
            }
            Msg::SocketText(id, text) => self.on_text(id, &text),
            Msg::SocketClosed(id) => self.on_closed(id),
            Msg::PluginExited(uuid, why) => self.on_exited(&uuid, why),
            Msg::Restart(uuid) => {
                if self.layout.keys.values().any(|i| i.plugin == uuid) {
                    self.ensure_running(&uuid);
                }
            }
            Msg::Place(key, plugin, action, done) => {
                let _ = done.send(self.place(key, &plugin, &action));
            }
            Msg::Clear(key) => self.clear(key),
            Msg::Move(from, to) => self.move_key(from, to),
            Msg::Key(key, pressed) => self.on_key(key, pressed),
            Msg::DeviceConnected(c) => self.on_device(c),
            Msg::Actions(done) => {
                let _ = done.send(self.action_list());
            }
            Msg::Plugins(done) => {
                let _ = done.send(self.plugin_list());
            }
            Msg::KeyInfo(key, done) => {
                let _ = done.send(self.key_info(key));
            }
            Msg::AllFaces(force) => {
                if force {
                    self.shown.clear();
                }
                let keys: Vec<usize> = self.layout.keys.keys().copied().collect();
                for k in keys {
                    self.redraw(k);
                }
            }
            Msg::Shutdown(_) => unreachable!("handled in run"),
        }
    }

    // ---- sending -------------------------------------------------------

    fn send_conn(&self, id: ConnId, msg: &Value) {
        if let Some(c) = self.conns.get(&id) {
            let _ = c.tx.send(msg.to_string());
        }
    }

    fn to_plugin(&self, plugin: &str, msg: Value) {
        if let Some(&id) = self.plugin_conn.get(plugin) {
            self.send_conn(id, &msg);
        }
    }

    fn to_pi(&self, context: &str, msg: Value) {
        if let Some(&id) = self.pi_conn.get(context) {
            self.send_conn(id, &msg);
        }
    }

    fn coords(&self, key: usize) -> Value {
        let cols = self.config.device.columns.max(1);
        json!({ "column": key % cols, "row": key / cols })
    }

    fn key_of(&self, context: &str) -> Option<usize> {
        self.layout.keys.iter().find(|(_, i)| i.context == context).map(|(k, _)| *k)
    }

    fn instance_event(&self, key: usize, event: &str, extra: Value) -> Option<(String, Value)> {
        let inst = self.layout.keys.get(&key)?;
        let mut payload = json!({
            "settings": inst.settings,
            "coordinates": self.coords(key),
            "state": inst.state,
            "isInMultiAction": false,
        });
        if let (Some(p), Some(e)) = (payload.as_object_mut(), extra.as_object()) {
            p.extend(e.clone());
        }
        let msg = json!({
            "action": inst.action,
            "event": event,
            "context": inst.context,
            "device": self.config.device.id,
            "payload": payload,
        });
        Some((inst.plugin.clone(), msg))
    }

    fn send_instance(&self, key: usize, event: &str, extra: Value) {
        if let Some((plugin, msg)) = self.instance_event(key, event, extra) {
            self.to_plugin(&plugin, msg);
        }
    }

    fn device_info(&self) -> Value {
        json!({
            "id": self.config.device.id,
            "name": self.config.device.name,
            "size": { "columns": self.config.device.columns, "rows": self.config.device.rows },
            "type": 0,
        })
    }

    pub(crate) fn info_json(&self, plugin: &str) -> String {
        let version = self.plugins.get(plugin).map(|p| p.manifest.version.clone()).unwrap_or_default();
        json!({
            "application": {
                "font": if cfg!(windows) { "Segoe UI" } else { "sans-serif" },
                "language": "en",
                "platform": if cfg!(windows) { "windows" } else if cfg!(target_os = "macos") { "mac" } else { "linux" },
                "platformVersion": "10",
                // The Stream Dock version Agora is compatible with, so plugins
                // that check a minimum version run.
                "version": "3.10.188.226",
                "name": "Agora",
            },
            "colors": {
                "buttonMouseOverBackgroundColor": "#464646FF",
                "buttonPressedBackgroundColor": "#303030FF",
                "buttonPressedBorderColor": "#646464FF",
                "buttonPressedTextColor": "#969696FF",
                "highlightColor": "#0078FFFF",
            },
            "devicePixelRatio": 1,
            "devices": if self.device_connected { json!([self.device_info()]) } else { json!([]) },
            "plugin": { "uuid": plugin, "version": version },
        })
        .to_string()
    }

    // ---- faces ---------------------------------------------------------

    fn manifest_image(&mut self, plugin: &str, rel: &str) -> Option<String> {
        let key = (plugin.to_string(), rel.to_string());
        if let Some(v) = self.image_cache.get(&key) {
            return v.clone();
        }
        let v = self.plugins.get(plugin).and_then(|p| face::file_data_uri(&p.dir, rel));
        self.image_cache.insert(key, v.clone());
        v
    }

    fn face_for(&mut self, key: usize) -> Face {
        let Some(inst) = self.layout.keys.get(&key).cloned() else { return Face::default() };
        let state_def = self
            .plugins
            .get(&inst.plugin)
            .and_then(|p| p.action(&inst.action))
            .and_then(|a| a.states.get(inst.state).or(a.states.first()))
            .cloned()
            .unwrap_or_default();
        let image = match inst.images.get(&inst.state) {
            Some(img) => Some(img.clone()),
            None => self.manifest_image(&inst.plugin, &state_def.image),
        };
        let font_size = match &state_def.font_size {
            Some(Value::String(s)) => s.parse().unwrap_or(9.0),
            Some(Value::Number(n)) => n.as_f64().unwrap_or(9.0) as f32,
            _ => 9.0,
        };
        Face {
            image,
            title: inst.titles.get(&inst.state).cloned().unwrap_or(state_def.title.clone()),
            show_title: state_def.show_title.unwrap_or(true),
            align: state_def.title_alignment.clone().unwrap_or_else(|| "bottom".into()),
            font_size,
            color: state_def.title_color.clone().unwrap_or_else(|| "#ffffff".into()),
            overlay: Overlay::None,
        }
    }

    fn redraw(&mut self, key: usize) {
        let face = self.face_for(key);
        self.show(key, face);
    }

    fn show(&mut self, key: usize, face: Face) {
        if self.shown.get(&key) != Some(&face) {
            self.shown.insert(key, face.clone());
            self.emit(HostEvent::Face { key, face });
        }
    }

    fn flash(&mut self, key: usize, overlay: Overlay) {
        let mut face = self.face_for(key);
        face.overlay = overlay;
        self.show(key, face);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1200)).await;
            let _ = tx.send(Msg::AllFaces(false));
        });
    }

    // ---- plugin processes ------------------------------------------------

    fn ensure_running(&mut self, uuid: &str) {
        if self.running.contains_key(uuid) || self.plugin_conn.contains_key(uuid) {
            return;
        }
        let Some(plugin) = self.plugins.get(uuid).cloned() else {
            self.log(uuid, "Plugin not installed");
            return;
        };
        let info = self.info_json(uuid);
        match plugin.kind() {
            Some(Kind::Html(path)) => {
                let rel = path.strip_prefix(&plugin.dir).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                let url = server::connect_url(self.port, uuid, &rel, uuid, "registerPlugin", &info, None);
                self.running.insert(uuid.into(), Running::Html);
                self.emit(HostEvent::StartHtmlPlugin { plugin: uuid.into(), url });
                self.set_status(uuid, "starting");
            }
            Some(kind) => {
                let logs = self.config.data_dir.join("logs");
                match launch::spawn(&plugin, &kind, self.config.node.as_deref(), self.port, &info, &logs, self.tx.clone(), self.events.clone()) {
                    Ok(r) => {
                        self.running.insert(uuid.into(), r);
                        self.set_status(uuid, "starting");
                    }
                    Err(e) => {
                        self.log(uuid, format!("Could not start: {e}"));
                        self.set_status(uuid, "could not start");
                    }
                }
            }
            None => {
                self.log(uuid, "Manifest has no code path for this system");
                self.set_status(uuid, "not supported here");
            }
        }
    }

    fn stop_plugin(&mut self, uuid: &str) {
        if let Some(r) = self.running.remove(uuid) {
            match r {
                Running::Html => self.emit(HostEvent::StopHtmlPlugin { plugin: uuid.into() }),
                Running::Process(kill) => {
                    let _ = kill.send(());
                }
            }
        }
        self.set_status(uuid, "stopped");
    }

    fn stop_all(&mut self) {
        let ids: Vec<String> = self.running.keys().cloned().collect();
        for id in ids {
            self.stop_plugin(&id);
        }
    }

    fn on_exited(&mut self, uuid: &str, why: String) {
        if self.running.remove(uuid).is_none() {
            return; // stopped on purpose
        }
        self.log(uuid, format!("Plugin exited: {why}"));
        let now = Instant::now();
        let recent = self.restarts.entry(uuid.into()).or_default();
        recent.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
        recent.push(now);
        if recent.len() > 5 {
            self.set_status(uuid, "crashed (gave up after 5 restarts in a minute)");
            return;
        }
        let delay = Duration::from_secs(1 << (recent.len() - 1).min(4));
        self.set_status(uuid, "crashed, restarting");
        let tx = self.tx.clone();
        let id = uuid.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = tx.send(Msg::Restart(id));
        });
    }

    // ---- app requests ----------------------------------------------------

    fn place(&mut self, key: usize, plugin: &str, action: &str) -> Result<(), String> {
        let cols = self.config.device.columns;
        if key >= self.config.device.rows * cols {
            return Err(format!("no key {key}"));
        }
        let def = self
            .plugins
            .get(plugin)
            .and_then(|p| p.action(action))
            .ok_or_else(|| format!("{plugin} has no action {action}"))?;
        let settings = def.settings.clone().unwrap_or_else(empty_object);
        self.clear(key);
        self.layout.keys.insert(
            key,
            Instance {
                context: new_context(),
                plugin: plugin.into(),
                action: action.into(),
                settings,
                state: 0,
                images: HashMap::new(),
                titles: HashMap::new(),
            },
        );
        self.dirty = true;
        self.redraw(key);
        if self.plugin_conn.contains_key(plugin) {
            self.appear(key);
        } else {
            self.ensure_running(plugin);
        }
        Ok(())
    }

    fn clear(&mut self, key: usize) {
        if !self.layout.keys.contains_key(&key) {
            return;
        }
        self.send_instance(key, "willDisappear", json!({ "controller": "Keypad" }));
        let inst = self.layout.keys.remove(&key).expect("checked");
        self.dirty = true;
        self.show(key, Face::default());
        if !self.layout.keys.values().any(|i| i.plugin == inst.plugin) {
            self.stop_plugin(&inst.plugin);
        }
    }

    fn move_key(&mut self, from: usize, to: usize) {
        let count = self.config.device.rows * self.config.device.columns;
        if from == to || from >= count || to >= count || !self.layout.keys.contains_key(&from) {
            return;
        }
        // Plugins see the action leave its old spot and appear at the new one;
        // the context stays the same, so settings and open panels carry over.
        let moved: Vec<usize> = [from, to].into_iter().filter(|k| self.layout.keys.contains_key(k)).collect();
        for &k in &moved {
            self.send_instance(k, "willDisappear", json!({ "controller": "Keypad" }));
        }
        let a = self.layout.keys.remove(&from);
        let b = self.layout.keys.remove(&to);
        if let Some(a) = a {
            self.layout.keys.insert(to, a);
        }
        if let Some(b) = b {
            self.layout.keys.insert(from, b);
        }
        self.dirty = true;
        for k in [from, to] {
            self.redraw(k);
            let connected = self.layout.keys.get(&k).is_some_and(|i| self.plugin_conn.contains_key(&i.plugin));
            if connected {
                self.appear(k);
            }
        }
    }

    fn appear(&mut self, key: usize) {
        self.send_instance(key, "willAppear", json!({ "controller": "Keypad" }));
        let face = self.face_for(key);
        self.send_instance(
            key,
            "titleParametersDidChange",
            json!({
                "title": face.title,
                "titleParameters": {
                    "fontFamily": "", "fontSize": face.font_size, "fontStyle": "", "fontUnderline": false,
                    "showTitle": face.show_title, "titleAlignment": face.align, "titleColor": face.color,
                },
            }),
        );
    }

    fn on_key(&mut self, key: usize, pressed: bool) {
        if self.layout.keys.contains_key(&key) {
            self.send_instance(key, if pressed { "keyDown" } else { "keyUp" }, json!({}));
        }
    }

    fn on_device(&mut self, connected: bool) {
        self.device_connected = connected;
        let ev = if connected {
            json!({ "event": "deviceDidConnect", "device": self.config.device.id, "deviceInfo": self.device_info() })
        } else {
            json!({ "event": "deviceDidDisconnect", "device": self.config.device.id })
        };
        let plugins: Vec<String> = self.plugin_conn.keys().cloned().collect();
        for p in plugins {
            self.to_plugin(&p, ev.clone());
        }
    }

    fn action_list(&mut self) -> Vec<ActionInfo> {
        let mut out = Vec::new();
        let plugins: Vec<Plugin> = self.plugins.values().cloned().collect();
        for p in plugins {
            for a in p.manifest.actions.iter().filter(|a| a.visible_in_actions_list) {
                let keypad = a.controllers.is_empty() || a.controllers.iter().any(|c| c == "Keypad");
                if !keypad {
                    continue;
                }
                out.push(ActionInfo {
                    plugin: p.uuid.clone(),
                    plugin_name: p.manifest.name.clone(),
                    category: p.manifest.category.clone().unwrap_or_else(|| p.manifest.name.clone()),
                    action: a.uuid.clone(),
                    name: a.name.clone(),
                    tooltip: a.tooltip.clone(),
                    icon: self.manifest_image(&p.uuid, &a.icon),
                });
            }
        }
        out
    }

    fn plugin_list(&self) -> Vec<PluginInfo> {
        self.plugins
            .values()
            .map(|p| PluginInfo {
                uuid: p.uuid.clone(),
                name: p.manifest.name.clone(),
                version: p.manifest.version.clone(),
                source: match p.source {
                    manifest::Source::Agora => "Agora".into(),
                    manifest::Source::VsdCraft => "VSD Craft".into(),
                },
                kind: match p.kind() {
                    Some(Kind::Node(_)) => "Node.js".into(),
                    Some(Kind::Html(_)) => "HTML".into(),
                    Some(Kind::Exe(_)) => "program".into(),
                    None => "unsupported".into(),
                },
                status: self.status.get(&p.uuid).cloned().unwrap_or_else(|| "not running".into()),
            })
            .collect()
    }

    fn key_info(&self, key: usize) -> Option<KeyInfo> {
        let inst = self.layout.keys.get(&key)?;
        let plugin = self.plugins.get(&inst.plugin)?;
        let def = plugin.action(&inst.action)?;
        let settings_url = plugin.property_inspector(&inst.action).map(|rel| {
            let action_info = json!({
                "action": inst.action,
                "context": inst.context,
                "device": self.config.device.id,
                "payload": { "settings": inst.settings, "coordinates": self.coords(key), "controller": "Keypad", "state": inst.state },
            })
            .to_string();
            server::connect_url(
                self.port,
                &inst.plugin,
                &rel,
                &inst.context,
                "registerPropertyInspector",
                &self.info_json(&inst.plugin),
                Some(&action_info),
            )
        });
        Some(KeyInfo {
            key,
            plugin: inst.plugin.clone(),
            plugin_name: plugin.manifest.name.clone(),
            action: inst.action.clone(),
            name: def.name.clone(),
            settings_url,
        })
    }

    // ---- sockets ---------------------------------------------------------

    fn on_closed(&mut self, id: ConnId) {
        let Some(conn) = self.conns.remove(&id) else { return };
        match conn.role {
            Role::Plugin(uuid) => {
                if self.plugin_conn.get(&uuid) == Some(&id) {
                    self.plugin_conn.remove(&uuid);
                    if matches!(self.running.get(&uuid), Some(Running::Html)) {
                        self.set_status(&uuid, "disconnected");
                    }
                }
            }
            Role::Pi(context) => {
                if self.pi_conn.get(&context) == Some(&id) {
                    self.pi_conn.remove(&context);
                    if let Some(key) = self.key_of(&context) {
                        self.send_instance(key, "propertyInspectorDidDisappear", json!({}));
                    }
                }
            }
            Role::New => {}
        }
    }

    fn on_text(&mut self, id: ConnId, text: &str) {
        let Ok(msg) = serde_json::from_str::<Value>(text) else { return };
        let event = msg.get("event").and_then(Value::as_str).unwrap_or("").to_string();
        let role = match self.conns.get(&id) {
            Some(c) => match &c.role {
                Role::New => None,
                Role::Plugin(u) => Some(Role::Plugin(u.clone())),
                Role::Pi(c) => Some(Role::Pi(c.clone())),
            },
            None => return,
        };
        match role {
            None => self.on_register(id, &event, &msg),
            Some(Role::Plugin(uuid)) => self.handle_plugin(&uuid, &event, &msg),
            Some(Role::Pi(context)) => self.handle_pi(&context, &event, &msg),
            Some(Role::New) => unreachable!(),
        }
    }

    fn on_register(&mut self, id: ConnId, event: &str, msg: &Value) {
        let uuid = msg.get("uuid").and_then(Value::as_str).unwrap_or("").to_string();
        match event {
            "registerPlugin" if self.plugins.contains_key(&uuid) => {
                if let Some(c) = self.conns.get_mut(&id) {
                    c.role = Role::Plugin(uuid.clone());
                }
                self.plugin_conn.insert(uuid.clone(), id);
                self.set_status(&uuid, "running");
                if self.device_connected {
                    let ev = json!({ "event": "deviceDidConnect", "device": self.config.device.id, "deviceInfo": self.device_info() });
                    self.send_conn(id, &ev);
                }
                let keys: Vec<usize> =
                    self.layout.keys.iter().filter(|(_, i)| i.plugin == uuid).map(|(k, _)| *k).collect();
                for k in keys {
                    self.appear(k);
                }
            }
            "registerPropertyInspector" => {
                let Some(key) = self.key_of(&uuid) else { return };
                if let Some(c) = self.conns.get_mut(&id) {
                    c.role = Role::Pi(uuid.clone());
                }
                self.pi_conn.insert(uuid.clone(), id);
                self.send_instance(key, "propertyInspectorDidAppear", json!({}));
            }
            _ => {}
        }
    }

    /// Settings for a context, as both sides expect them in didReceiveSettings.
    fn settings_event(&self, key: usize) -> Option<Value> {
        self.instance_event(key, "didReceiveSettings", json!({})).map(|(_, m)| m)
    }

    fn handle_plugin(&mut self, uuid: &str, event: &str, msg: &Value) {
        let context = msg.get("context").and_then(Value::as_str).unwrap_or("");
        let payload = msg.get("payload").cloned().unwrap_or(Value::Null);
        // Only let a plugin touch its own keys.
        let key = self.key_of(context).filter(|k| self.layout.keys.get(k).is_some_and(|i| i.plugin == uuid));
        match (event, key) {
            ("setTitle", Some(k)) => {
                let title = payload.get("title").and_then(Value::as_str).map(str::to_string);
                self.per_state(k, &payload, |inst, s| match &title {
                    Some(t) => {
                        inst.titles.insert(s, t.clone());
                    }
                    None => {
                        inst.titles.remove(&s);
                    }
                });
                self.redraw(k);
            }
            ("setImage", Some(k)) => {
                let image = payload.get("image").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
                self.per_state(k, &payload, |inst, s| match &image {
                    Some(i) => {
                        inst.images.insert(s, i.clone());
                    }
                    None => {
                        inst.images.remove(&s);
                    }
                });
                self.redraw(k);
            }
            ("setState", Some(k)) => {
                let s = payload.get("state").and_then(Value::as_u64).unwrap_or(0) as usize;
                if let Some(inst) = self.layout.keys.get_mut(&k) {
                    inst.state = s;
                    self.dirty = true;
                }
                self.redraw(k);
            }
            ("showOk", Some(k)) => self.flash(k, Overlay::Ok),
            ("showAlert", Some(k)) => self.flash(k, Overlay::Alert),
            ("setSettings", Some(k)) => {
                if let Some(inst) = self.layout.keys.get_mut(&k) {
                    inst.settings = payload;
                    self.dirty = true;
                }
                // Stream Dock tells the other side (the settings panel).
                if let Some(ev) = self.settings_event(k) {
                    self.to_pi(context, ev);
                }
            }
            ("getSettings", Some(k)) => {
                if let Some(ev) = self.settings_event(k) {
                    self.to_plugin(uuid, ev);
                }
            }
            ("setGlobalSettings", _) => {
                self.layout.global_settings.insert(uuid.into(), payload.clone());
                self.dirty = true;
                let ev = json!({ "event": "didReceiveGlobalSettings", "payload": { "settings": payload } });
                let pis: Vec<String> = self
                    .layout
                    .keys
                    .values()
                    .filter(|i| i.plugin == uuid)
                    .map(|i| i.context.clone())
                    .collect();
                for c in pis {
                    self.to_pi(&c, ev.clone());
                }
            }
            ("getGlobalSettings", _) => {
                let settings = self.layout.global_settings.get(uuid).cloned().unwrap_or_else(empty_object);
                self.to_plugin(uuid, json!({ "event": "didReceiveGlobalSettings", "payload": { "settings": settings } }));
            }
            ("sendToPropertyInspector", Some(_)) => {
                let action = msg.get("action").cloned().unwrap_or(Value::Null);
                self.to_pi(
                    context,
                    json!({ "action": action, "event": "sendToPropertyInspector", "context": context, "payload": payload }),
                );
            }
            ("openUrl", _) => self.open_url(uuid, &payload),
            ("logMessage", _) => {
                let m = payload.get("message").and_then(Value::as_str).unwrap_or("");
                self.log(uuid, m.chars().take(2000).collect::<String>());
            }
            // Elgato-only extras: accepted quietly so plugins that use them keep working.
            ("setFeedback" | "setFeedbackLayout" | "setTriggerDescription" | "switchToProfile" | "showAlertOnEncoder", _) => {}
            (other, _) => self.log(uuid, format!("Ignored event '{other}'")),
        }
    }

    fn handle_pi(&mut self, context: &str, event: &str, msg: &Value) {
        let Some(key) = self.key_of(context) else { return };
        let plugin = self.layout.keys[&key].plugin.clone();
        let payload = msg.get("payload").cloned().unwrap_or(Value::Null);
        match event {
            "setSettings" => {
                if let Some(inst) = self.layout.keys.get_mut(&key) {
                    inst.settings = payload;
                    self.dirty = true;
                }
                if let Some(ev) = self.settings_event(key) {
                    self.to_plugin(&plugin, ev);
                }
            }
            "getSettings" => {
                if let Some(ev) = self.settings_event(key) {
                    self.to_pi(context, ev);
                }
            }
            "setGlobalSettings" => {
                self.layout.global_settings.insert(plugin.clone(), payload.clone());
                self.dirty = true;
                self.to_plugin(&plugin, json!({ "event": "didReceiveGlobalSettings", "payload": { "settings": payload } }));
            }
            "getGlobalSettings" => {
                let settings = self.layout.global_settings.get(&plugin).cloned().unwrap_or_else(empty_object);
                self.to_pi(context, json!({ "event": "didReceiveGlobalSettings", "payload": { "settings": settings } }));
            }
            "sendToPlugin" => {
                let action = self.layout.keys[&key].action.clone();
                self.to_plugin(
                    &plugin,
                    json!({ "action": action, "event": "sendToPlugin", "context": context, "payload": payload }),
                );
            }
            "openUrl" => self.open_url(&plugin, &payload),
            "logMessage" => {
                let m = payload.get("message").and_then(Value::as_str).unwrap_or("");
                self.log(&plugin, format!("(settings) {}", m.chars().take(2000).collect::<String>()));
            }
            _ => {}
        }
    }

    fn per_state(&mut self, key: usize, payload: &Value, mut f: impl FnMut(&mut Instance, usize)) {
        let n_states = self
            .layout
            .keys
            .get(&key)
            .and_then(|i| self.plugins.get(&i.plugin).and_then(|p| p.action(&i.action)))
            .map_or(1, |a| a.states.len().max(1));
        let Some(inst) = self.layout.keys.get_mut(&key) else { return };
        match payload.get("state").and_then(Value::as_u64) {
            Some(s) => f(inst, s as usize),
            None => {
                for s in 0..n_states {
                    f(inst, s);
                }
            }
        }
    }

    fn open_url(&self, plugin: &str, payload: &Value) {
        let url = payload.get("url").and_then(Value::as_str).unwrap_or("");
        if url.starts_with("https://") || url.starts_with("http://") {
            self.emit(HostEvent::OpenUrl(url.into()));
        } else {
            self.log(plugin, format!("Refused to open non-web address: {}", url.chars().take(200).collect::<String>()));
        }
    }
}
