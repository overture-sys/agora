//! Agora device layer.
//!
//! One driver ([`StreamDock`]) speaks the USB protocol for every model over
//! a [`Transport`]; [`models`] holds what differs between models.
//! [`worker::DockWorker`] runs a driver on its own thread,
//! keeps the dock awake, and turns raw reports into [`InputEvent`]s.
//!
//! Protocol notes come from our own M18 test (2026-10-29) and MIT /
//! public-domain sources only. No OpenStreamDeck (GPL) code is used.

pub mod glyphs;
pub mod keyimage;
pub mod models;
pub mod streamdock;
pub mod transport;
pub mod worker;

#[cfg(feature = "hid")]
pub mod discover;

pub use models::Model;
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
    pub fn of(m: &Model) -> Self {
        Layout {
            rows: m.rows,
            columns: m.columns,
            key_size: m.key_size(),
            buttons: m.buttons.len(),
            dials: m.dials.len(),
            strip: m.strip.is_some(),
            ring_leds: m.ring_leds,
        }
    }

    pub fn key_count(&self) -> usize {
        self.rows * self.columns
    }
}
