//! Elgato Stream Deck driver.
//!
//! The USB protocol is handled by the `elgato-streamdeck` crate (MPL-2.0),
//! used unchanged as a dependency. This file only fits it behind Agora's
//! [`Dock`] trait: it describes each model as a [`DeviceSpec`] and turns
//! the crate's "every key's state" reports into one press or release at a
//! time.
//!
//! Supported for now: the models that only have screen keys (Original,
//! MK.2, Mini, XL, Neo's keys). The Plus (knobs and touch strip) and the
//! Pedal come later.

use std::collections::VecDeque;
use std::time::Duration;

use elgato_streamdeck::info::{ImageMirroring, ImageMode, ImageRotation, Kind};
use elgato_streamdeck::{StreamDeck, StreamDeckError, StreamDeckInput};
use hidapi::HidApi;

use crate::dock::Dock;
use crate::keyimage::Orientation;
use crate::spec::{DeviceSpec, Family, ImageFormat, Size};
use crate::{Action, Control, Error, InputEvent, Result};

/// The Stream Deck models Agora drives today.
pub fn supported(kind: Kind) -> bool {
    !matches!(kind, Kind::Plus | Kind::PlusXl | Kind::Pedal)
}

/// Every model the crate knows, supported or not.
pub const ALL: [Kind; 17] = [
    Kind::Original,
    Kind::OriginalV2,
    Kind::Mini,
    Kind::MiniMk2,
    Kind::MiniDiscord,
    Kind::MiniMk2Module,
    Kind::Xl,
    Kind::XlV2,
    Kind::XlV2Module,
    Kind::Mk2,
    Kind::Mk2Scissor,
    Kind::Mk2Module,
    Kind::Mk2ModuleScissor,
    Kind::Neo,
    Kind::Plus,
    Kind::PlusXl,
    Kind::Pedal,
];

/// A supported model by Agora's short id ("SDMINI").
pub fn by_id(id: &str) -> Option<Kind> {
    ALL.into_iter()
        .find(|&k| supported(k) && names(k).0 == id)
}

/// Agora's short id and the name people know, per model.
fn names(kind: Kind) -> (&'static str, &'static str) {
    match kind {
        Kind::Original => ("SD", "Stream Deck"),
        Kind::OriginalV2 => ("SDV2", "Stream Deck"),
        Kind::Mini => ("SDMINI", "Stream Deck Mini"),
        Kind::MiniMk2 => ("SDMINI2", "Stream Deck Mini MK.2"),
        Kind::MiniDiscord => ("SDMINID", "Stream Deck Mini (Discord)"),
        Kind::MiniMk2Module => ("SDMINIM", "Stream Deck Mini module"),
        Kind::Xl => ("SDXL", "Stream Deck XL"),
        Kind::XlV2 => ("SDXL2", "Stream Deck XL"),
        Kind::XlV2Module => ("SDXLM", "Stream Deck XL module"),
        Kind::Mk2 => ("SDMK2", "Stream Deck MK.2"),
        Kind::Mk2Scissor => ("SDMK2S", "Stream Deck MK.2 (scissor keys)"),
        Kind::Mk2Module => ("SDMK2M", "Stream Deck MK.2 module"),
        Kind::Mk2ModuleScissor => ("SDMK2MS", "Stream Deck MK.2 module (scissor keys)"),
        Kind::Neo => ("SDNEO", "Stream Deck Neo"),
        Kind::Plus => ("SDPLUS", "Stream Deck +"),
        Kind::PlusXl => ("SDPLUSXL", "Stream Deck + XL"),
        Kind::Pedal => ("SDPEDAL", "Stream Deck Pedal"),
    }
}

/// The device type number Elgato's plugin SDK uses in `deviceInfo.type`.
fn plugin_type(kind: Kind) -> u8 {
    match kind {
        Kind::Mini | Kind::MiniMk2 | Kind::MiniDiscord | Kind::MiniMk2Module => 1,
        Kind::Xl | Kind::XlV2 | Kind::XlV2Module => 2,
        Kind::Pedal => 5,
        Kind::Plus | Kind::PlusXl => 7,
        Kind::Neo => 9,
        _ => 0,
    }
}

