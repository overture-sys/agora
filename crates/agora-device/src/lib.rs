//! Agora device layer.
//!
//! Every driver offers the same [`Dock`] trait and describes its device
//! with a [`DeviceSpec`], so the app and the plugin host never see which
//! family a device comes from. Drivers talk through a [`Transport`] (HID
//! output, input and feature reports).
//!
//! Families:
//! - Stream Dock: one driver ([`StreamDock`]) for every model; [`models`]
//!   holds what differs between models.
//! - Stream Deck: not yet; [`spec::Family::StreamDeck`] reserves its place.
//!
//! [`worker::DockWorker`] runs any driver on its own thread, keeps the
//! device awake if it needs that, and turns raw reports into
//! [`InputEvent`]s.
//!
//! Protocol notes come from our own M18 test (2026-10-29) and MIT /
//! public-domain sources only. No OpenStreamDeck (GPL) code is used.

pub mod dock;
pub mod glyphs;
pub mod keyimage;
pub mod models;
pub mod spec;
pub mod streamdock;
pub mod transport;
pub mod worker;

#[cfg(feature = "hid")]
pub mod discover;

pub use dock::Dock;
pub use models::Model;
pub use spec::{DeviceSpec, Family, ImageFormat, Size};
pub use streamdock::StreamDock;
pub use transport::Transport;

/// Errors from the device layer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("USB error: {0}")]
    Usb(String),
    #[error("payload is {len} bytes; the limit is {max}")]
    TooLong { len: usize, max: usize },
    #[error("key {0} is outside the device's grid")]
    NoSuchKey(usize),
    #[error("image error: {0}")]
    Image(String),
    #[error("the device was disconnected")]
    Disconnected,
    #[error("this device doesn't support that")]
    Unsupported,
}

pub type Result<T> = std::result::Result<T, Error>;

/// An RGB colour for a light ring LED.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// A physical control that produced an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Control {
    /// A screen key, numbered from 0 at the top left, row by row.
    Key(usize),
    /// A button without a screen (the M18's round buttons), from the left.
    Button(usize),
    /// A knob, from the left.
    Dial(usize),
    /// A tap on one segment of the touch strip (the N4), from the left.
    Touch(usize),
    /// The touch strip as a whole, for swipes.
    Strip,
    /// A control id the driver doesn't know yet. Logged so we can map it.
    Unknown(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Press,
    Release,
    /// A knob turned one step: -1 to the left, 1 to the right.
    Turn(i8),
    /// A swipe along the touch strip: -1 to the left, 1 to the right.
    Swipe(i8),
}

/// Something the user did on the dock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputEvent {
    pub control: Control,
    pub action: Action,
    /// The raw id byte from the report, for logging and mapping.
    pub raw_id: u8,
}

/// How a model lays out its controls, in short.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub rows: usize,
    pub columns: usize,
    /// The largest key picture, in pixels (square).
    pub key_size: u32,
    pub buttons: usize,
    pub dials: usize,
    /// Knobs have a strip screen above them (the N4).
    pub strip: bool,
    pub ring_leds: usize,
}

impl Layout {
    pub fn of(s: &DeviceSpec) -> Self {
        Layout {
            rows: s.rows,
            columns: s.columns,
            key_size: s.key_size(),
            buttons: s.buttons,
            dials: s.dials,
            strip: s.has_strip(),
            ring_leds: s.ring_leds,
        }
    }

    pub fn key_count(&self) -> usize {
        self.rows * self.columns
    }
}
