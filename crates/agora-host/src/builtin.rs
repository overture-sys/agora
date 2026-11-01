//! Agora's own actions: pages, folders and profiles (handled by the host),
//! and system actions such as opening an app or pressing a hotkey (carried
//! out by the app). They show in the action list like a plugin's, under the
//! plugin id [`PLUGIN`], but nothing is started for them.

use base64::Engine;
use serde_json::{json, Value};

pub const PLUGIN: &str = "com.agora.builtin";

pub struct Def {
    pub id: &'static str,
    pub name: &'static str,
    pub group: &'static str,
    pub tooltip: &'static str,
    /// The icon's SVG body, drawn on a 72x72 canvas.
    icon: &'static str,
    /// Settings a new key starts with.
    pub settings: fn() -> Value,
    /// Handled by the host (navigation) rather than the app.
    pub navigation: bool,
}

pub const FOLDER: &str = "folder";
pub const BACK: &str = "back";
pub const NEXT_PAGE: &str = "next-page";
pub const PREVIOUS_PAGE: &str = "previous-page";
pub const GO_TO_PAGE: &str = "go-to-page";
pub const SWITCH_PROFILE: &str = "switch-profile";
/// Shows "page n / m"; a press goes to the next page.
pub const PAGE_INDICATOR: &str = "page-indicator";
pub const MULTI: &str = "multi";
/// Runs one of its steps at random.
pub const RANDOM: &str = "random";
/// Presses one hotkey, then the other on the next press.
pub const HOTKEY_SWITCH: &str = "hotkey-switch";
pub const TOGGLE: &str = "toggle";
/// A pause between multi-action steps; only inside multi-actions.
pub const DELAY: &str = "delay";
/// A knob: actions for turning left, turning right and pressing. Only on
/// knobs.
pub const DIAL: &str = "dial";

const STROKE: &str = r##"fill="none" stroke="#e9dcc4" stroke-width="4" stroke-linecap="round" stroke-linejoin="round""##;

