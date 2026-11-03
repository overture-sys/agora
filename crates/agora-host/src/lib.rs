//! Agora's plugin host.
//!
//! Runs Stream Dock (and Elgato-style) plugins unchanged: it finds plugin
//! folders, starts each plugin the way its manifest asks (Node.js script,
//! HTML page or program), and speaks the Stream Dock SDK over a WebSocket on
//! 127.0.0.1. The same port serves plugin files, so settings panels
//! (Property Inspectors) and HTML plugins load from it too.
//!
//! The app talks to the host through [`HostHandle`] and gets key faces and
//! other requests back as [`HostEvent`]s.

pub mod builtin;
pub mod face;
mod host;
mod launch;
pub mod manifest;
mod server;
pub mod stats;
pub mod vsd_import;

pub use face::{Face, Overlay};
pub use host::{
    ActionInfo, DeviceDesc, DialInput, HostEvent, HostHandle, ImportReport, KeyInfo, KnownDevice, LayoutOp,
    LayoutView, Look, NameId, PageOverview, PluginInfo, BUTTON_BASE, DIAL_BASE,
};
pub use manifest::{Plugin, Source};

use std::path::PathBuf;

/// Where the host finds things.
#[derive(Debug, Clone)]
pub struct Config {
    /// Plugin folders in priority order (Agora's own first).
    pub plugin_dirs: Vec<(PathBuf, Source)>,
    /// Where the layout, settings and plugin logs are kept.
    pub data_dir: PathBuf,
    /// Node.js to run Node plugins with. `None` means "node" on PATH.
    pub node: Option<PathBuf>,
    pub device: DeviceDesc,
}
