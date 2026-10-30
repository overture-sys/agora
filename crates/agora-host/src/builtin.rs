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
pub const MULTI: &str = "multi";
pub const TOGGLE: &str = "toggle";
/// A pause between multi-action steps; only inside multi-actions.
pub const DELAY: &str = "delay";

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
        id: SWITCH_PROFILE,
        name: "Switch profile",
        group: "Navigation",
        tooltip: "Changes to another profile",
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
        id: MULTI,
        name: "Multi-action",
        group: "Multi-action",
        tooltip: "Runs several actions in order, with pauses if you like",
        icon: r##"<rect x="14" y="14" width="44" height="12" rx="3" S/><rect x="14" y="30" width="44" height="12" rx="3" S/><rect x="14" y="46" width="44" height="12" rx="3" S/>"##,
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
];

/// Steps a multi-action may hold: Agora's own actions except folders and
/// other multi-actions, plus pauses.
pub fn step_allowed(id: &str) -> bool {
    id == DELAY || (def(id).is_some() && !matches!(id, FOLDER | MULTI | TOGGLE))
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
    if id != TOGGLE || state == 0 {
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