pub static DEFS: &[Def] = &[
    Def {
        id: FOLDER,
        name: "Folder",
        group: "Navigation",
        tooltip: "A key that opens its own page of keys",
        icon: r##"<path d="M12 22 h17 l5 6 h26 v26 h-48 z" S/>"##,
        settings: || json!({ "page": "" }),
        navigation: true,
    },
    Def {
        id: BACK,
        name: "Back",
        group: "Navigation",
        tooltip: "Leaves a folder",
        icon: r##"<path d="M30 22 l-12 12 l12 12 M19 34 h26 a10 10 0 0 1 0 20 h-6" S/>"##,
        settings: || json!({}),
        navigation: true,
    },
    Def {
        id: NEXT_PAGE,
        name: "Next page",
        group: "Navigation",
        tooltip: "Goes to the next page",
        icon: r##"<path d="M28 20 l16 16 l-16 16" S/>"##,
        settings: || json!({}),
        navigation: true,
    },
    Def {
        id: PREVIOUS_PAGE,
        name: "Previous page",
        group: "Navigation",
        tooltip: "Goes to the page before",
        icon: r##"<path d="M44 20 l-16 16 l16 16" S/>"##,
        settings: || json!({}),
        navigation: true,
    },
    Def {
        id: GO_TO_PAGE,
        name: "Go to page",
        group: "Navigation",
        tooltip: "Jumps to a page",
        icon: r##"<rect x="18" y="14" width="36" height="44" rx="3" S/><path d="M27 28 h18 M27 37 h18 M27 46 h10" S/>"##,
        settings: || json!({ "page": 1 }),
        navigation: true,
    },
    Def {
        id: PAGE_INDICATOR,
        name: "Page indicator",
        group: "Navigation",
        tooltip: "Shows which page you're on; press it for the next page",
        icon: r##"<rect x="18" y="14" width="36" height="44" rx="3" S/><circle cx="29" cy="36" r="2" S/><circle cx="36" cy="36" r="2" S/><circle cx="43" cy="36" r="2" S/>"##,
        settings: || json!({}),
        navigation: true,
    },
    Def {
        id: SWITCH_PROFILE,
        name: "Switch profile",
        group: "Navigation",
        tooltip: "Changes to another profile, or the next or previous one",
        icon: r##"<rect x="12" y="16" width="30" height="22" rx="3" S/><rect x="30" y="34" width="30" height="22" rx="3" S/>"##,
        settings: || json!({ "profile": "" }),
        navigation: true,
    },
    Def {
        id: "open",
        name: "Open",
        group: "System",
        tooltip: "Opens an app, file or folder",
        icon: r##"<rect x="14" y="16" width="44" height="40" rx="4" S/><path d="M14 26 h44 M30 46 l12 -12 M33 34 h9 v9" S/>"##,
        settings: || json!({ "path": "", "args": "" }),
        navigation: false,
    },
    Def {
        id: "website",
        name: "Website",
        group: "System",
        tooltip: "Opens a web page",
        icon: r##"<circle cx="36" cy="36" r="21" S/><path d="M15 36 h42 M36 15 c-12 12 -12 30 0 42 c12 -12 12 -30 0 -42" S/>"##,
        settings: || json!({ "url": "https://" }),
        navigation: false,
    },
    Def {
        id: "hotkey",
        name: "Hotkey",
        group: "System",
        tooltip: "Presses a key combination, such as Ctrl+Shift+M",
        icon: r##"<rect x="10" y="22" width="52" height="30" rx="4" S/><path d="M19 31 h2 M28 31 h2 M37 31 h2 M46 31 h6 M22 43 h28" S/>"##,
        settings: || json!({ "keys": "" }),
        navigation: false,
    },
    Def {
        id: "text",
        name: "Type text",
        group: "System",
        tooltip: "Types a piece of text",
        icon: r##"<path d="M18 20 h36 M36 20 v34 M28 54 h16" S/>"##,
        settings: || json!({ "text": "", "enter": false }),
        navigation: false,
    },
    Def {
        id: "media",
        name: "Media",
        group: "System",
        tooltip: "Play or pause, skip, and volume",
        icon: r##"<path d="M16 22 v28 l20 -14 z M42 22 v28 M54 22 v28" S/>"##,
        settings: || json!({ "key": "play-pause" }),
        navigation: false,
    },
    Def {
        id: "brightness",
        name: "Dock brightness",
        group: "System",
        tooltip: "Changes the dock's screen brightness",
        icon: r##"<circle cx="36" cy="36" r="10" S/><path d="M36 12 v6 M36 54 v6 M12 36 h6 M54 36 h6 M19 19 l4 4 M49 49 l4 4 M19 53 l4 -4 M49 23 l4 -4" S/>"##,
        settings: || json!({ "mode": "up", "value": 50 }),
        navigation: false,
    },
    Def {
        id: "ring",
        name: "Light ring",
        group: "System",
        tooltip: "Turns the light ring on or off, or sets its colour",
        icon: r##"<circle cx="36" cy="36" r="20" S/><circle cx="36" cy="36" r="9" S/>"##,
        settings: || json!({ "mode": "toggle", "color": "#d9794a" }),
        navigation: false,
    },
    Def {
        id: "quick-control",
        name: "Quick control",
        group: "System",
        tooltip: "Sleep, lock, show the desktop, turn the screen off, mute the microphone or switch screens",
        icon: r##"<circle cx="36" cy="38" r="18" S/><path d="M36 14 v20" S/>"##,
        settings: || json!({ "what": "show-desktop" }),
        navigation: false,
    },
    Def {
        id: "quick-tool",
        name: "Quick tool",
        group: "System",
        tooltip: "Opens a Windows tool: Calculator, emoji panel, Task Manager, search and more",
        icon: r##"<path d="M44 14 a12 12 0 0 0 -11 17 l-19 19 l7 7 l19 -19 a12 12 0 0 0 17 -11 l-7 7 l-6 -1 l-1 -6 z" S/>"##,
        settings: || json!({ "tool": "calculator" }),
        navigation: false,
    },
    Def {
        id: "close-window",
        name: "Close window",
        group: "System",
        tooltip: "Closes the window in front",
        icon: r##"<rect x="12" y="14" width="48" height="44" rx="4" S/><path d="M12 24 h48 M28 34 l16 16 M44 34 l-16 16" S/>"##,
        settings: || json!({}),
        navigation: false,
    },
    Def {
        id: "mouse",
        name: "Mouse",
        group: "System",
        tooltip: "Clicks, scrolls or moves the mouse",
        icon: r##"<rect x="22" y="12" width="28" height="46" rx="14" S/><path d="M36 12 v14 M22 28 h28" S/>"##,
        settings: || json!({ "mode": "click", "button": "left", "double": false, "amount": 3, "x": 0, "y": 0, "relative": true }),
        navigation: false,
    },
    Def {
        id: "browser",
        name: "Browser",
        group: "System",
        tooltip: "Back, forward, refresh and tabs in your web browser",
        icon: r##"<rect x="10" y="14" width="52" height="44" rx="4" S/><path d="M10 26 h52 M26 42 l-6 -6 l6 -6 M46 42 l6 -6 l-6 -6" S/>"##,
        settings: || json!({ "what": "back" }),
        navigation: false,
    },
    Def {
        id: "password",
        name: "Password",
        group: "System",
        tooltip: "Types a password kept in Windows Credential Manager, never in Agora's files",
        icon: r##"<rect x="16" y="32" width="40" height="26" rx="4" S/><path d="M24 32 v-8 a12 12 0 0 1 24 0 v8 M36 42 v6" S/>"##,
        settings: || json!({ "name": "", "enter": false }),
        navigation: false,
    },
    Def {
        id: "sound",
        name: "Soundboard",
        group: "System",
        tooltip: "Plays a sound file, or stops all sounds",
        icon: r##"<path d="M14 30 h10 l14 -12 v36 l-14 -12 h-10 z M46 28 a10 10 0 0 1 0 16 M52 22 a18 18 0 0 1 0 28" S/>"##,
        settings: || json!({ "mode": "play", "path": "", "volume": 100 }),
        navigation: false,
    },
    Def {
        id: "udp",
        name: "Network message",
        group: "System",
        tooltip: "Sends a UDP message to a device or program on your network",
        icon: r##"<circle cx="18" cy="36" r="6" S/><circle cx="54" cy="22" r="6" S/><circle cx="54" cy="50" r="6" S/><path d="M24 33 l24 -9 M24 39 l24 9" S/>"##,
        settings: || json!({ "host": "", "port": 0, "message": "", "hex": false }),
        navigation: false,
    },
    Def {
        id: "dock-sleep",
        name: "Dock sleep",
        group: "System",
        tooltip: "Turns the dock's screen and light ring off; press any key to wake it",
        icon: r##"<path d="M46 16 a22 22 0 1 0 10 30 a17 17 0 0 1 -10 -30 z" S/>"##,
        settings: || json!({}),
        navigation: false,
    },
    Def {
        id: "cpu",
        name: "CPU",
        group: "Monitor",
        tooltip: "Shows how busy the processor is",
        icon: r##"<rect x="20" y="20" width="32" height="32" rx="3" S/><rect x="29" y="29" width="14" height="14" S/><path d="M28 14 v6 M36 14 v6 M44 14 v6 M28 52 v6 M36 52 v6 M44 52 v6 M14 28 h6 M14 36 h6 M14 44 h6 M52 28 h6 M52 36 h6 M52 44 h6" S/>"##,
        settings: || json!({}),
        navigation: false,
    },
    Def {
        id: "memory",
        name: "Memory",
        group: "Monitor",
        tooltip: "Shows how much memory (RAM) is in use",
        icon: r##"<rect x="10" y="24" width="52" height="22" rx="2" S/><path d="M19 31 v8 M29 31 v8 M39 31 v8 M49 31 v8 M16 46 v6 M26 46 v6 M36 46 v6 M46 46 v6 M56 46 v6" S/>"##,
        settings: || json!({}),
        navigation: false,
    },
    Def {
        id: "disk",
        name: "Disk",
        group: "Monitor",
        tooltip: "Shows a drive's free space or how busy it is",
        icon: r##"<rect x="12" y="22" width="48" height="28" rx="4" S/><circle cx="50" cy="36" r="3" S/><path d="M20 36 h18" S/>"##,
        settings: || json!({ "drive": "C:", "show": "space" }),
        navigation: false,
    },
    Def {
        id: "gpu",
        name: "GPU",
        group: "Monitor",
        tooltip: "Shows how busy the graphics card is",
        icon: r##"<rect x="10" y="20" width="52" height="30" rx="3" S/><circle cx="28" cy="35" r="8" S/><path d="M44 29 h10 M44 35 h10 M44 41 h10 M18 50 v6 M54 50 v6" S/>"##,
        settings: || json!({}),
        navigation: false,
    },
    Def {
        id: "gpu-memory",
        name: "GPU memory",
        group: "Monitor",
        tooltip: "Shows how much of the graphics card's memory is in use",
        icon: r##"<rect x="10" y="20" width="52" height="30" rx="3" S/><path d="M18 28 h8 v14 h-8 z M32 28 h8 v14 h-8 z M46 28 h8 v14 h-8 z M18 50 v6 M54 50 v6" S/>"##,
        settings: || json!({}),
        navigation: false,
    },
    Def {
        id: "ip",
        name: "IP address",
        group: "Monitor",
        tooltip: "Shows your public or local IP address; press to look it up again",
        icon: r##"<circle cx="36" cy="36" r="20" S/><path d="M16 36 h40 M36 16 c-10 10 -10 30 0 40 c10 -10 10 -30 0 -40" S/><path d="M26 28 h20" S/>"##,
        settings: || json!({ "which": "public" }),
        navigation: false,
    },
    Def {
        id: HOTKEY_SWITCH,
        name: "Hotkey switch",
        group: "System",
        tooltip: "Presses one hotkey, then a second one on the next press",
        icon: r##"<rect x="10" y="12" width="52" height="24" rx="4" S/><path d="M18 20 h2 M27 20 h2 M36 20 h2 M45 20 h8 M20 29 h32" S/><rect x="18" y="42" width="36" height="18" rx="9" S/><circle cx="48" cy="51" r="5" S/>"##,
        settings: || json!({ "keys": "", "keys_b": "" }),
        navigation: false,
    },
    Def {
        id: "audio-output",
        name: "Audio output",
        group: "Audio",
        tooltip: "Switches sound between your speakers, headset and other outputs",
        icon: r##"<path d="M14 40 v-6 a22 22 0 0 1 44 0 v6" S/><rect x="12" y="40" width="10" height="16" rx="3" S/><rect x="50" y="40" width="10" height="16" rx="3" S/>"##,
        settings: || json!({ "mode": "cycle", "device": "", "device_name": "" }),
        navigation: false,
    },
    Def {
        id: "app-volume",
        name: "App volume",
        group: "Audio",
        tooltip: "Turns one app's volume up or down, or mutes it",
        icon: r##"<path d="M12 30 h10 l12 -10 v32 l-12 -10 h-10 z" S/><path d="M44 28 a10 10 0 0 1 0 16 M50 22 a18 18 0 0 1 0 28" S/>"##,
        settings: || json!({ "app": "", "what": "up", "step": 10 }),
        navigation: false,
    },
    Def {
        id: "web-request",
        name: "Web request",
        group: "System",
        tooltip: "Sends a request to a web address, for Home Assistant, webhooks and smart lights",
        icon: r##"<circle cx="30" cy="38" r="16" S/><path d="M14 38 h32 M30 22 c-8 8 -8 24 0 32" S/><path d="M44 18 h14 v14 M58 18 l-14 14" S/>"##,
        settings: || json!({ "method": "POST", "url": "https://", "body": "", "content_type": "application/json" }),
        navigation: false,
    },
    Def {
        id: "run-command",
        name: "Run command",
        group: "System",
        tooltip: "Runs a PowerShell command, and can show what it prints on the key",
        icon: r##"<rect x="10" y="16" width="52" height="40" rx="4" S/><path d="M20 30 l8 6 l-8 6 M32 44 h14" S/>"##,
        settings: || json!({ "command": "", "show_output": true }),
        navigation: false,
    },
    Def {
        id: "snippets",
        name: "Snippets",
        group: "System",
        tooltip: "Shows your saved snippets beside the mouse; pick one to type it",
        icon: r##"<rect x="16" y="12" width="40" height="48" rx="4" S/><path d="M24 24 h24 M24 33 h24 M24 42 h14" S/>"##,
        settings: || json!({ "snippets": [], "enter": false }),
        navigation: false,
    },
    Def {
        id: "window",
        name: "Window",
        group: "System",
        tooltip: "Moves the window in front to the other screen, snaps it, or keeps it on top",
        icon: r##"<rect x="10" y="14" width="52" height="40" rx="4" S/><path d="M10 24 h52 M36 24 v30" S/>"##,
        settings: || json!({ "what": "next-screen" }),
        navigation: false,
    },
    Def {
        id: "clock",
        name: "Clock",
        group: "Time",
        tooltip: "Shows the time and date",
        icon: r##"<circle cx="36" cy="36" r="22" S/><path d="M36 22 v14 l9 6" S/>"##,
        settings: || json!({ "format": "24h", "seconds": false }),
        navigation: false,
    },
    Def {
        id: "timer",
        name: "Timer",
        group: "Time",
        tooltip: "Counts down; press to start or pause, hold to reset",
        icon: r##"<circle cx="36" cy="40" r="19" S/><path d="M30 14 h12 M36 14 v7 M36 40 l8 -8" S/>"##,
        settings: || json!({ "minutes": 5, "seconds": 0, "sound": "" }),
        navigation: false,
    },
    Def {
        id: "stopwatch",
        name: "Stopwatch",
        group: "Time",
        tooltip: "Counts up; press to start or pause, hold to reset",
        icon: r##"<circle cx="36" cy="40" r="19" S/><path d="M30 14 h12 M36 14 v7 M36 40 v-11 M52 22 l4 -4" S/>"##,
        settings: || json!({}),
        navigation: false,
    },
    Def {
        id: "counter",
        name: "Counter",
        group: "Time",
        tooltip: "Adds one each press; hold to reset",
        icon: r##"<rect x="12" y="18" width="48" height="36" rx="5" S/><path d="M26 30 v12 M20 36 h12 M42 30 v12" S/>"##,
        settings: || json!({ "count": 0, "step": 1, "name": "" }),
        navigation: false,
    },
    Def {
        id: MULTI,
        name: "Multi-action",
        group: "Multi-action",
        tooltip: "Runs several actions in order, with pauses if you like",
        icon: r##"<rect x="14" y="14" width="44" height="12" rx="3" S/><rect x="14" y="30" width="44" height="12" rx="3" S/><rect x="14" y="46" width="44" height="12" rx="3" S/>"##,
        settings: || json!({ "steps": [] }),
        navigation: false,
    },
    Def {
        id: RANDOM,
        name: "Random action",
        group: "Multi-action",
        tooltip: "Runs one of its actions, picked at random",
        icon: r##"<rect x="14" y="14" width="44" height="44" rx="6" S/><circle cx="26" cy="26" r="2" S/><circle cx="46" cy="26" r="2" S/><circle cx="36" cy="36" r="2" S/><circle cx="26" cy="46" r="2" S/><circle cx="46" cy="46" r="2" S/>"##,
        settings: || json!({ "steps": [] }),
        navigation: false,
    },
    Def {
        id: TOGGLE,
        name: "Toggle",
        group: "Multi-action",
        tooltip: "Runs one set of actions, then the other set on the next press",
        icon: r##"<rect x="10" y="24" width="52" height="24" rx="12" S/><circle cx="48" cy="36" r="7" S/>"##,
        settings: || json!({ "steps": [], "steps_b": [] }),
        navigation: false,
    },
    Def {
        id: DIAL,
        name: "Knob",
        group: "Knob",
        tooltip: "Runs actions when the knob turns left, turns right or is pressed",
        icon: r##"<circle cx="36" cy="36" r="15" S/><path d="M36 36 v-9 M13 45 a24 24 0 0 1 0 -18 M59 27 a24 24 0 0 1 0 18" S/>"##,
        settings: || json!({ "left": [], "right": [], "press": [] }),
        navigation: false,
    },
];

