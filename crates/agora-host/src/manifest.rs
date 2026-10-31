//! Reads plugin folders (`<uuid>.sdPlugin/manifest.json`) as described by
//! the Stream Dock SDK. Field names are PascalCase, as in the SDK.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub category_icon: Option<String>,
    #[serde(default)]
    pub code_path: Option<String>,
    #[serde(default)]
    pub code_path_win: Option<String>,
    #[serde(default)]
    pub code_path_mac: Option<String>,
    #[serde(default)]
    pub property_inspector_path: Option<String>,
    #[serde(default, alias = "Node.js")]
    pub nodejs: Option<NodeSpec>,
    #[serde(default)]
    pub applications_to_monitor: Option<AppsToMonitor>,
    #[serde(default)]
    pub actions: Vec<ActionDef>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NodeSpec {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub debug: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AppsToMonitor {
    #[serde(default)]
    pub windows: Vec<String>,
    #[serde(default)]
    pub mac: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ActionDef {
    #[serde(rename = "UUID")]
    pub uuid: String,
    pub name: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub tooltip: String,
    #[serde(default)]
    pub states: Vec<StateDef>,
    #[serde(default)]
    pub property_inspector_path: Option<String>,
    #[serde(default)]
    pub controllers: Vec<String>,
    #[serde(default = "yes")]
    pub visible_in_actions_list: bool,
    #[serde(default = "yes")]
    pub user_title_enabled: bool,
    #[serde(default)]
    pub settings: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StateDef {
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub show_title: Option<bool>,
    #[serde(default)]
    pub title_color: Option<String>,
    #[serde(default)]
    pub title_alignment: Option<String>,
    #[serde(default)]
    pub font_family: Option<String>,
    #[serde(default)]
    pub font_style: Option<String>,
    #[serde(default)]
    pub font_size: Option<serde_json::Value>,
    #[serde(default)]
    pub font_underline: Option<bool>,
}

fn yes() -> bool {
    true
}

/// How a plugin's code runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A script run by Node.js (manifest asks for `Nodejs`, or code is `.js`).
    Node(PathBuf),
    /// An HTML page run in a hidden browser view.
    Html(PathBuf),
    /// A program started with `-port -pluginUUID -registerEvent -info`.
    Exe(PathBuf),
}

/// Where a plugin was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Agora's own plugin folder.
    Agora,
    /// VSD Craft's plugin folder, used read-only.
    VsdCraft,
}

#[derive(Debug, Clone)]
pub struct Plugin {
    pub uuid: String,
    pub dir: PathBuf,
    pub manifest: Manifest,
    pub source: Source,
}

impl Plugin {
    /// Reads `dir/manifest.json`. The plugin id is the folder name without `.sdPlugin`.
    pub fn load(dir: &Path, source: Source) -> Result<Self, String> {
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("bad folder name")?;
        let uuid = name
            .strip_suffix(".sdPlugin")
            .ok_or("not a .sdPlugin folder")?
            .to_string();
        // Some plugins come zipped one folder too deep: the manifest sits in
        // a single folder inside. That inner folder is the plugin then.
        let dir = &if dir.join("manifest.json").is_file() {
            dir.to_path_buf()
        } else {
            nested_plugin_dir(dir).ok_or_else(|| format!("{name}: no manifest.json"))?
        };
        let text = std::fs::read_to_string(dir.join("manifest.json"))
            .map_err(|e| format!("{name}: {e}"))?;
        // Some manifests start with a UTF-8 byte order mark.
        let text = text.trim_start_matches('\u{feff}');
        let manifest: Manifest = serde_json::from_str(text).map_err(|e| format!("{name}: {e}"))?;
        Ok(Self {
            uuid,
            dir: dir.to_path_buf(),
            manifest,
            source,
        })
    }

    pub fn action(&self, uuid: &str) -> Option<&ActionDef> {
        self.manifest.actions.iter().find(|a| a.uuid == uuid)
    }

    /// The code to run on this OS, and how to run it.
    pub fn kind(&self) -> Option<Kind> {
        let m = &self.manifest;
        let rel = if cfg!(windows) {
            m.code_path_win.as_ref().or(m.code_path.as_ref())
        } else if cfg!(target_os = "macos") {
            m.code_path_mac.as_ref().or(m.code_path.as_ref())
        } else {
            m.code_path
                .as_ref()
                .or(m.code_path_win.as_ref())
                .or(m.code_path_mac.as_ref())
        }?;
        let path = self.dir.join(rel);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        Some(match ext.as_str() {
            "html" | "htm" => Kind::Html(path),
            "js" | "cjs" | "mjs" => Kind::Node(path),
            _ if m.nodejs.is_some() => Kind::Node(path),
            _ => Kind::Exe(path),
        })
    }

    /// Settings panel page for an action, relative to the plugin folder.
    pub fn property_inspector(&self, action: &str) -> Option<String> {
        self.action(action)
            .and_then(|a| a.property_inspector_path.clone())
            .or_else(|| self.manifest.property_inspector_path.clone())
    }
}

/// The one folder inside `dir` that holds a manifest.json, if there is
/// exactly one.
fn nested_plugin_dir(dir: &Path) -> Option<PathBuf> {
    let mut found = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("manifest.json").is_file());
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

/// Finds plugins in `dirs`, in order. A plugin id found earlier wins, so put
/// Agora's own folder first. Folders that fail to load are reported, not fatal.
pub fn discover(dirs: &[(PathBuf, Source)]) -> (Vec<Plugin>, Vec<String>) {
    let mut plugins: Vec<Plugin> = Vec::new();
    let mut problems = Vec::new();
    for (dir, source) in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            let is_plugin = path.is_dir()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("sdPlugin"));
            if !is_plugin {
                continue;
            }
            match Plugin::load(&path, *source) {
                Ok(p) if plugins.iter().any(|q| q.uuid == p.uuid) => {}
                Ok(p) => plugins.push(p),
                Err(e) => problems.push(e),
            }
        }
    }
    (plugins, problems)
}