/// How the crate's picture format reads in Agora's terms.
fn orientation(rotation: ImageRotation, mirror: ImageMirroring) -> Orientation {
    match (rotation, mirror) {
        (ImageRotation::Rot0, ImageMirroring::None) => Orientation::Upright,
        (ImageRotation::Rot0, ImageMirroring::X) => Orientation::FlipH,
        (ImageRotation::Rot0, ImageMirroring::Y) => Orientation::FlipV,
        (ImageRotation::Rot0, ImageMirroring::Both) => Orientation::Rotate180,
        (ImageRotation::Rot90, ImageMirroring::None) => Orientation::Rotate90,
        (ImageRotation::Rot90, ImageMirroring::X) => Orientation::Transpose,
        (ImageRotation::Rot180, ImageMirroring::None) => Orientation::Rotate180,
        (ImageRotation::Rot180, ImageMirroring::Both) => Orientation::Upright,
        (ImageRotation::Rot270, ImageMirroring::None) => Orientation::Rotate270,
        // Not used by any model today.
        _ => Orientation::Upright,
    }
}

/// A Stream Deck model as the rest of Agora sees it.
pub fn spec(kind: Kind) -> DeviceSpec {
    let (id, name) = names(kind);
    let format = kind.key_image_format();
    let size = Size {
        width: format.size.0 as u32,
        height: format.size.1 as u32,
    };
    DeviceSpec {
        family: Family::StreamDeck,
        id: id.into(),
        name: name.into(),
        rows: kind.row_count() as usize,
        columns: kind.column_count() as usize,
        keys: vec![size; kind.key_count() as usize],
        buttons: 0,
        dials: 0,
        strip: Vec::new(),
        ring_leds: 0,
        orientation: orientation(format.rotation, format.mirror),
        image_format: match format.mode {
            ImageMode::BMP => ImageFormat::Bmp,
            _ => ImageFormat::Jpeg,
        },
        // Pictures go out in pages, so the crate takes any size; keep JPEGs
        // modest anyway.
        max_image: 64 * 1024,
        plugin_type: plugin_type(kind),
        vsd_device: None,
        // Tried by us on a Mini (2026-11-03): keys, plugins, brightness,
        // screen saver, and the logo coming back on quit.
        tested: kind == Kind::Mini,
        boot_logo: None,
        boot_logo_orientation: Orientation::Upright,
    }
}

fn usb(e: StreamDeckError) -> Error {
    match e {
        StreamDeckError::InvalidKeyIndex => Error::NoSuchKey(usize::MAX),
        StreamDeckError::NoScreen | StreamDeckError::UnsupportedOperation => Error::Unsupported,
        other => Error::Usb(other.to_string()),
    }
}

/// One open Stream Deck.
pub struct ElgatoDeck {
    spec: DeviceSpec,
    deck: StreamDeck,
    /// Which keys were down at the last report.
    down: Vec<bool>,
    /// Presses and releases from one report, handed out one at a time.
    pending: VecDeque<InputEvent>,
}

impl ElgatoDeck {
    /// Opens the deck with this serial number.
    pub fn connect(api: &HidApi, kind: Kind, serial: &str) -> Result<Self> {
        let deck = StreamDeck::connect(api, kind, serial).map_err(|e| {
            Error::Usb(format!(
                "{e}. Is the Elgato Stream Deck app still running? Quit it and try again."
            ))
        })?;
        Ok(ElgatoDeck {
            spec: spec(kind),
            down: vec![false; kind.key_count() as usize],
            pending: VecDeque::new(),
            deck,
        })
    }

