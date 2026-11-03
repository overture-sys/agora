//! What the rest of Agora needs to know about a device, whatever family it
//! comes from: its grid, the size of each screen, its buttons and knobs,
//! and how pictures must be prepared for it.
//!
//! Each family keeps its own tables (the Stream Dock ones are in
//! [`crate::models`]) and turns an entry into a [`DeviceSpec`]. The app and
//! the plugin host only ever look at the spec.

use crate::keyimage::{self, DynamicImage, Orientation};
use crate::Result;

/// The maker's line of devices a driver speaks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    /// MiraBox / VSD Stream Dock (M18, N3, N4, 293...).
    StreamDock,
    /// Elgato Stream Deck. No driver yet; reserved so the spec, the host and
    /// the layouts already have a place for it.
    StreamDeck,
}

impl Family {
    pub fn name(self) -> &'static str {
        match self {
            Family::StreamDock => "Stream Dock",
            Family::StreamDeck => "Stream Deck",
        }
    }
}

/// How a device wants its pictures encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageFormat {
    #[default]
    Jpeg,
    /// Plain 24-bit BMP (older Stream Decks such as the Mini).
    Bmp,
}

/// One screen's size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// Everything about a device model that isn't protocol detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSpec {
    pub family: Family,
    /// Short model id ("M18", "N4"...). Stable; used in settings.
    pub id: String,
    /// The name people know it by ("Stream Dock M18").
    pub name: String,
    pub rows: usize,
    pub columns: usize,
    /// Each screen key in grid order (top left first, row by row).
    pub keys: Vec<Size>,
    /// Buttons without a screen, left to right.
    pub buttons: usize,
    pub dials: usize,
    /// Touch strip segments, one per knob (the N4). Empty if there is none.
    pub strip: Vec<Size>,
    /// Light ring or side lights.
    pub ring_leds: usize,
    /// How pictures are turned before they are sent.
    pub orientation: Orientation,
    pub image_format: ImageFormat,
    /// The largest encoded picture the device takes, in bytes.
    pub max_image: usize,
    /// The device type number Stream Deck style plugins see in
    /// `deviceInfo.type`. Stream Docks report 0, as VSD Craft does.
    pub plugin_type: u8,
    /// VSD Craft's DeviceUUID for profiles made for this model, if known.
    pub vsd_device: Option<String>,
    /// Tried on a real device by us.
    pub tested: bool,
}

impl DeviceSpec {
    pub fn key_count(&self) -> usize {
        self.rows * self.columns
    }

    /// The largest key picture, for previews.
    pub fn key_size(&self) -> u32 {
        self.keys.iter().map(|s| s.width).max().unwrap_or(64)
    }

    pub fn has_strip(&self) -> bool {
        !self.strip.is_empty()
    }

    /// Encodes a picture for one screen of this device: scaled to fill it,
    /// turned, and in the device's format and size limit.
    pub fn encode(&self, img: &DynamicImage, screen: Size) -> Result<Vec<u8>> {
        keyimage::encode(
            img,
            screen.width,
            screen.height,
            self.orientation,
            self.image_format,
            self.max_image,
        )
    }
}