/// VSD Craft's plugin folder on Windows: `%APPDATA%\HotSpot\StreamDock\plugins`.
pub fn vsd_craft_plugin_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| {
        PathBuf::from(a)
            .join("HotSpot")
            .join("StreamDock")
            .join("plugins")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_plugin(root: &Path, uuid: &str, manifest: &str) -> PathBuf {
        let dir = root.join(format!("{uuid}.sdPlugin"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), manifest).unwrap();
        dir
    }

    const NODE_MANIFEST: &str = r#"{
        "SDKVersion": 1, "Name": "Demo", "Version": "1.0", "Icon": "i",
        "CodePathWin": "plugin/index.js", "CodePath": "plugin/index.js",
        "PropertyInspectorPath": "pi/index.html",
        "Nodejs": {"Version": "20"},
        "Actions": [{"UUID": "com.demo.a", "Name": "A", "States": [{"Image": "a.png", "TitleAlignment": "bottom", "FontSize": "9"}]}]
    }"#;

    #[test]
    fn reads_manifest_and_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = write_plugin(tmp.path(), "com.demo", &format!("\u{feff}{NODE_MANIFEST}"));
        let p = Plugin::load(&dir, Source::Agora).unwrap();
        assert_eq!(p.uuid, "com.demo");
        assert_eq!(
            p.manifest.actions[0].states[0].title_alignment.as_deref(),
            Some("bottom")
        );
        assert!(matches!(p.kind(), Some(Kind::Node(_))));
        assert_eq!(
            p.property_inspector("com.demo.a").as_deref(),
            Some("pi/index.html")
        );
        assert!(p.manifest.actions[0].visible_in_actions_list);
    }

    #[test]
    fn html_and_exe_kinds() {
        let tmp = tempfile::tempdir().unwrap();
        let html = write_plugin(
            tmp.path(),
            "com.h",
            r#"{"Name":"H","CodePath":"index.html","Actions":[]}"#,
        );
        let exe = write_plugin(
            tmp.path(),
            "com.e",
            r#"{"Name":"E","CodePath":"bin/plugin.exe","Actions":[]}"#,
        );
        assert!(matches!(
            Plugin::load(&html, Source::Agora).unwrap().kind(),
            Some(Kind::Html(_))
        ));
        assert!(matches!(
            Plugin::load(&exe, Source::Agora).unwrap().kind(),
            Some(Kind::Exe(_))
        ));
    }

    #[test]
    fn own_folder_wins_over_vsd_craft() {
        let own = tempfile::tempdir().unwrap();
        let vsd = tempfile::tempdir().unwrap();
        write_plugin(own.path(), "com.demo", NODE_MANIFEST);
        write_plugin(vsd.path(), "com.demo", NODE_MANIFEST);
        write_plugin(vsd.path(), "com.broken", "{ not json");
        let (plugins, problems) = discover(&[
            (own.path().into(), Source::Agora),
            (vsd.path().into(), Source::VsdCraft),
        ]);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].source, Source::Agora);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn a_plugin_one_folder_too_deep_is_found() {
        let root = tempfile::tempdir().unwrap();
        let outer = root.path().join("com.example.deep.sdPlugin");
        let inner = outer.join("com.example.deep.sdPlugin");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(
            inner.join("manifest.json"),
            r#"{"Name":"Deep","CodePath":"index.html","Actions":[]}"#,
        )
        .unwrap();
        let (plugins, problems) = discover(&[(root.path().to_path_buf(), Source::Agora)]);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(plugins[0].uuid, "com.example.deep");
        assert_eq!(plugins[0].dir, inner);
    }
}