    /// Turns a full key-state report into presses and releases.
    fn take_states(&mut self, states: &[bool]) {
        for (key, &now) in states.iter().enumerate().take(self.down.len()) {
            if now != self.down[key] {
                self.down[key] = now;
                self.pending.push_back(InputEvent {
                    control: Control::Key(key),
                    action: if now { Action::Press } else { Action::Release },
                    raw_id: key as u8,
                });
            }
        }
    }
}

impl Dock for ElgatoDeck {
    fn spec(&self) -> &DeviceSpec {
        &self.spec
    }

    fn open(&mut self) -> Result<()> {
        self.clear_all()
    }

    fn set_brightness(&mut self, pct: u8) -> Result<()> {
        self.deck.set_brightness(pct.min(100)).map_err(usb)
    }

    fn clear_all(&mut self) -> Result<()> {
        self.deck.clear_all_button_images().map_err(usb)?;
        self.deck.flush().map_err(usb)
    }

    fn set_key_image(&mut self, key: usize, image: &[u8]) -> Result<()> {
        if key >= self.spec.keys.len() {
            return Err(Error::NoSuchKey(key));
        }
        self.deck.write_image(key as u8, image).map_err(usb)?;
        self.deck.flush().map_err(usb)
    }

    /// Back to the Elgato logo.
    fn close(&mut self) -> Result<()> {
        self.deck.reset().map_err(usb)
    }

    fn read_event(&mut self, timeout_ms: i32) -> Result<Option<InputEvent>> {
        if let Some(ev) = self.pending.pop_front() {
            return Ok(Some(ev));
        }
        let timeout = Duration::from_millis(timeout_ms.max(0) as u64);
        // Knobs and the touch strip aren't driven yet.
        if let StreamDeckInput::ButtonStateChange(states) =
            self.deck.read_input(Some(timeout)).map_err(usb)?
        {
            self.take_states(&states);
        }
        Ok(self.pending.pop_front())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyimage;
    use image::{DynamicImage, Rgb, RgbImage};

    /// A picture where every pixel is different, so a wrong turn shows.
    fn sample() -> DynamicImage {
        let mut img = RgbImage::new(80, 80);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = Rgb([(x * 3) as u8, (y * 3) as u8, ((x + y) % 7 * 30) as u8]);
        }
        DynamicImage::ImageRgb8(img)
    }

    #[test]
    fn our_pictures_match_what_the_crate_would_send() {
        // Same size in, so neither side scales: the bytes must agree.
        for kind in [Kind::Mini, Kind::Original] {
            let format = kind.key_image_format();
            let (w, h) = (format.size.0 as u32, format.size.1 as u32);
            let img = sample().resize_exact(w, h, image::imageops::FilterType::Nearest);
            let theirs =
                elgato_streamdeck::images::convert_image_with_format(format, img.clone()).unwrap();
            let s = spec(kind);
            let ours = keyimage::encode(
                &img,
                w,
                h,
                s.orientation,
                s.image_format,
                s.max_image,
            )
            .unwrap();
            assert_eq!(ours, theirs, "{kind:?}");
        }
    }

    #[test]
    fn mini_spec() {
        let s = spec(Kind::Mini);
        assert_eq!((s.rows, s.columns, s.keys.len()), (2, 3, 6));
        assert_eq!(s.key_size(), 80);
        assert_eq!(s.image_format, ImageFormat::Bmp);
        assert_eq!(s.orientation, Orientation::Transpose);
        assert_eq!(s.plugin_type, 1);
        assert_eq!(s.family, Family::StreamDeck);
        assert!(s.tested && !spec(Kind::Xl).tested);
        assert!(supported(Kind::Mini) && !supported(Kind::Plus));
        assert_eq!(by_id("SDMINI"), Some(Kind::Mini));
        assert_eq!(by_id("SDPLUS"), None);
        // Ids never clash with each other or with a Stream Dock's.
        let mut ids: Vec<&str> = ALL.iter().map(|&k| names(k).0).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), ALL.len());
        assert!(ids.iter().all(|id| crate::models::by_id(id).is_none()));
    }
}
