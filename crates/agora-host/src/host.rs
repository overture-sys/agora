//! The host's state and the SDK event routing. One task owns all state;
//! sockets, plugin processes and the app send it messages.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use crate::builtin::{self, PLUGIN as BUILTIN};
use crate::face::{self, Face, Overlay};
use crate::launch::{self, Running};
use crate::manifest::{self, Kind, Plugin};
use crate::stats;
use crate::vsd_import;
use crate::{server, Config};

/// The dock as plugins see it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DeviceDesc {
    pub id: String,
    pub name: String,
    pub rows: usize,
    pub columns: usize,
    /// Buttons without a screen (the M18's round buttons).
    pub buttons: usize,
    /// Knobs (the N3 and N4).
    pub dials: usize,
}

/// Where buttons and knobs sit among a page's keys: screen keys count from
/// 0, buttons from 100 and knobs from 200, so a layout keeps its meaning
/// whatever size of dock it is shown on.
pub const BUTTON_BASE: usize = 100;
pub const DIAL_BASE: usize = 200;

/// What a knob did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialInput {
    /// Turned some steps: below zero to the left.
    Turn(i32),
    Press,
    Release,
    /// A tap on the knob's strip screen (the N4).
    Tap,
}

/// The kind of place a key number is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Key,
    Button,
    Dial,
}

fn slot(key: usize) -> Slot {
    if key >= DIAL_BASE {
        Slot::Dial
    } else if key >= BUTTON_BASE {
        Slot::Button
    } else {
        Slot::Key
    }
}

/// Things the app should do or show.
#[derive(Debug, Clone)]
pub enum HostEvent {
    /// Redraw a key.
    Face {
        key: usize,
        face: Face,
    },
    /// A built-in key was pressed: play its press bounce with this face.
    Pressed {
        key: usize,
        face: Face,
    },
    /// Open a web page in the browser (http/https only; checked by the host).
    OpenUrl(String),
    /// Start an HTML plugin in a hidden browser view at this address.
    StartHtmlPlugin {
        plugin: String,
        url: String,
    },
    StopHtmlPlugin {
        plugin: String,
    },
    /// A plugin started, stopped, crashed or couldn't start.
    PluginStatus {
        plugin: String,
        status: String,
    },
    /// Plugin output (logMessage, stdout, stderr).
    Log {
        plugin: String,
        message: String,
    },
    /// The dock went to another page or profile, or pages changed.
    PageChanged,
    /// Carry out one of Agora's system actions (open, hotkey, media...).
    Run {
        /// The key's action, so a result can be shown on it (see
        /// [`Host::action_result`]).
        context: String,
        action: String,
        settings: Value,
    },
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
    /// The plugin's (category's) icon for its group in the list.
    pub group_icon: Option<String>,
    /// Where it can go: "Keypad" (keys and buttons), "Encoder" (knobs).
    pub controllers: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginInfo {
    pub uuid: String,
    pub name: String,
    pub version: String,
    pub source: String,
    pub kind: String,
    pub status: String,
    /// The plugin's folder.
    pub dir: String,
    /// Hidden from the action list (used for VSD Craft's plugins, which
    /// Agora never deletes).
    pub hidden: bool,
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
    /// One of Agora's own actions; the editor shows its settings itself.
    pub builtin: bool,
    pub settings: Value,
    pub look: Look,
    /// Steps run when the key is held instead of tapped.
    pub hold: Vec<Value>,
}

/// The title and icon the user chose for a key. Anything left empty comes
/// from the action.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Look {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_title: Option<bool>,
    /// "top", "middle" or "bottom".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
    /// An image as a data URI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NameId {
    pub id: String,
    pub name: String,
}

/// Profiles and pages, for the editor.
#[derive(Debug, Clone, Serialize)]
pub struct LayoutView {
    pub profiles: Vec<NameId>,
    pub profile: String,
    /// The active profile's pages (not folders).
    pub pages: Vec<NameId>,
    /// The page showing now, which may be a folder.
    pub page: String,
    /// The profile page that holds the page showing now.
    pub top_page: String,
    /// Folders open from `top_page` down to `page`; empty on a profile page.
    pub path: Vec<NameId>,
    /// The apps that bring the active profile up.
    pub apps: Vec<String>,
}

/// Everything the editor shows for the page in front: profiles and pages,
/// and what's on each key, button and knob. One question instead of one per
/// place, so a page change doesn't queue a burst behind the dock's redraw.
#[derive(Debug, Clone, Serialize)]
pub struct PageOverview {
    pub layout: LayoutView,
    pub keys: Vec<KeyInfo>,
}

/// What a VSD Craft import brought in.
#[derive(Debug, Clone, Serialize)]
pub struct ImportReport {
    pub profiles: Vec<String>,
    pub keys: usize,
    /// Actions Agora can't run (yet), by name.
    pub skipped: Vec<String>,
}

/// Changes to profiles and pages.
#[derive(Debug, Clone)]
pub enum LayoutOp {
    GoToPage(String),
    AddPage,
    RemovePage(String),
    RenamePage(String, String),
    AddProfile(String),
    RemoveProfile(String),
    RenameProfile(String, String),
    /// The apps (program file names, e.g. "obs64.exe") that bring a profile up.
    SetProfileApps(String, Vec<String>),
    SwitchProfile(String),
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
    #[serde(default, skip_serializing_if = "is_default")]
    look: Look,
    /// Agora steps to run when the key is held down instead of tapped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    hold: Vec<Value>,
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

fn is_default(l: &Look) -> bool {
    *l == Look::default()
}

impl Instance {
    fn new(plugin: &str, action: &str, settings: Value) -> Self {
        Instance {
            context: new_context(),
            plugin: plugin.into(),
            action: action.into(),
            settings,
            state: 0,
            look: Look::default(),
            hold: Vec::new(),
            images: HashMap::new(),
            titles: HashMap::new(),
        }
    }

    fn is_builtin(&self, action: &str) -> bool {
        self.plugin == BUILTIN && self.action == action
    }
}

/// One screen of keys. A folder is a page with a parent.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Page {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent: Option<String>,
    #[serde(default)]
    keys: BTreeMap<usize, Instance>,
}