/// Steps a multi-action (or knob) may hold: Agora's own actions except
/// folders, multi-actions and knobs, plus pauses.
pub fn step_allowed(id: &str) -> bool {
    id == DELAY
        || (def(id).is_some()
            && !matches!(id, FOLDER | MULTI | TOGGLE | DIAL | RANDOM | HOTKEY_SWITCH)
            && !is_monitor(id)
            && !is_live(id))
}

/// Keys that redraw themselves as time passes or count presses.
pub fn is_live(id: &str) -> bool {
    def(id).is_some_and(|d| d.group == "Time")
}

/// Keys whose hold, unless the user gave them a hold action, resets them.
pub fn resets_on_hold(id: &str) -> bool {
    matches!(id, "timer" | "stopwatch" | "counter")
}

/// Keys that show a live reading.
pub fn is_monitor(id: &str) -> bool {
    def(id).is_some_and(|d| d.group == "Monitor")
}

/// A monitor key's face: its name at the top, the value large in the
/// middle and, when there is one, a bar for the share in use.
pub fn monitor_image(label: &str, value: &str, share: Option<f32>) -> String {
    let esc = |t: &str| {
        t.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let size = match value.chars().count() {
        0..=5 => 20,
        6..=8 => 15,
        9..=11 => 11,
        12..=15 => 9,
        _ => 8,
    };
    let label_size = if label.chars().count() > 9 { 8 } else { 10 };
    let bar = share.map_or(String::new(), |f| {
        let f = f.clamp(0.0, 1.0);
        let color = if f >= 0.9 {
            "#d9534a"
        } else if f >= 0.7 {
            "#e8bc62"
        } else {
            "#d9794a"
        };
        format!(
            r##"<rect x="10" y="54" width="52" height="6" rx="3" fill="#4a3f36"/><rect x="10" y="54" width="{:.1}" height="6" rx="3" fill="{color}"/>"##,
            52.0 * f
        )
    });
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="72" height="72" viewBox="0 0 72 72"><rect width="72" height="72" fill="#1d1915"/><text x="36" y="18" font-family="sans-serif" font-size="{label_size}" font-weight="600" letter-spacing="0.5" text-anchor="middle" fill="#c9b79c">{}</text><text x="36" y="{}" font-family="sans-serif" font-size="{size}" font-weight="700" text-anchor="middle" fill="#f3ecdf">{}</text>{bar}</svg>"##,
        esc(label),
        if share.is_some() { 44 } else { 46 },
        esc(value)
    );
    format!(
        "data:image/svg+xml;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(svg)
    )
}

