//! Agora device layer.
//!
//! Each supported model has a driver that speaks its USB protocol over a
//! [`Transport`]. [`worker::DockWorker`] runs a driver on its own thread,
//! keeps the dock awake, and turns raw reports into [`InputEvent`]s.
//!
//! Protocol notes come from our own M18 test (2026-10-29) and MIT /
//! public-domain sources only. No OpenStreamDeck (GPL) code is used.

pub mod glyphs;
pub mod keyimage;
pub mod m18;
pub mod transport;
pub mod worker;

#[cfg(feature = "hid")]
pub mod discover;

pub use m18::M18;
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
}

pub type Result<T> = std::result::Result<T, Error>;

/// An RGB colour for a light ring LED.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// One of the round buttons under the M18's screen keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Button {
    Left,
    Middle,
    Right,
}

/// A physical control that produced an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Control {
    /// A screen key, numbered from 0 at the top left, row by row.
    Key(usize),
    Button(Button),
    /// A control id the driver doesn't know yet. Logged so we can map it.
    Unknown(u8),
}

/// A press or release reported by the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputEvent {
    pub control: Control,
    pub pressed: bool,
    /// The raw id byte from the report, for logging and mapping.
    pub raw_id: u8,
}

/// How a model lays out its controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub rows: usize,
    pub columns: usize,
    /// Key image size in pixels (square).
    pub key_size: u32,
    pub buttons: usize,
    pub ring_leds: usize,
}

impl Layout {
    pub fn key_count(&self) -> usize {
        self.rows * self.columns
    }
}