impl Page {
    fn new(name: &str, parent: Option<String>) -> Self {
        Page {
            id: new_context(),
            name: name.into(),
            parent,
            keys: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Profile {
    id: String,
    name: String,
    /// Profile pages in order, then folders.
    pages: Vec<Page>,
    /// The page showing when this profile is active.
    #[serde(default)]
    current: String,
    /// Programs that bring this profile up while they are in front.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    apps: Vec<String>,
}

impl Profile {
    fn new(name: &str) -> Self {
        let page = Page::new("Page 1", None);
        Profile {
            id: new_context(),
            name: name.into(),
            current: page.id.clone(),
            pages: vec![page],
            apps: Vec::new(),
        }
    }

    fn page(&self, id: &str) -> Option<&Page> {
        self.pages.iter().find(|p| p.id == id)
    }

    fn top_pages(&self) -> impl Iterator<Item = &Page> {
        self.pages.iter().filter(|p| p.parent.is_none())
    }

    /// The page and every folder inside it.
    fn subtree(&self, id: &str) -> Vec<String> {
        let mut out = vec![id.to_string()];
        let mut i = 0;
        while i < out.len() {
            let kids: Vec<String> = self
                .pages
                .iter()
                .filter(|p| p.parent.as_deref() == Some(out[i].as_str()))
                .map(|p| p.id.clone())
                .collect();
            out.extend(kids);
            i += 1;
        }
        out
    }

    /// The profile page a page sits under (itself if it is one).
    fn top_of(&self, id: &str) -> String {
        let mut cur = id.to_string();
        while let Some(parent) = self.page(&cur).and_then(|p| p.parent.clone()) {
            cur = parent;
        }
        cur
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Layout {
    #[serde(default)]
    profiles: Vec<Profile>,
    /// The active profile.
    #[serde(default)]
    active: String,
    #[serde(default)]
    global_settings: HashMap<String, Value>,
    /// Plugins hidden from the action list.
    #[serde(default)]
    hidden: BTreeSet<String>,
    /// Keys saved before profiles existed; moved to the first page on load.
    #[serde(default, skip_serializing)]
    keys: BTreeMap<usize, Instance>,
}

impl Layout {
    /// Makes sure there is a profile with a page, and that the active
    /// profile and its current page exist. Carries old layouts over.
    fn fix(&mut self) {
        if self.profiles.is_empty() {
            let mut p = Profile::new("Default");
            p.pages[0].keys = std::mem::take(&mut self.keys);
            self.profiles.push(p);
        }
        for p in &mut self.profiles {
            if p.pages.iter().all(|g| g.parent.is_some()) {
                p.pages.insert(0, Page::new("Page 1", None));
            }
            if p.page(&p.current).is_none() {
                let first = p
                    .top_pages()
                    .next()
                    .map(|g| g.id.clone())
                    .unwrap_or_default();
                p.current = first;
            }
        }
        if !self.profiles.iter().any(|p| p.id == self.active) {
            self.active = self.profiles[0].id.clone();
        }
    }

    fn profile(&self) -> &Profile {
        self.profiles
            .iter()
            .find(|p| p.id == self.active)
            .expect("layout fixed")
    }

    fn profile_mut(&mut self) -> &mut Profile {
        let active = self.active.clone();
        self.profiles
            .iter_mut()
            .find(|p| p.id == active)
            .expect("layout fixed")
    }

    fn page(&self) -> &Page {
        let p = self.profile();
        p.page(&p.current).expect("layout fixed")
    }

    fn keys(&self) -> &BTreeMap<usize, Instance> {
        &self.page().keys
    }

    fn keys_mut(&mut self) -> &mut BTreeMap<usize, Instance> {
        let p = self.profile_mut();
        let cur = p.current.clone();
        &mut p
            .pages
            .iter_mut()
            .find(|g| g.id == cur)
            .expect("layout fixed")
            .keys
    }

    /// Every placed action, on every page of every profile.
    fn instances(&self) -> impl Iterator<Item = &Instance> {
        self.profiles
            .iter()
            .flat_map(|p| p.pages.iter())
            .flat_map(|g| g.keys.values())
    }

    fn uses(&self, plugin: &str) -> bool {
        self.instances().any(|i| i.plugin == plugin)
    }

    fn view(&self) -> LayoutView {
        let p = self.profile();
        let top = p.top_of(&p.current);
        let mut path = Vec::new();
        let mut cur = p.current.clone();
        while cur != top {
            let Some(g) = p.page(&cur) else { break };
            path.insert(
                0,
                NameId {
                    id: g.id.clone(),
                    name: g.name.clone(),
                },
            );
            cur = g.parent.clone().unwrap_or_default();
        }
        LayoutView {
            profiles: self
                .profiles
                .iter()
                .map(|p| NameId {
                    id: p.id.clone(),
                    name: p.name.clone(),
                })
                .collect(),
            profile: p.id.clone(),
            pages: p
                .top_pages()
                .map(|g| NameId {
                    id: g.id.clone(),
                    name: g.name.clone(),
                })
                .collect(),
            page: p.current.clone(),
            top_page: top,
            path,
            apps: p.apps.clone(),
        }
    }
}

/// Puts imported keys on a page, making folder pages as it goes.
fn fill_page(
    pages: &mut Vec<Page>,
    page_id: &str,
    read: &vsd_import::ReadPage,
    ids: &HashMap<String, String>,
    count: &mut usize,
) {
    for (key, rk) in &read.keys {
        let mut settings = rk.settings.clone();
        if let Some(folder) = &rk.folder {
            let name = settings
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Folder")
                .to_string();
            let fp = Page::new(&name, Some(page_id.to_string()));
            let fid = fp.id.clone();
            pages.push(fp);
            fill_page(pages, &fid, folder, ids, count);
            settings["page"] = json!(fid);
        }
        if let Some(target) = &rk.switch_to {
            settings["profile"] = json!(ids.get(target).cloned().unwrap_or_default());
        }
        let mut inst = Instance::new(&rk.plugin, &rk.action, settings);
        inst.look = Look {
            title: rk.title.clone(),
            show_title: rk.show_title,
            align: rk.align.clone(),
            color: rk.color.clone(),
            font_size: rk.font_size,
            icon: rk.icon.clone(),
        };
        if let Some(p) = pages.iter_mut().find(|p| p.id == page_id) {
            p.keys.insert(*key, inst);
            *count += 1;
        }
    }
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
    SetHidden(String, bool),
    Forget(String, oneshot::Sender<Option<PathBuf>>),
    Key(usize, bool),
    Dial(usize, DialInput),
    Swipe(i32),
    DeviceConnected(bool),
    Device(DeviceDesc),
    Actions(oneshot::Sender<Vec<ActionInfo>>),
    Plugins(oneshot::Sender<Vec<PluginInfo>>),
    KeyInfo(usize, oneshot::Sender<Option<KeyInfo>>),
    SetSettings(usize, Value),
    SetLook(usize, Look),
    View(oneshot::Sender<LayoutView>),
    Overview(oneshot::Sender<PageOverview>),
    Foreground(String),
    ImportVsd(Vec<PathBuf>, oneshot::Sender<Result<ImportReport, String>>),
    Layout(LayoutOp, oneshot::Sender<Result<(), String>>),
    /// Redraw keys whose face changed; `true` sends every face again.
    AllFaces(bool),
    /// The rest of a multi-action, after a pause.
    Steps(Vec<Value>),
    /// Puts back what the last removal took away.
    Undo(oneshot::Sender<bool>),
    Shutdown(oneshot::Sender<()>),
    /// New readings for the monitor keys.
    Stats(Box<stats::Snapshot>),
    /// Twice a second, for clocks, timers and stopwatches.
    Tick,
    /// A key has been down long enough to count as held.
    HoldTimer(usize, u64),
    SetHold(usize, Vec<Value>),
    /// How an action the app ran went: ok or not, and text to show.
    ActionResult(String, bool, Option<String>),
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
    pub fn start(
        config: Config,
        events: std::sync::mpsc::Sender<HostEvent>,
    ) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("agora-host")
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let (plugins, problems) = manifest::discover(&config.plugin_dirs);
        for p in problems {
            let _ = events.send(HostEvent::Log {
                plugin: "agora".into(),
                message: format!("Skipped plugin: {p}"),
            });
        }
        let plugin_dirs: HashMap<String, PathBuf> = plugins
            .iter()
            .map(|p| (p.uuid.clone(), p.dir.clone()))
            .collect();
        let (tx, rx) = mpsc::unbounded_channel();
        let port = runtime.block_on(server::bind(Arc::new(plugin_dirs), tx.clone()))?;
        let mut layout = load_layout(&config.data_dir);
        layout.fix();
        let stats_tx = tx.clone();
        let stats_ctl = stats::start(move |s| stats_tx.send(Msg::Stats(Box::new(s))).is_ok());
        let tick_tx = tx.clone();
        let _ = std::thread::Builder::new()
            .name("agora-tick".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_millis(500));
                if tick_tx.send(Msg::Tick).is_err() {
                    break;
                }
            });
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
            down: HashMap::new(),
            auto_from: None,
            undo: None,
            stats: stats::Snapshot::default(),
            stats_ctl,
            holding: HashMap::new(),
            hold_seq: 0,
            watches: HashMap::new(),
            results: HashMap::new(),
        };
        runtime.spawn(state.run(rx));
        tx.send(Msg::AllFaces(true)).ok();
        Ok(Self {
            tx,
            port,
            runtime: Some(runtime),
        })
    }

    fn ask<T>(&self, make: impl FnOnce(oneshot::Sender<T>) -> Msg) -> Option<T> {
        let (otx, orx) = oneshot::channel();
        self.tx.send(make(otx)).ok()?;
        orx.blocking_recv().ok()
    }

    pub fn place(&self, key: usize, plugin: &str, action: &str) -> Result<(), String> {
        self.ask(|o| Msg::Place(key, plugin.into(), action.into(), o))
            .unwrap_or(Err("host stopped".into()))
    }

    pub fn clear(&self, key: usize) {
        let _ = self.tx.send(Msg::Clear(key));
    }

    /// Undoes the last removal of a key, page, profile or plugin, if nothing
    /// else changed since. Returns whether there was one to undo.
    pub fn undo(&self) -> bool {
        self.ask(Msg::Undo).unwrap_or(false)
    }

    /// Hides a plugin from the action list (or shows it again). Hiding
    /// clears the keys that use it.
    pub fn set_hidden(&self, plugin: &str, hidden: bool) {
        let _ = self.tx.send(Msg::SetHidden(plugin.into(), hidden));
    }

    /// Stops a plugin, clears its keys and drops it from the host, so its
    /// folder can be deleted. Returns the folder.
    pub fn forget(&self, plugin: &str) -> Option<PathBuf> {
        self.ask(|o| Msg::Forget(plugin.into(), o)).flatten()
    }

    /// Moves the action on `from` to `to`. Whatever was on `to` swaps to `from`.
    pub fn move_key(&self, from: usize, to: usize) {
        let _ = self.tx.send(Msg::Move(from, to));
    }

    pub fn key(&self, key: usize, pressed: bool) {
        let _ = self.tx.send(Msg::Key(key, pressed));
    }

    /// A knob, counted from 0 on the left.
    pub fn dial(&self, dial: usize, input: DialInput) {
        let _ = self.tx.send(Msg::Dial(dial, input));
    }

    /// A swipe along the touch strip: next page to the left, previous to
    /// the right, like turning pages.
    pub fn swipe(&self, direction: i32) {
        let _ = self.tx.send(Msg::Swipe(direction));
    }

    pub fn device_connected(&self, connected: bool) {
        let _ = self.tx.send(Msg::DeviceConnected(connected));
    }

    /// A different model was plugged in.
    pub fn set_device(&self, device: DeviceDesc) {
        let _ = self.tx.send(Msg::Device(device));
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

    /// Agora steps to run when a key on the current page is held.
    pub fn set_hold(&self, key: usize, steps: Vec<Value>) {
        let _ = self.tx.send(Msg::SetHold(key, steps));
    }

    /// How an action from [`HostEvent::Run`] went. `text`, if any, shows
    /// as the key's title until it runs again.
    pub fn action_result(&self, context: &str, ok: bool, text: Option<String>) {
        let _ = self.tx.send(Msg::ActionResult(context.into(), ok, text));
    }

    /// Settings for one of Agora's own actions on the current page.
    pub fn set_settings(&self, key: usize, settings: Value) {
        let _ = self.tx.send(Msg::SetSettings(key, settings));
    }

    /// The user's title and icon for a key on the current page.
    pub fn set_look(&self, key: usize, look: Look) {
        let _ = self.tx.send(Msg::SetLook(key, look));
    }

    pub fn layout(&self) -> Option<LayoutView> {
        self.ask(Msg::View)
    }

    /// The layout and every place's action on the page in front, in one go.
    pub fn overview(&self) -> Option<PageOverview> {
        self.ask(Msg::Overview)
    }

    /// Brings VSD Craft profiles (their folders) in as new profiles.
    pub fn import_vsd(&self, dirs: Vec<PathBuf>) -> Result<ImportReport, String> {
        self.ask(|o| Msg::ImportVsd(dirs, o))
            .unwrap_or(Err("host stopped".into()))
    }

    /// The program in front changed (its file name, e.g. "obs64.exe").
    pub fn foreground_app(&self, app: &str) {
        let _ = self.tx.send(Msg::Foreground(app.into()));
    }

    pub fn layout_op(&self, op: LayoutOp) -> Result<(), String> {
        self.ask(|o| Msg::Layout(op, o))
            .unwrap_or(Err("host stopped".into()))
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
    /// The action each held key was pressed on, so its keyUp goes there
    /// even if the page changed in between.
    down: HashMap<usize, String>,
    /// While an app has brought its profile up: the profile to go back to
    /// when another app comes to the front.
    auto_from: Option<String>,
    /// The layout as it was before the last removal (a key, page, profile
    /// or plugin), until something else changes.
    undo: Option<Layout>,
    /// The latest readings for monitor keys, and the sampler's controls.
    stats: stats::Snapshot,
    stats_ctl: stats::Control,
    /// Keys down that have an action for holding: which press, and whether
    /// the hold action has run.
    holding: HashMap<usize, (u64, bool)>,
    hold_seq: u64,
    /// Timers and stopwatches, by action context. Not saved.
    watches: HashMap<String, Watch>,
    /// Text an action sent back (Run command), by context, shown as title.
    results: HashMap<String, String>,
}

/// A running or paused timer or stopwatch.
#[derive(Debug, Clone, Default)]
struct Watch {
    started: Option<std::time::Instant>,
    banked: Duration,
    done: bool,
}

impl Watch {
    fn elapsed(&self) -> Duration {
        self.banked + self.started.map_or(Duration::ZERO, |t| t.elapsed())
    }
    fn toggle(&mut self) {
        match self.started.take() {
            Some(t) => self.banked += t.elapsed(),
            None => self.started = Some(std::time::Instant::now()),
        }
    }
}

/// How long a key must be down to count as held.
const HOLD: Duration = Duration::from_millis(500);

/// How long a Timer runs.
fn timer_length(settings: &Value) -> Duration {
    let n = |k: &str| settings.get(k).and_then(Value::as_u64).unwrap_or(0);
    let secs = n("minutes") * 60 + n("seconds") + n("hours") * 3600;
    Duration::from_secs(if secs == 0 { 300 } else { secs.min(100 * 3600) })
}

/// A title from text an action sent back: its first line, kept short.
fn result_title(text: &str) -> String {
    let line = text.trim().lines().next().unwrap_or("").trim();
    let mut out: String = line.chars().take(30).collect();
    if line.chars().count() > 30 {
        out.push('\u{2026}');
    }
    // Two lines of 15 fit on a key.
    if out.chars().count() > 15 {
        let at = out
            .char_indices()
            .filter(|(i, c)| *c == ' ' && *i <= 16)
            .map(|(i, _)| i)
            .next_back()
            .unwrap_or_else(|| out.char_indices().nth(15).map_or(out.len(), |(i, _)| i));
        let (a, b) = out.split_at(at);
        out = format!("{}\n{}", a.trim_end(), b.trim_start());
    }
    out
}

/// A random number below `n`, good enough for picking an action.
fn random_index(n: usize) -> usize {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    (h.finish() % n.max(1) as u64) as usize
}

/// "4:05" or "1:02:03".
fn clock_text(d: Duration) -> String {
    let t = d.as_secs();
    if t >= 3600 {
        format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
    } else {
        format!("{}:{:02}", t / 60, t % 60)
    }
}

/// The drive a Disk key shows, as "C:".
fn disk_drive(settings: &Value) -> String {
    let d = settings
        .get("drive")
        .and_then(Value::as_str)
        .unwrap_or("C:")
        .trim()
        .trim_end_matches(['\\', '/'])
        .to_ascii_uppercase();
    match d.len() {
        1 => format!("{d}:"),
        _ if d.is_empty() => "C:".into(),
        _ => d,
    }
}

fn layout_path(dir: &std::path::Path) -> PathBuf {
    dir.join("layout.json")
}

fn load_layout(dir: &std::path::Path) -> Layout {
    std::fs::read_to_string(layout_path(dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn new_context() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{:016X}{:08X}",
        t as u64,
        N.fetch_add(1, Ordering::Relaxed) as u32 ^ std::process::id()
    )
}

impl State {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Msg>) {
        // Start every plugin that has a key placed.
        let needed: BTreeSet<String> = self
            .layout
            .instances()
            .filter(|i| i.plugin != BUILTIN)
            .map(|i| i.plugin.clone())
            .collect();
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
        self.emit(HostEvent::Log {
            plugin: plugin.into(),
            message: message.into(),
        });
    }

    fn set_status(&mut self, plugin: &str, status: &str) {
        self.status.insert(plugin.into(), status.into());
        self.emit(HostEvent::PluginStatus {
            plugin: plugin.into(),
            status: status.into(),
        });
    }

    fn save(&mut self) {
        self.dirty = false;
        let dir = &self.config.data_dir;
        let _ = std::fs::create_dir_all(dir);
        let Ok(text) = serde_json::to_string_pretty(&self.layout) else {
            return;
        };
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
        self.handle_msg(msg);
        self.want_stats();
    }

    /// Tells the sampler whether monitor keys are showing, and which drives.
    fn want_stats(&mut self) {
        let mut any = false;
        let mut drives = Vec::new();
        for i in self.layout.keys().values() {
            if i.plugin == BUILTIN && builtin::is_monitor(&i.action) {
                any = true;
                if i.action == "disk" {
                    drives.push(disk_drive(&i.settings));
                }
            }
        }
        drives.sort();
        drives.dedup();
        use std::sync::atomic::Ordering;
        self.stats_ctl.wanted.store(any, Ordering::Relaxed);
        if let Ok(mut d) = self.stats_ctl.drives.lock() {
            *d = drives;
        }
    }

    fn monitor_face(&self, inst: &Instance) -> Face {
        let s = &self.stats;
        let pct = |v: f32| format!("{:.0}%", v.clamp(0.0, 100.0));
        let share = |used: u64, total: u64| used as f32 / total.max(1) as f32;
        let dash = "\u{2013}".to_string();
        let (label, value, bar) = match inst.action.as_str() {
            "cpu" => (
                "CPU".to_string(),
                s.cpu.map_or(dash, pct),
                s.cpu.map(|v| v / 100.0),
            ),
            "memory" => match s.memory {
                Some((u, t)) => ("MEMORY".into(), pct(100.0 * share(u, t)), Some(share(u, t))),
                None => ("MEMORY".into(), dash, None),
            },
            "disk" => {
                let d = disk_drive(&inst.settings);
                if inst.settings.get("show").and_then(Value::as_str) == Some("busy") {
                    let v = s.disk_busy.get(&d).copied();
                    (
                        format!("{d} BUSY"),
                        v.map_or(dash, pct),
                        v.map(|v| v / 100.0),
                    )
                } else {
                    match s.disk_space.get(&d) {
                        Some(&(u, t)) => (
                            format!("DISK {d}"),
                            pct(100.0 * share(u, t)),
                            Some(share(u, t)),
                        ),
                        None => (format!("DISK {d}"), dash, None),
                    }
                }
            }
            "gpu" => (
                "GPU".into(),
                s.gpu.map_or(dash, pct),
                s.gpu.map(|v| v / 100.0),
            ),
            "gpu-memory" => match s.gpu_memory {
                Some((u, Some(t))) => (
                    "GPU MEMORY".into(),
                    pct(100.0 * share(u, t)),
                    Some(share(u, t)),
                ),
                Some((u, None)) => ("GPU MEMORY".into(), stats::bytes(u), None),
                None => ("GPU MEMORY".into(), dash, None),
            },
            _ => {
                let local = inst.settings.get("which").and_then(Value::as_str) == Some("local");
                let ip = if local { &s.local_ip } else { &s.public_ip };
                (
                    if local { "LOCAL IP" } else { "PUBLIC IP" }.into(),
                    ip.clone().unwrap_or(dash),
                    None,
                )
            }
        };
        Face {
            image: Some(builtin::monitor_image(&label, &value, bar)),
            title: String::new(),
            show_title: true,
            align: "bottom".into(),
            font_size: 11.0,
            color: "#ffffff".into(),
            overlay: Overlay::None,
        }
    }

    fn handle_msg(&mut self, msg: Msg) {
        // A removal can be undone until something else changes the layout.
        match &msg {
            Msg::Clear(_)
            | Msg::SetHidden(_, true)
            | Msg::Layout(LayoutOp::RemovePage(_) | LayoutOp::RemoveProfile(_), _) => {
                self.undo = Some(self.layout.clone());
            }
            Msg::Place(..)
            | Msg::Move(..)
            | Msg::SetHidden(_, false)
            | Msg::Forget(..)
            | Msg::SetSettings(..)
            | Msg::SetHold(..)
            | Msg::SetLook(..)
            | Msg::ImportVsd(..)
            | Msg::Layout(..) => self.undo = None,
            _ => {}
        }
        match msg {
            Msg::Stats(snapshot) => {
                if *snapshot != self.stats {
                    self.stats = *snapshot;
                    let keys: Vec<usize> = self
                        .layout
                        .keys()
                        .iter()
                        .filter(|(_, i)| i.plugin == BUILTIN && builtin::is_monitor(&i.action))
                        .map(|(k, _)| *k)
                        .collect();
                    for k in keys {
                        self.redraw(k);
                    }
                }
            }
            Msg::Undo(done) => {
                let Some(before) = self.undo.take() else {
                    let _ = done.send(false);
                    return;
                };
                self.leave_page();
                self.layout = before;
                self.layout.fix();
                self.dirty = true;
                self.enter_page();
                self.stop_unused();
                self.emit(HostEvent::PageChanged);
                let _ = done.send(true);
            }
            Msg::SocketOpen(id, tx) => {
                self.conns.insert(
                    id,
                    Conn {
                        tx,
                        role: Role::New,
                    },
                );
            }
            Msg::SocketText(id, text) => self.on_text(id, &text),
            Msg::SocketClosed(id) => self.on_closed(id),
            Msg::PluginExited(uuid, why) => self.on_exited(&uuid, why),
            Msg::Restart(uuid) => {
                if self.layout.uses(&uuid) {
                    self.ensure_running(&uuid);
                }
            }
            Msg::Place(key, plugin, action, done) => {
                let _ = done.send(self.place(key, &plugin, &action));
            }
            Msg::Clear(key) => self.clear(key),
            Msg::Move(from, to) => self.move_key(from, to),
            Msg::SetHidden(uuid, hidden) => {
                if hidden {
                    self.clear_plugin_keys(&uuid);
                    self.layout.hidden.insert(uuid);
                } else {
                    self.layout.hidden.remove(&uuid);
                }
                self.dirty = true;
            }
            Msg::Forget(uuid, done) => {
                self.clear_plugin_keys(&uuid);
                self.stop_plugin(&uuid);
                self.layout.global_settings.remove(&uuid);
                self.layout.hidden.remove(&uuid);
                self.dirty = true;
                let _ = done.send(self.plugins.remove(&uuid).map(|p| p.dir));
            }
            Msg::Key(key, pressed) => self.on_key(key, pressed),
            Msg::Tick => self.on_tick(),
            Msg::HoldTimer(key, seq) => self.on_hold(key, seq),
            Msg::SetHold(key, steps) => {
                if let Some(inst) = self.layout.keys_mut().get_mut(&key) {
                    inst.hold = steps
                        .into_iter()
                        .filter(|s| {
                            s.get("action")
                                .and_then(Value::as_str)
                                .is_some_and(builtin::step_allowed)
                        })
                        .collect();
                    self.dirty = true;
                }
            }
            Msg::ActionResult(context, ok, text) => {
                if let Some(key) = self.key_of(&context) {
                    match text {
                        Some(t) => {
                            self.results.insert(context, t);
                        }
                        None => {
                            self.results.remove(&context);
                        }
                    }
                    self.redraw(key);
                    self.flash(key, if ok { Overlay::Ok } else { Overlay::Alert });
                }
            }
            Msg::Dial(dial, input) => self.on_dial(dial, input),
            Msg::Swipe(direction) => {
                let action = if direction < 0 {
                    builtin::NEXT_PAGE
                } else {
                    builtin::PREVIOUS_PAGE
                };
                self.run_builtin(&Instance::new(BUILTIN, action, json!({})));
            }
            Msg::DeviceConnected(c) => self.on_device(c),
            Msg::Device(d) => {
                if d != self.config.device {
                    let connected = self.device_connected;
                    if connected {
                        self.on_device(false);
                    }
                    self.leave_page();
                    self.config.device = d;
                    self.shown.clear();
                    self.enter_page();
                    if connected {
                        self.on_device(true);
                    }
                }
            }
            Msg::Actions(done) => {
                let _ = done.send(self.action_list());
            }
            Msg::Plugins(done) => {
                let _ = done.send(self.plugin_list());
            }
            Msg::KeyInfo(key, done) => {
                let _ = done.send(self.key_info(key));
            }
            Msg::SetSettings(key, settings) => self.set_builtin_settings(key, settings),
            Msg::SetLook(key, look) => {
                if let Some(inst) = self.layout.keys_mut().get_mut(&key) {
                    inst.look = look;
                    self.dirty = true;
                    self.redraw(key);
                    self.send_title_params(key);
                }
            }
            Msg::View(done) => {
                let _ = done.send(self.layout.view());
            }
            Msg::Overview(done) => {
                let keys = self.all_slots().filter_map(|k| self.key_info(k)).collect();
                let _ = done.send(PageOverview {
                    layout: self.layout.view(),
                    keys,
                });
            }
            Msg::Foreground(app) => self.on_foreground(&app),
            Msg::ImportVsd(dirs, done) => {
                let r = self.import_vsd(&dirs);
                if r.is_ok() {
                    self.dirty = true;
                    self.emit(HostEvent::PageChanged);
                }
                let _ = done.send(r);
            }
            Msg::Layout(op, done) => {
                // Choosing a profile yourself ends any app-chosen one.
                if matches!(
                    op,
                    LayoutOp::SwitchProfile(_)
                        | LayoutOp::AddProfile(_)
                        | LayoutOp::RemoveProfile(_)
                ) {
                    self.auto_from = None;
                }
                let r = self.layout_op(op);
                if r.is_ok() {
                    self.dirty = true;
                    self.emit(HostEvent::PageChanged);
                }
                let _ = done.send(r);
            }
            Msg::Steps(steps) => self.run_steps(steps),
            Msg::AllFaces(force) => {
                if force {
                    self.shown.clear();
                }
                let keys: Vec<usize> = self.layout.keys().keys().copied().collect();
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
        match slot(key) {
            Slot::Key => json!({ "column": key % cols, "row": key / cols }),
            // Under the screen keys.
            Slot::Button => json!({ "column": key - BUTTON_BASE, "row": self.config.device.rows }),
            // Knobs number from the left in a row of their own, as on Elgato's.
            Slot::Dial => json!({ "column": key - DIAL_BASE, "row": 0 }),
        }
    }

    /// Every key, button and knob this dock has.
    fn all_slots(&self) -> impl Iterator<Item = usize> {
        let d = &self.config.device;
        (0..d.rows * d.columns)
            .chain(BUTTON_BASE..BUTTON_BASE + d.buttons)
            .chain(DIAL_BASE..DIAL_BASE + d.dials)
    }

    /// Whether this dock has the key, button or knob.
    fn slot_exists(&self, key: usize) -> bool {
        let d = &self.config.device;
        match slot(key) {
            Slot::Key => key < d.rows * d.columns,
            Slot::Button => key - BUTTON_BASE < d.buttons,
            Slot::Dial => key - DIAL_BASE < d.dials,
        }
    }

    /// The controller name plugins expect for a place: knobs are "Encoder"
    /// to Elgato plugins and "Knob" to Stream Dock ones.
    fn controller(&self, key: usize) -> &'static str {
        if slot(key) != Slot::Dial {
            // An action made only for "Information" hears that name back.
            let info_only = self.layout.keys().get(&key).is_some_and(|i| {
                self.plugins
                    .get(&i.plugin)
                    .and_then(|p| p.action(&i.action))
                    .is_some_and(|a| {
                        !a.controllers.is_empty()
                            && a.controllers.iter().all(|c| c == "Information")
                    })
            });
            return if info_only { "Information" } else { "Keypad" };
        }
        let knob = self.layout.keys().get(&key).is_some_and(|i| {
            self.plugins
                .get(&i.plugin)
                .and_then(|p| p.action(&i.action))
                .is_some_and(|a| a.controllers.iter().any(|c| c == "Knob"))
        });
        if knob {
            "Knob"
        } else {
            "Encoder"
        }
    }

    fn key_of(&self, context: &str) -> Option<usize> {
        self.layout
            .keys()
            .iter()
            .find(|(_, i)| i.context == context)
            .map(|(k, _)| *k)
    }

    fn instance_event(&self, key: usize, event: &str, extra: Value) -> Option<(String, Value)> {
        let inst = self.layout.keys().get(&key)?;
        let mut payload = json!({
            "settings": inst.settings,
            "coordinates": self.coords(key),
            "controller": self.controller(key),
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
        let version = self
            .plugins
            .get(plugin)
            .map(|p| p.manifest.version.clone())
            .unwrap_or_default();
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
        let v = self
            .plugins
            .get(plugin)
            .and_then(|p| face::file_data_uri(&p.dir, rel));
        self.image_cache.insert(key, v.clone());
        v
    }

    fn face_for(&mut self, key: usize) -> Face {
        let Some(inst) = self.layout.keys().get(&key).cloned() else {
            return Face::default();
        };
        let mut face = if inst.plugin == BUILTIN && builtin::is_monitor(&inst.action) {
            self.monitor_face(&inst)
        } else if inst.plugin == BUILTIN && builtin::is_live(&inst.action) {
            self.live_face(&inst)
        } else if inst.plugin == BUILTIN {
            Face {
                image: builtin::icon_for(&inst.action, inst.state),
                title: builtin::default_title(&inst.action, &inst.settings),
                show_title: true,
                align: "bottom".into(),
                font_size: 11.0,
                color: "#ffffff".into(),
                overlay: Overlay::None,
            }
        } else {
            self.plugin_face(&inst)
        };
        if inst.plugin == BUILTIN && inst.action == builtin::PAGE_INDICATOR {
            let profile = self.layout.profile();
            let tops: Vec<String> = profile.top_pages().map(|p| p.id.clone()).collect();
            let top = profile.top_of(&profile.current);
            let at = tops.iter().position(|p| *p == top).unwrap_or(0);
            face.title = format!("{} / {}", at + 1, tops.len().max(1));
            face.align = "middle".into();
            face.font_size = 16.0;
            face.image = None;
        }
        if let Some(text) = self.results.get(&inst.context) {
            face.title = result_title(text);
            face.align = "middle".into();
            face.font_size = 10.0;
        }
        let look = &inst.look;
        if let Some(t) = &look.title {
            face.title = t.clone();
        }
        if let Some(v) = look.show_title {
            face.show_title = v;
        }
        if let Some(v) = &look.align {
            face.align = v.clone();
        }
        if let Some(v) = &look.color {
            face.color = v.clone();
        }
        if let Some(v) = look.font_size {
            face.font_size = v;
        }
        if let Some(v) = &look.icon {
            face.image = Some(v.clone());
        }
        face
    }

    fn plugin_face(&mut self, inst: &Instance) -> Face {
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
            title: inst
                .titles
                .get(&inst.state)
                .cloned()
                .unwrap_or(state_def.title.clone()),
            show_title: state_def.show_title.unwrap_or(true),
            align: state_def
                .title_alignment
                .clone()
                .unwrap_or_else(|| "bottom".into()),
            font_size,
            color: state_def
                .title_color
                .clone()
                .unwrap_or_else(|| "#ffffff".into()),
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
        if uuid == BUILTIN || self.running.contains_key(uuid) || self.plugin_conn.contains_key(uuid)
        {
            return;
        }
        let Some(plugin) = self.plugins.get(uuid).cloned() else {
            self.log(uuid, "Plugin not installed");
            return;
        };
        let info = self.info_json(uuid);
        match plugin.kind() {
            Some(Kind::Html(path)) => {
                let rel = path
                    .strip_prefix(&plugin.dir)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                let url =
                    server::connect_url(self.port, uuid, &rel, uuid, "registerPlugin", &info, None);
                self.running.insert(uuid.into(), Running::Html);
                self.emit(HostEvent::StartHtmlPlugin {
                    plugin: uuid.into(),
                    url,
                });
                self.set_status(uuid, "starting");
            }
            Some(kind) => {
                let logs = self.config.data_dir.join("logs");
                match launch::spawn(
                    &plugin,
                    &kind,
                    self.config.node.as_deref(),
                    self.port,
                    &info,
                    &logs,
                    self.tx.clone(),
                    self.events.clone(),
                ) {
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
                Running::Html => self.emit(HostEvent::StopHtmlPlugin {
                    plugin: uuid.into(),
                }),
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

    fn key_count(&self) -> usize {
        self.config.device.rows * self.config.device.columns
    }

    /// Whether an action can go on keys and buttons ("Keypad") or knobs
    /// ("Encoder").
    fn fits(&self, plugin: &str, action: &str, controller: &str) -> bool {
        if plugin == BUILTIN {
            return (action == builtin::DIAL) == (controller == "Encoder");
        }
        let Some(a) = self.plugins.get(plugin).and_then(|p| p.action(action)) else {
            return false;
        };
        match controller {
            "Encoder" => a.controllers.iter().any(|c| c == "Encoder" || c == "Knob"),
            _ => {
                a.controllers.is_empty()
                    || a.controllers
                        .iter()
                        .any(|c| c == "Keypad" || c == "Information")
            }
        }
    }

    fn place(&mut self, key: usize, plugin: &str, action: &str) -> Result<(), String> {
        if !self.slot_exists(key) {
            return Err(format!("no key {key}"));
        }
        let controller = if slot(key) == Slot::Dial {
            "Encoder"
        } else {
            "Keypad"
        };
        if !self.fits(plugin, action, controller) {
            return Err(if controller == "Encoder" {
                "That action can't go on a knob. Use Knob, or a plugin action made for knobs."
                    .into()
            } else {
                "That action is made for knobs.".into()
            });
        }
        if plugin == BUILTIN {
            let def =
                builtin::def(action).ok_or_else(|| format!("Agora has no action {action}"))?;
            let mut settings = (def.settings)();
            self.clear(key);
            if action == builtin::FOLDER {
                // A folder gets its own page, with a Back key on it.
                let mut page = Page::new("Folder", Some(self.layout.page().id.clone()));
                page.keys
                    .insert(0, Instance::new(BUILTIN, builtin::BACK, json!({})));
                settings["page"] = json!(page.id);
                settings["name"] = json!("Folder");
                self.layout.profile_mut().pages.push(page);
            }
            self.layout
                .keys_mut()
                .insert(key, Instance::new(BUILTIN, action, settings));
            self.dirty = true;
            self.redraw(key);
            return Ok(());
        }
        let def = self
            .plugins
            .get(plugin)
            .and_then(|p| p.action(action))
            .ok_or_else(|| format!("{plugin} has no action {action}"))?;
        let settings = def.settings.clone().unwrap_or_else(empty_object);
        self.clear(key);
        self.layout
            .keys_mut()
            .insert(key, Instance::new(plugin, action, settings));
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
        if !self.layout.keys().contains_key(&key) {
            return;
        }
        self.send_instance(key, "willDisappear", json!({}));
        let inst = self.layout.keys_mut().remove(&key).expect("checked");
        self.dirty = true;
        self.show(key, Face::default());
        if inst.is_builtin(builtin::FOLDER) {
            if let Some(page) = inst.settings.get("page").and_then(Value::as_str) {
                self.remove_pages(page);
            }
        } else if inst.plugin != BUILTIN && !self.layout.uses(&inst.plugin) {
            self.stop_plugin(&inst.plugin);
        }
    }

    /// Removes a page (not the current one) and the folders inside it, and
    /// stops plugins nothing uses any more.
    fn remove_pages(&mut self, id: &str) {
        let doomed = self.layout.profile().subtree(id);
        self.layout
            .profile_mut()
            .pages
            .retain(|p| !doomed.contains(&p.id));
        self.dirty = true;
        self.stop_unused();
    }

    fn stop_unused(&mut self) {
        let unused: Vec<String> = self
            .running
            .keys()
            .filter(|p| !self.layout.uses(p))
            .cloned()
            .collect();
        for p in unused {
            self.stop_plugin(&p);
        }
    }

    fn clear_plugin_keys(&mut self, uuid: &str) {
        let keys: Vec<usize> = self
            .layout
            .keys()
            .iter()
            .filter(|(_, i)| i.plugin == uuid)
            .map(|(k, _)| *k)
            .collect();
        for k in keys {
            self.clear(k);
        }
        // Pages not showing: nothing to tell the plugin.
        for g in self
            .layout
            .profiles
            .iter_mut()
            .flat_map(|p| p.pages.iter_mut())
        {
            g.keys.retain(|_, i| i.plugin != uuid);
        }
    }

    fn move_key(&mut self, from: usize, to: usize) {
        // Keys and buttons swap freely; knobs only with knobs.
        let same_kind = (slot(from) == Slot::Dial) == (slot(to) == Slot::Dial);
        if from == to
            || !same_kind
            || !self.slot_exists(from)
            || !self.slot_exists(to)
            || !self.layout.keys().contains_key(&from)
        {
            return;
        }
        // Plugins see the action leave its old spot and appear at the new one;
        // the context stays the same, so settings and open panels carry over.
        let moved: Vec<usize> = [from, to]
            .into_iter()
            .filter(|k| self.layout.keys().contains_key(k))
            .collect();
        for &k in &moved {
            self.send_instance(k, "willDisappear", json!({}));
        }
        let a = self.layout.keys_mut().remove(&from);
        let b = self.layout.keys_mut().remove(&to);
        if let Some(a) = a {
            self.layout.keys_mut().insert(to, a);
        }
        if let Some(b) = b {
            self.layout.keys_mut().insert(from, b);
        }
        self.dirty = true;
        for k in [from, to] {
            self.redraw(k);
            let connected = self
                .layout
                .keys()
                .get(&k)
                .is_some_and(|i| self.plugin_conn.contains_key(&i.plugin));
            if connected {
                self.appear(k);
            }
        }
    }

    fn appear(&mut self, key: usize) {
        self.send_instance(key, "willAppear", json!({}));
        self.send_title_params(key);
    }

    fn send_title_params(&mut self, key: usize) {
        if self
            .layout
            .keys()
            .get(&key)
            .is_none_or(|i| i.plugin == BUILTIN)
        {
            return;
        }
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
        let Some(inst) = self.layout.keys().get(&key).cloned() else {
            self.down.remove(&key);
            return;
        };
        if !pressed {
            let same = self.down.remove(&key).as_deref() == Some(inst.context.as_str());
            if let Some((_, fired)) = self.holding.remove(&key) {
                // Let go before it counted as held: a tap.
                if same && !fired {
                    self.tap(key, &inst);
                }
                return;
            }
            // Only the action that saw the press gets the release.
            if same && inst.plugin != BUILTIN {
                self.send_instance(key, "keyUp", json!({}));
            }
            return;
        }
        self.down.insert(key, inst.context.clone());
        if inst.plugin == BUILTIN {
            // The bounce goes out first, so a face the action changes
            // (a new page, a toggle) arrives after it and stops it.
            if let Some(face) = self.shown.get(&key).cloned() {
                if face != Face::default() {
                    self.emit(HostEvent::Pressed { key, face });
                }
            }
        }
        if !inst.hold.is_empty() || (inst.plugin == BUILTIN && builtin::resets_on_hold(&inst.action))
        {
            // Wait to see whether it's a tap or a hold.
            self.hold_seq += 1;
            let seq = self.hold_seq;
            self.holding.insert(key, (seq, false));
            let tx = self.tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(HOLD).await;
                let _ = tx.send(Msg::HoldTimer(key, seq));
            });
            return;
        }
        if inst.plugin == BUILTIN {
            self.run_builtin(&inst);
        } else {
            self.send_instance(key, "keyDown", json!({}));
        }
    }

    /// A short press on a key that also has an action for holding.
    fn tap(&mut self, key: usize, inst: &Instance) {
        if inst.plugin == BUILTIN {
            self.run_builtin(inst);
        } else {
            self.send_instance(key, "keyDown", json!({}));
            self.send_instance(key, "keyUp", json!({}));
        }
    }

    fn on_hold(&mut self, key: usize, seq: u64) {
        match self.holding.get_mut(&key) {
            Some((s, fired)) if *s == seq && !*fired => *fired = true,
            _ => return,
        }
        let Some(inst) = self.layout.keys().get(&key).cloned() else {
            return;
        };
        if self.down.get(&key) != Some(&inst.context) {
            return;
        }
        if !inst.hold.is_empty() {
            self.run_steps(inst.hold.clone());
        } else {
            self.reset_watch(key, &inst);
        }
    }

    /// Holding a timer, stopwatch or counter puts it back to the start.
    fn reset_watch(&mut self, key: usize, inst: &Instance) {
        if inst.action == "counter" {
            if let Some(i) = self.layout.keys_mut().get_mut(&key) {
                i.settings["count"] = json!(0);
                self.dirty = true;
            }
        } else {
            self.watches.remove(&inst.context);
        }
        self.redraw(key);
    }

    fn on_tick(&mut self) {
        let keys: Vec<(usize, Instance)> = self
            .layout
            .keys()
            .iter()
            .filter(|(_, i)| i.plugin == BUILTIN && builtin::is_live(&i.action))
            .map(|(k, i)| (*k, i.clone()))
            .collect();
        for (k, inst) in keys {
            if inst.action == "timer" {
                let total = timer_length(&inst.settings);
                if let Some(w) = self.watches.get_mut(&inst.context) {
                    if !w.done && w.started.is_some() && w.elapsed() >= total {
                        w.started = None;
                        w.banked = total;
                        w.done = true;
                        let sound = inst
                            .settings
                            .get("sound")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        if !sound.is_empty() {
                            self.emit(HostEvent::Run {
                                context: inst.context.clone(),
                                action: "sound".into(),
                                settings: json!({ "path": sound, "volume": 100 }),
                            });
                        }
                    }
                }
            }
            self.redraw(k);
        }
    }

    /// The face of a clock, timer, stopwatch or counter.
    fn live_face(&self, inst: &Instance) -> Face {
        let watch = self.watches.get(&inst.context);
        let paused = watch.is_some_and(|w| w.started.is_none() && !w.done && w.banked > Duration::ZERO);
        let (label, value, bar) = match inst.action.as_str() {
            "clock" => {
                let now = chrono::Local::now();
                let twelve = inst.settings.get("format").and_then(Value::as_str) == Some("12h");
                let secs = inst.settings.get("seconds").and_then(Value::as_bool).unwrap_or(false);
                let fmt = match (twelve, secs) {
                    (true, true) => "%-I:%M:%S",
                    (true, false) => "%-I:%M %p",
                    (false, true) => "%H:%M:%S",
                    (false, false) => "%H:%M",
                };
                (
                    now.format("%a %-d %b").to_string().to_uppercase(),
                    now.format(fmt).to_string(),
                    None,
                )
            }
            "timer" => {
                let total = timer_length(&inst.settings);
                let w = watch.cloned().unwrap_or_default();
                let left = total.saturating_sub(w.elapsed());
                let label = if w.done {
                    "DONE"
                } else if paused {
                    "PAUSED"
                } else {
                    "TIMER"
                };
                // Round up, so it shows 0:00 only when it's over.
                let shown = Duration::from_secs(left.as_secs() + u64::from(left.subsec_nanos() > 0));
                (
                    label.to_string(),
                    clock_text(shown),
                    Some(left.as_secs_f32() / total.as_secs_f32().max(1.0)),
                )
            }
            "stopwatch" => (
                if paused { "PAUSED" } else { "STOPWATCH" }.to_string(),
                clock_text(watch.map_or(Duration::ZERO, Watch::elapsed)),
                None,
            ),
            _ => {
                let name = inst.settings.get("name").and_then(Value::as_str).unwrap_or("").trim();
                (
                    if name.is_empty() { "COUNT".to_string() } else { name.to_uppercase() },
                    inst.settings.get("count").and_then(Value::as_i64).unwrap_or(0).to_string(),
                    None,
                )
            }
        };
        Face {
            image: Some(builtin::monitor_image(&label, &value, bar)),
            title: String::new(),
            show_title: true,
            align: "bottom".into(),
            font_size: 11.0,
            color: "#ffffff".into(),
            overlay: Overlay::None,
        }
    }

    fn on_dial(&mut self, dial: usize, input: DialInput) {
        let key = DIAL_BASE + dial;
        let Some(inst) = self.layout.keys().get(&key).cloned() else {
            self.down.remove(&key);
            return;
        };
        if inst.is_builtin(builtin::DIAL) {
            let (field, times) = match input {
                DialInput::Turn(t) if t < 0 => ("left", t.unsigned_abs()),
                DialInput::Turn(t) => ("right", t.unsigned_abs()),
                DialInput::Press | DialInput::Tap => ("press", 1),
                DialInput::Release => return,
            };
            let steps: Vec<Value> = inst
                .settings
                .get(field)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            // A fast spin can report several steps at once; ten is plenty.
            for _ in 0..times.min(10) {
                self.run_steps(steps.clone());
            }
            return;
        }
        if inst.plugin == BUILTIN {
            return;
        }
        match input {
            DialInput::Turn(ticks) => {
                let pressed = self.down.contains_key(&key);
                self.send_instance(
                    key,
                    "dialRotate",
                    json!({ "ticks": ticks, "pressed": pressed }),
                );
            }
            DialInput::Press => {
                self.down.insert(key, inst.context.clone());
                self.send_instance(key, "dialDown", json!({}));
            }
            DialInput::Release => {
                if self.down.remove(&key).as_deref() == Some(inst.context.as_str()) {
                    self.send_instance(key, "dialUp", json!({}));
                }
            }
            DialInput::Tap => {
                self.send_instance(
                    key,
                    "touchTap",
                    json!({ "tapPos": [100, 50], "hold": false }),
                );
            }
        }
    }

    // ---- Agora's own actions and navigation --------------------------------

    /// Runs multi-action steps in order. A pause hands the rest to a timer.
    fn run_steps(&mut self, mut steps: Vec<Value>) {
        while !steps.is_empty() {
            let step = steps.remove(0);
            let action = step.get("action").and_then(Value::as_str).unwrap_or("");
            if !builtin::step_allowed(action) {
                continue;
            }
            if action == builtin::DELAY {
                let ms = step
                    .get("settings")
                    .and_then(|s| s.get("ms"))
                    .and_then(Value::as_u64)
                    .unwrap_or(500);
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(ms.min(60_000))).await;
                    let _ = tx.send(Msg::Steps(steps));
                });
                return;
            }
            let settings = step
                .get("settings")
                .cloned()
                .filter(Value::is_object)
                .unwrap_or_else(empty_object);
            self.run_builtin(&Instance::new(BUILTIN, action, settings));
        }
    }

    fn run_builtin(&mut self, inst: &Instance) {
        let Some(def) = builtin::def(&inst.action) else {
            return;
        };
        if inst.action == builtin::MULTI || inst.action == builtin::TOGGLE {
            let list = |k: &str| {
                inst.settings
                    .get(k)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            };
            let steps = if inst.action == builtin::TOGGLE && inst.state == 1 {
                list("steps_b")
            } else {
                list("steps")
            };
            if inst.action == builtin::TOGGLE {
                self.flip_toggle(&inst.context);
            }
            self.run_steps(steps);
            return;
        }
        match inst.action.as_str() {
            builtin::HOTKEY_SWITCH => {
                let k = if inst.state == 1 { "keys_b" } else { "keys" };
                let keys = inst.settings.get(k).cloned().unwrap_or(json!(""));
                self.flip_toggle(&inst.context);
                self.emit(HostEvent::Run {
                    context: inst.context.clone(),
                    action: "hotkey".into(),
                    settings: json!({ "keys": keys }),
                });
                return;
            }
            builtin::RANDOM => {
                let steps: Vec<Value> = inst
                    .settings
                    .get("steps")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter(|s| s.get("action").and_then(Value::as_str) != Some(builtin::DELAY)).cloned().collect())
                    .unwrap_or_default();
                if !steps.is_empty() {
                    let pick = random_index(steps.len());
                    self.run_steps(vec![steps[pick].clone()]);
                }
                return;
            }
            "timer" | "stopwatch" => {
                let w = self.watches.entry(inst.context.clone()).or_default();
                if w.done {
                    *w = Watch::default();
                } else {
                    w.toggle();
                }
                if let Some(k) = self.key_of(&inst.context) {
                    self.redraw(k);
                }
                return;
            }
            "counter" => {
                if let Some(k) = self.key_of(&inst.context) {
                    let step = inst.settings.get("step").and_then(Value::as_i64).unwrap_or(1);
                    let count = inst.settings.get("count").and_then(Value::as_i64).unwrap_or(0);
                    if let Some(i) = self.layout.keys_mut().get_mut(&k) {
                        i.settings["count"] = json!(count.saturating_add(step));
                        self.dirty = true;
                    }
                    self.redraw(k);
                }
                return;
            }
            "clock" => return,
            _ => {}
        }
        if builtin::is_monitor(&inst.action) {
            // A press looks the address up again; the others just show.
            if inst.action == "ip" {
                self.stats_ctl
                    .refresh_ip
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            return;
        }
        if !def.navigation {
            self.emit(HostEvent::Run {
                context: inst.context.clone(),
                action: inst.action.clone(),
                settings: inst.settings.clone(),
            });
            return;
        }
        let profile = self.layout.profile();
        let tops: Vec<String> = profile.top_pages().map(|p| p.id.clone()).collect();
        let top = profile.top_of(&profile.current);
        let at = tops.iter().position(|p| *p == top).unwrap_or(0);
        let target = match inst.action.as_str() {
            builtin::FOLDER => inst
                .settings
                .get("page")
                .and_then(Value::as_str)
                .map(str::to_string),
            builtin::BACK => self.layout.page().parent.clone(),
            builtin::NEXT_PAGE | builtin::PAGE_INDICATOR => {
                Some(tops[(at + 1) % tops.len()].clone())
            }
            builtin::PREVIOUS_PAGE => Some(tops[(at + tops.len() - 1) % tops.len()].clone()),
            builtin::GO_TO_PAGE => {
                let n = inst
                    .settings
                    .get("page")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .max(1) as usize;
                tops.get(n - 1).cloned()
            }
            builtin::SWITCH_PROFILE => {
                let id = inst
                    .settings
                    .get("profile")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let ids: Vec<String> = self.layout.profiles.iter().map(|p| p.id.clone()).collect();
                let here = ids
                    .iter()
                    .position(|p| *p == self.layout.active)
                    .unwrap_or(0);
                let id = match id {
                    "next" if !ids.is_empty() => ids[(here + 1) % ids.len()].as_str(),
                    "previous" if !ids.is_empty() => {
                        ids[(here + ids.len() - 1) % ids.len()].as_str()
                    }
                    other => other,
                };
                if self.layout.profiles.iter().any(|p| p.id == id) {
                    self.auto_from = None;
                    self.switch_profile(id);
                    self.emit(HostEvent::PageChanged);
                }
                return;
            }
            _ => None,
        };
        if let Some(page) = target {
            if self.go_to_page(&page) {
                self.emit(HostEvent::PageChanged);
            }
        }
    }

    /// A Toggle key remembers which set runs next.
    fn flip_toggle(&mut self, context: &str) {
        let key = self.key_of(context);
        if let Some(k) = key {
            if let Some(i) = self.layout.keys_mut().get_mut(&k) {
                i.state = 1 - i.state.min(1);
                self.dirty = true;
            }
            self.redraw(k);
        }
    }

    /// Tells plugins the current page's actions are going away.
    fn leave_page(&mut self) {
        let keys: Vec<usize> = self.layout.keys().keys().copied().collect();
        for k in keys {
            self.send_instance(k, "willDisappear", json!({}));
        }
        self.down.clear();
        self.holding.clear();
    }

    /// Draws every key of the page now showing and tells plugins.
    fn enter_page(&mut self) {
        let d = &self.config.device;
        let slots: Vec<usize> = (0..self.key_count())
            .chain(BUTTON_BASE..BUTTON_BASE + d.buttons)
            .chain(DIAL_BASE..DIAL_BASE + d.dials)
            .collect();
        for k in slots {
            self.redraw(k);
        }
        let keys: Vec<(usize, String)> = self
            .layout
            .keys()
            .iter()
            .map(|(k, i)| (*k, i.plugin.clone()))
            .collect();
        for (k, plugin) in keys {
            if self.plugin_conn.contains_key(&plugin) {
                self.appear(k);
            } else {
                self.ensure_running(&plugin);
            }
        }
    }

    /// Shows a page of the active profile. Returns false if there's no such
    /// page or it is already showing.
    fn go_to_page(&mut self, id: &str) -> bool {
        let profile = self.layout.profile();
        if profile.current == id || profile.page(id).is_none() {
            return false;
        }
        self.leave_page();
        self.layout.profile_mut().current = id.to_string();
        self.dirty = true;
        self.enter_page();
        true
    }

    fn import_vsd(&mut self, dirs: &[PathBuf]) -> Result<ImportReport, String> {
        let plugins = &self.plugins;
        let known = |action: &str| {
            plugins
                .values()
                .find(|p| p.action(action).is_some())
                .map(|p| p.uuid.clone())
        };
        let dev = &self.config.device;
        let (cols, rows, buttons) = (dev.columns, dev.rows, dev.buttons);
        let mut read = Vec::new();
        let mut skipped = Vec::new();
        for d in dirs {
            let (p, s) = vsd_import::read(d, cols, rows, buttons, &known)?;
            skipped.extend(s);
            read.push(p);
        }
        // New ids first, so Scene Shift keys can point at each other.
        // Keyed by folder name and by the bare id Scene Shift keys store.
        let mut ids: HashMap<String, String> = HashMap::new();
        for p in &read {
            let id = new_context();
            if let Some(bare) = p.id.strip_suffix(".sdProfile") {
                ids.insert(bare.to_string(), id.clone());
            }
            ids.insert(p.id.clone(), id);
        }
        let mut report = ImportReport {
            profiles: Vec::new(),
            keys: 0,
            skipped: Vec::new(),
        };
        for rp in read {
            let mut pages = Vec::new();
            for (n, page) in rp.pages.iter().enumerate() {
                let top = Page::new(&format!("Page {}", n + 1), None);
                let top_id = top.id.clone();
                pages.push(top);
                fill_page(&mut pages, &top_id, page, &ids, &mut report.keys);
            }
            let current = pages[0].id.clone();
            let name = if rp.name.trim().is_empty() {
                "Imported".to_string()
            } else {
                rp.name.clone()
            };
            report.profiles.push(name.clone());
            self.layout.profiles.push(Profile {
                id: ids[&rp.id].clone(),
                name,
                pages,
                current,
                apps: rp.apps,
            });
        }
        // Plugins for keys on pages not showing start now, as at launch.
        let needed: BTreeSet<String> = self
            .layout
            .instances()
            .filter(|i| i.plugin != BUILTIN)
            .map(|i| i.plugin.clone())
            .collect();
        for p in needed {
            self.ensure_running(&p);
        }
        skipped.sort();
        skipped.dedup();
        report.skipped = skipped;
        Ok(report)
    }

    /// Brings up the profile that lists the app now in front, and goes back
    /// to the one you chose when an app without a profile comes forward.
    fn on_foreground(&mut self, app: &str) {
        let app = app.to_lowercase();
        // Using Agora itself (to edit keys) keeps whatever is showing.
        if app.is_empty() || app == "agora.exe" {
            return;
        }
        let target = self
            .layout
            .profiles
            .iter()
            .find(|p| p.apps.contains(&app))
            .map(|p| p.id.clone());
        match target {
            Some(t) if t == self.layout.active => {}
            Some(t) => {
                if self.auto_from.is_none() {
                    self.auto_from = Some(self.layout.active.clone());
                }
                self.switch_profile(&t);
                self.emit(HostEvent::PageChanged);
            }
            None => {
                if let Some(back) = self.auto_from.take() {
                    if self.layout.profiles.iter().any(|p| p.id == back) {
                        self.switch_profile(&back);
                        self.emit(HostEvent::PageChanged);
                    }
                }
            }
        }
    }

    fn switch_profile(&mut self, id: &str) {
        if self.layout.active == id {
            return;
        }
        self.leave_page();
        self.layout.active = id.to_string();
        self.layout.fix();
        self.dirty = true;
        self.enter_page();
    }

    fn layout_op(&mut self, op: LayoutOp) -> Result<(), String> {
        let clean = |s: &str| s.trim().chars().take(60).collect::<String>();
        match op {
            LayoutOp::GoToPage(id) => {
                if self.layout.profile().page(&id).is_none() {
                    return Err("That page is gone.".into());
                }
                self.go_to_page(&id);
            }
            LayoutOp::AddPage => {
                let n = self.layout.profile().top_pages().count() + 1;
                let page = Page::new(&format!("Page {n}"), None);
                let id = page.id.clone();
                let profile = self.layout.profile_mut();
                // After the last profile page, before the folders.
                let at = profile
                    .pages
                    .iter()
                    .rposition(|p| p.parent.is_none())
                    .map_or(0, |i| i + 1);
                profile.pages.insert(at, page);
                self.go_to_page(&id);
            }
            LayoutOp::RemovePage(id) => {
                let profile = self.layout.profile();
                let tops: Vec<String> = profile.top_pages().map(|p| p.id.clone()).collect();
                let Some(at) = tops.iter().position(|p| *p == id) else {
                    return Err("Only pages can be removed here.".into());
                };
                if tops.len() == 1 {
                    return Err("A profile needs at least one page.".into());
                }
                if profile.top_of(&profile.current) == id {
                    let next = tops[if at + 1 < tops.len() { at + 1 } else { at - 1 }].clone();
                    self.go_to_page(&next);
                }
                self.remove_pages(&id);
            }
            LayoutOp::RenamePage(id, name) => {
                let name = clean(&name);
                if name.is_empty() {
                    return Err("Give the page a name.".into());
                }
                let page = self
                    .layout
                    .profile_mut()
                    .pages
                    .iter_mut()
                    .find(|p| p.id == id)
                    .ok_or("That page is gone.")?;
                page.name = name.clone();
                // A folder's key shows its name.
                let key = self.layout.keys().iter().find(|(_, i)| {
                    i.is_builtin(builtin::FOLDER)
                        && i.settings.get("page").and_then(Value::as_str) == Some(id.as_str())
                });
                let key = key.map(|(k, _)| *k);
                let folder = self
                    .layout
                    .profiles
                    .iter_mut()
                    .flat_map(|p| p.pages.iter_mut())
                    .flat_map(|g| g.keys.values_mut())
                    .find(|i| {
                        i.is_builtin(builtin::FOLDER)
                            && i.settings.get("page").and_then(Value::as_str) == Some(id.as_str())
                    });
                if let Some(i) = folder {
                    i.settings["name"] = json!(name);
                }
                if let Some(k) = key {
                    self.redraw(k);
                }
            }
            LayoutOp::AddProfile(name) => {
                let name = clean(&name);
                let profile = Profile::new(if name.is_empty() {
                    "New profile"
                } else {
                    &name
                });
                let id = profile.id.clone();
                self.layout.profiles.push(profile);
                self.switch_profile(&id);
            }
            LayoutOp::RemoveProfile(id) => {
                if self.layout.profiles.len() == 1 {
                    return Err("You need at least one profile.".into());
                }
                let Some(at) = self.layout.profiles.iter().position(|p| p.id == id) else {
                    return Err("That profile is gone.".into());
                };
                if self.layout.active == id {
                    let next = self.layout.profiles[if at == 0 { 1 } else { at - 1 }]
                        .id
                        .clone();
                    self.switch_profile(&next);
                }
                self.layout.profiles.remove(at);
                self.stop_unused();
            }
            LayoutOp::RenameProfile(id, name) => {
                let name = clean(&name);
                if name.is_empty() {
                    return Err("Give the profile a name.".into());
                }
                let p = self
                    .layout
                    .profiles
                    .iter_mut()
                    .find(|p| p.id == id)
                    .ok_or("That profile is gone.")?;
                p.name = name;
            }
            LayoutOp::SwitchProfile(id) => {
                if !self.layout.profiles.iter().any(|p| p.id == id) {
                    return Err("That profile is gone.".into());
                }
                self.switch_profile(&id);
            }
            LayoutOp::SetProfileApps(id, apps) => {
                let mut clean_apps: Vec<String> = apps
                    .iter()
                    .map(|a| {
                        a.trim()
                            .rsplit(['\\', '/'])
                            .next()
                            .unwrap_or("")
                            .to_lowercase()
                    })
                    .filter(|a| !a.is_empty())
                    .collect();
                clean_apps.dedup();
                let p = self
                    .layout
                    .profiles
                    .iter_mut()
                    .find(|p| p.id == id)
                    .ok_or("That profile is gone.")?;
                p.apps = clean_apps;
            }
        }
        Ok(())
    }

    fn set_builtin_settings(&mut self, key: usize, mut settings: Value) {
        let Some(inst) = self.layout.keys().get(&key).cloned() else {
            return;
        };
        if inst.plugin != BUILTIN || !settings.is_object() {
            return;
        }
        if inst.action == builtin::FOLDER {
            // The page a folder opens is Agora's business, not the editor's.
            settings["page"] = inst.settings["page"].clone();
            let name = settings
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .chars()
                .take(60)
                .collect::<String>();
            if let Some(page_id) = inst.settings["page"].as_str() {
                if let Some(page) = self
                    .layout
                    .profile_mut()
                    .pages
                    .iter_mut()
                    .find(|p| p.id == page_id)
                {
                    page.name = if name.is_empty() {
                        "Folder".into()
                    } else {
                        name
                    };
                }
            }
        }
        if let Some(i) = self.layout.keys_mut().get_mut(&key) {
            i.settings = settings;
        }
        self.dirty = true;
        self.redraw(key);
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
        let mut out: Vec<ActionInfo> = builtin::DEFS
            .iter()
            .map(|d| ActionInfo {
                plugin: BUILTIN.into(),
                plugin_name: "Agora".into(),
                category: format!("Agora: {}", d.group),
                action: d.id.into(),
                name: d.name.into(),
                tooltip: d.tooltip.into(),
                icon: builtin::list_icon(d.id),
                group_icon: builtin::group_icon(d.group),
                controllers: vec![if d.id == builtin::DIAL {
                    "Encoder".into()
                } else {
                    "Keypad".into()
                }],
            })
            .collect();
        let plugins: Vec<Plugin> = self.plugins.values().cloned().collect();
        let hidden = self.layout.hidden.clone();
        for p in plugins.into_iter().filter(|p| !hidden.contains(&p.uuid)) {
            let group_rel = p
                .manifest
                .category_icon
                .clone()
                .unwrap_or_else(|| p.manifest.icon.clone());
            let group_icon = self.manifest_image(&p.uuid, &group_rel);
            for a in p
                .manifest
                .actions
                .iter()
                .filter(|a| a.visible_in_actions_list)
            {
                let mut controllers = Vec::new();
                // "Information" actions only show something; on a Stream
                // Dock they go on a key like any other.
                if a.controllers.is_empty()
                    || a.controllers
                        .iter()
                        .any(|c| c == "Keypad" || c == "Information")
                {
                    controllers.push("Keypad".to_string());
                }
                if a.controllers.iter().any(|c| c == "Encoder" || c == "Knob") {
                    controllers.push("Encoder".to_string());
                }
                if controllers.is_empty() {
                    continue;
                }
                out.push(ActionInfo {
                    plugin: p.uuid.clone(),
                    plugin_name: p.manifest.name.clone(),
                    category: p
                        .manifest
                        .category
                        .clone()
                        .unwrap_or_else(|| p.manifest.name.clone()),
                    action: a.uuid.clone(),
                    name: a.name.clone(),
                    tooltip: a.tooltip.clone(),
                    icon: self.manifest_image(&p.uuid, &a.icon),
                    group_icon: group_icon.clone(),
                    controllers,
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
                status: self
                    .status
                    .get(&p.uuid)
                    .cloned()
                    .unwrap_or_else(|| "not running".into()),
                dir: p.dir.display().to_string(),
                hidden: self.layout.hidden.contains(&p.uuid),
            })
            .collect()
    }

    fn key_info(&self, key: usize) -> Option<KeyInfo> {
        let inst = self.layout.keys().get(&key)?;
        if inst.plugin == BUILTIN {
            let def = builtin::def(&inst.action)?;
            return Some(KeyInfo {
                key,
                plugin: BUILTIN.into(),
                plugin_name: "Agora".into(),
                action: inst.action.clone(),
                name: def.name.into(),
                settings_url: None,
                builtin: true,
                settings: inst.settings.clone(),
                look: inst.look.clone(),
                hold: inst.hold.clone(),
            });
        }
        let plugin = self.plugins.get(&inst.plugin)?;
        let def = plugin.action(&inst.action)?;
        let settings_url = plugin.property_inspector(&inst.action).map(|rel| {
            let action_info = json!({
                "action": inst.action,
                "context": inst.context,
                "device": self.config.device.id,
                "payload": { "settings": inst.settings, "coordinates": self.coords(key), "controller": self.controller(key), "state": inst.state },
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
            builtin: false,
            settings: inst.settings.clone(),
            look: inst.look.clone(),
            hold: inst.hold.clone(),
        })
    }

    // ---- sockets ---------------------------------------------------------

    fn on_closed(&mut self, id: ConnId) {
        let Some(conn) = self.conns.remove(&id) else {
            return;
        };
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
        let Ok(msg) = serde_json::from_str::<Value>(text) else {
            return;
        };
        let event = msg
            .get("event")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
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
        let uuid = msg
            .get("uuid")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        match event {
            "registerPlugin"
                if self.plugins.contains_key(&uuid)
                    // A plugin that is connected already can't be taken over
                    // by a second socket using its id.
                    && !self
                        .plugin_conn
                        .get(&uuid)
                        .is_some_and(|c| self.conns.contains_key(c)) =>
            {
                if let Some(c) = self.conns.get_mut(&id) {
                    c.role = Role::Plugin(uuid.clone());
                }
                self.plugin_conn.insert(uuid.clone(), id);
                self.set_status(&uuid, "running");
                if self.device_connected {
                    let ev = json!({ "event": "deviceDidConnect", "device": self.config.device.id, "deviceInfo": self.device_info() });
                    self.send_conn(id, &ev);
                }
                let keys: Vec<usize> = self
                    .layout
                    .keys()
                    .iter()
                    .filter(|(_, i)| i.plugin == uuid)
                    .map(|(k, _)| *k)
                    .collect();
                for k in keys {
                    self.appear(k);
                }
            }
            "registerPropertyInspector" => {
                // Agora's own keys have no settings panel on the socket, so
                // nothing there can change what they run.
                let Some(key) = self
                    .key_of(&uuid)
                    .filter(|k| self.layout.keys().get(k).is_some_and(|i| i.plugin != BUILTIN))
                else {
                    return;
                };
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
        self.instance_event(key, "didReceiveSettings", json!({}))
            .map(|(_, m)| m)
    }

    fn handle_plugin(&mut self, uuid: &str, event: &str, msg: &Value) {
        let context = msg.get("context").and_then(Value::as_str).unwrap_or("");
        let payload = msg.get("payload").cloned().unwrap_or(Value::Null);
        // Only let a plugin touch its own keys.
        let key = self
            .key_of(context)
            .filter(|k| self.layout.keys().get(k).is_some_and(|i| i.plugin == uuid));
        match (event, key) {
            ("setTitle", Some(k)) => {
                let title = payload
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string);
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
                let image = payload
                    .get("image")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
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
                if let Some(inst) = self.layout.keys_mut().get_mut(&k) {
                    inst.state = s;
                    self.dirty = true;
                }
                self.redraw(k);
            }
            ("showOk", Some(k)) => self.flash(k, Overlay::Ok),
            ("showAlert" | "showAlertOnEncoder", Some(k)) => self.flash(k, Overlay::Alert),
            // A plugin asks for a profile by name.
            ("switchToProfile", _) => {
                let wanted = payload.get("profile").and_then(Value::as_str).unwrap_or("");
                let id = self
                    .layout
                    .profiles
                    .iter()
                    .find(|p| p.name.eq_ignore_ascii_case(wanted.trim()))
                    .map(|p| p.id.clone());
                match id {
                    Some(id) => {
                        self.auto_from = None;
                        self.switch_profile(&id);
                        self.emit(HostEvent::PageChanged);
                    }
                    None => self.log(uuid, format!("No profile called '{wanted}' to switch to")),
                }
            }
            ("setSettings", Some(k)) => {
                if let Some(inst) = self.layout.keys_mut().get_mut(&k) {
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
                self.layout
                    .global_settings
                    .insert(uuid.into(), payload.clone());
                self.dirty = true;
                let ev = json!({ "event": "didReceiveGlobalSettings", "payload": { "settings": payload } });
                let pis: Vec<String> = self
                    .layout
                    .keys()
                    .values()
                    .filter(|i| i.plugin == uuid)
                    .map(|i| i.context.clone())
                    .collect();
                for c in pis {
                    self.to_pi(&c, ev.clone());
                }
            }
            ("getGlobalSettings", _) => {
                let settings = self
                    .layout
                    .global_settings
                    .get(uuid)
                    .cloned()
                    .unwrap_or_else(empty_object);
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
            // Knob screens (Elgato's Stream Deck+): show the title or value
            // and the icon the plugin sends, as a face.
            ("setFeedback", Some(k)) => {
                let text = ["value", "title"]
                    .iter()
                    .find_map(|f| match payload.get(*f) {
                        Some(Value::String(t)) => Some(t.clone()),
                        Some(Value::Number(n)) => Some(n.to_string()),
                        Some(Value::Object(o)) => o.get("value").map(|v| match v {
                            Value::String(t) => t.clone(),
                            other => other.to_string(),
                        }),
                        _ => None,
                    });
                let icon = match payload.get("icon") {
                    Some(Value::String(i)) => Some(i.clone()),
                    Some(Value::Object(o)) => {
                        o.get("value").and_then(Value::as_str).map(str::to_string)
                    }
                    _ => None,
                };
                self.per_state(k, &json!({}), |inst, s| {
                    if let Some(t) = &text {
                        inst.titles.insert(s, t.clone());
                    }
                    if let Some(i) = &icon {
                        inst.images.insert(s, i.clone());
                    }
                });
                self.redraw(k);
            }
            // Elgato-only extras: accepted quietly so plugins that use them keep working.
            ("setFeedback" | "setFeedbackLayout" | "setTriggerDescription", _) => {}
            (other, _) => self.log(uuid, format!("Ignored event '{other}'")),
        }
    }

    fn handle_pi(&mut self, context: &str, event: &str, msg: &Value) {
        let Some(key) = self.key_of(context) else {
            return;
        };
        let plugin = self.layout.keys()[&key].plugin.clone();
        let payload = msg.get("payload").cloned().unwrap_or(Value::Null);
        match event {
            "setSettings" => {
                if let Some(inst) = self.layout.keys_mut().get_mut(&key) {
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
                self.layout
                    .global_settings
                    .insert(plugin.clone(), payload.clone());
                self.dirty = true;
                self.to_plugin(&plugin, json!({ "event": "didReceiveGlobalSettings", "payload": { "settings": payload } }));
            }
            "getGlobalSettings" => {
                let settings = self
                    .layout
                    .global_settings
                    .get(&plugin)
                    .cloned()
                    .unwrap_or_else(empty_object);
                self.to_pi(context, json!({ "event": "didReceiveGlobalSettings", "payload": { "settings": settings } }));
            }
            "sendToPlugin" => {
                let action = self.layout.keys()[&key].action.clone();
                self.to_plugin(
                    &plugin,
                    json!({ "action": action, "event": "sendToPlugin", "context": context, "payload": payload }),
                );
            }
            "openUrl" => self.open_url(&plugin, &payload),
            "logMessage" => {
                let m = payload.get("message").and_then(Value::as_str).unwrap_or("");
                self.log(
                    &plugin,
                    format!("(settings) {}", m.chars().take(2000).collect::<String>()),
                );
            }
            _ => {}
        }
    }

    fn per_state(&mut self, key: usize, payload: &Value, mut f: impl FnMut(&mut Instance, usize)) {
        let n_states = self
            .layout
            .keys()
            .get(&key)
            .and_then(|i| {
                self.plugins
                    .get(&i.plugin)
                    .and_then(|p| p.action(&i.action))
            })
            .map_or(1, |a| a.states.len().max(1));
        let Some(inst) = self.layout.keys_mut().get_mut(&key) else {
            return;
        };
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
            self.log(
                plugin,
                format!(
                    "Refused to open non-web address: {}",
                    url.chars().take(200).collect::<String>()
                ),
            );
        }
    }
}