pub fn def(id: &str) -> Option<&'static Def> {
    DEFS.iter().find(|d| d.id == id)
}

/// The action's icon for a key, as a data URI on a dark tile. It is drawn
/// a little small and high, so a title fits underneath.
pub fn icon(id: &str) -> Option<String> {
    draw(id, "translate(13.5 5) scale(0.625)")
}

/// The action's icon full size, for the action list.
pub fn list_icon(id: &str) -> Option<String> {
    draw(id, "")
}

fn draw(id: &str, transform: &str) -> Option<String> {
    let d = def(id)?;
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="72" height="72" viewBox="0 0 72 72"><rect width="72" height="72" fill="#2a2420"/><g transform="{transform}">{}</g></svg>"##,
        d.icon.replace(" S/>", &format!(" {STROKE}/>"))
    );
    Some(format!(
        "data:image/svg+xml;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(svg)
    ))
}

/// The icon for a key in a given state: a Toggle shows which set runs next.
pub fn icon_for(id: &str, state: usize) -> Option<String> {
    let uri = icon(id)?;
    if !matches!(id, TOGGLE | HOTKEY_SWITCH) || state == 0 {
        return Some(uri);
    }
    let b64 = uri.strip_prefix("data:image/svg+xml;base64,")?;
    let svg =
        String::from_utf8(base64::engine::general_purpose::STANDARD.decode(b64).ok()?).ok()?;
    let svg = svg.replace(r#"cx="48""#, r#"cx="24""#);
    Some(format!(
        "data:image/svg+xml;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(svg)
    ))
}

/// The group icon for the action list.
pub fn group_icon(group: &str) -> Option<String> {
    list_icon(match group {
        "Navigation" => FOLDER,
        "Multi-action" => MULTI,
        "Knob" => DIAL,
        "Monitor" => "cpu",
        "Time" => "clock",
        "Audio" => "app-volume",
        _ => "open",
    })
}

/// The title a key shows unless the user set one.
pub fn default_title(id: &str, settings: &Value) -> String {
    let s = |k: &str| {
        settings
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    match id {
        FOLDER => s("name"),
        BACK => "Back".into(),
        GO_TO_PAGE => format!(
            "Page {}",
            settings.get("page").and_then(Value::as_u64).unwrap_or(1)
        ),
        "media" => match s("key").as_str() {
            "next" => "Next",
            "previous" => "Previous",
            "volume-up" => "Vol +",
            "volume-down" => "Vol -",
            "mute" => "Mute",
            _ => "Play",
        }
        .into(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_are_valid_svg() {
        for d in DEFS {
            let uri = icon(d.id).unwrap();
            let b64 = uri.strip_prefix("data:image/svg+xml;base64,").unwrap();
            let svg = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .unwrap();
            resvg::usvg::Tree::from_data(&svg, &Default::default())
                .unwrap_or_else(|e| panic!("{}: {e}", d.id));
        }
    }
}
