//! The Stream Dock models Agora knows, as data: USB ids, where each key's
//! picture goes, which report ids belong to which control, and how pictures
//! are turned.
//!
//! The M18 entry comes from our own tests on Devin's M18. The others come
//! from Bitfocus Companion's MIT-licensed Mirabox driver
//! (bitfocus/companion-surface-mirabox-stream-dock, src/models) and are not
//! yet tried on real hardware by us; [`Model::tested`] says which is which.

use crate::keyimage::Orientation;
use crate::spec::{DeviceSpec, Family, ImageFormat, Size};

/// Where one picture goes: the image id the dock wants, and its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screen {
    pub id: u8,
    pub width: u32,
    pub height: u32,
}

/// The boot logo: the full-screen picture the dock keeps in its own memory
/// and shows when it starts, and whenever no app is driving it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootLogo {
    pub width: u32,
    pub height: u32,
    /// How the picture is turned before it is sent.
    pub orientation: Orientation,
}

/// The report ids of one knob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dial {
    pub press: u8,
    pub left: u8,
    pub right: u8,
}

/// A touch strip split into one screen per knob (the N4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Strip {
    /// One per knob, left to right.
    pub segments: &'static [Screen],
    /// Tap ids, one per segment.
    pub taps: &'static [u8],
    pub swipe_left: u8,
    pub swipe_right: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    /// Short name, used as the device id plugins see ("M18", "N4"...).
    pub id: &'static str,
    pub name: &'static str,
    /// (vendor id, product id) pairs.
    pub usb: &'static [(u16, u16)],
    pub rows: usize,
    pub columns: usize,
    /// Each screen key in grid order (top left first, row by row).
    pub keys: &'static [Screen],
    /// The report id each screen key sends, in the same order.
    pub key_inputs: &'static [u8],
    /// Controls that only report a press, never a release. The driver adds
    /// the release itself.
    pub press_only: &'static [u8],
    /// Plain buttons without a screen, left to right.
    pub buttons: &'static [u8],
    pub dials: &'static [Dial],
    pub strip: Option<Strip>,
    /// How pictures are turned before they are sent.
    pub orientation: Orientation,
    /// Output report length.
    pub packet: usize,
    /// Light ring or side lights.
    pub ring_leds: usize,
    /// VSD Craft's DeviceUUID for profiles made for this model, if known.
    pub vsd_device: Option<&'static str>,
    /// Tried on a real dock by us.
    pub tested: bool,
    /// Its boot logo, if Agora may replace it. The logo is written to the
    /// dock's own memory, and MiraBox warns that a picture of the wrong size
    /// or turn can brick a dock, so a model only gets one here once its
    /// exact canvas is known. Sizes from MiraBox's MIT StreamDock-Device-SDK
    /// (Python-SDK `touchscreen_image_format`); for models not yet enabled:
    /// 293V3 / N4 / N4 Pro 800x480 turned 180, 293S V3 854x480 turned 90,
    /// XL 1024x600 turned 180, N3 320x240 turned -90 (older 293 and 293S
    /// take raw pixels instead of JPEG).
    pub boot_logo: Option<BootLogo>,
}

impl Model {
    pub fn key_count(&self) -> usize {
        self.rows * self.columns
    }

    /// The largest key picture, for previews.
    pub fn key_size(&self) -> u32 {
        self.keys.iter().map(|s| s.width).max().unwrap_or(64)
    }

    pub fn matches(&self, vid: u16, pid: u16) -> bool {
        self.usb.contains(&(vid, pid))
    }

    /// This model as the rest of Agora sees it.
    pub fn spec(&self) -> DeviceSpec {
        let size = |s: &Screen| Size {
            width: s.width,
            height: s.height,
        };
        DeviceSpec {
            boot_logo: self.boot_logo.map(|l| Size {
                width: l.width,
                height: l.height,
            }),
            boot_logo_orientation: self
                .boot_logo
                .map(|l| l.orientation)
                .unwrap_or_default(),
            family: Family::StreamDock,
            id: self.id.into(),
            name: self.name.into(),
            rows: self.rows,
            columns: self.columns,
            keys: self.keys.iter().map(size).collect(),
            buttons: self.buttons.len(),
            dials: self.dials.len(),
            strip: self
                .strip
                .map(|s| s.segments.iter().map(size).collect())
                .unwrap_or_default(),
            ring_leds: self.ring_leds,
            orientation: self.orientation,
            image_format: ImageFormat::Jpeg,
            max_image: crate::streamdock::MAX_IMAGE,
            plugin_type: 0,
            vsd_device: self.vsd_device.map(str::to_owned),
            tested: self.tested,
        }
    }
}

const fn sq(id: u8, size: u32) -> Screen {
    Screen {
        id,
        width: size,
        height: size,
    }
}

/// Three rows of five: image ids count from the bottom row, key ids from
/// the top (M18, M18V3, 293V3).
macro_rules! grid15 {
    ($s:expr) => {
        &[
            sq(0x0b, $s),
            sq(0x0c, $s),
            sq(0x0d, $s),
            sq(0x0e, $s),
            sq(0x0f, $s),
            sq(0x06, $s),
            sq(0x07, $s),
            sq(0x08, $s),
            sq(0x09, $s),
            sq(0x0a, $s),
            sq(0x01, $s),
            sq(0x02, $s),
            sq(0x03, $s),
            sq(0x04, $s),
            sq(0x05, $s),
        ]
    };
}

const INPUTS_1_TO_15: &[u8] = &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// The 293S's 15 keys plus its three small side keys on the right; image
/// and key ids are the same.
macro_rules! grid293s {
    ($s:expr) => {
        &[
            sq(0x0d, $s),
            sq(0x0a, $s),
            sq(0x07, $s),
            sq(0x04, $s),
            sq(0x01, $s),
            sq(0x10, 80),
            sq(0x0e, $s),
            sq(0x0b, $s),
            sq(0x08, $s),
            sq(0x05, $s),
            sq(0x02, $s),
            sq(0x11, 80),
            sq(0x0f, $s),
            sq(0x0c, $s),
            sq(0x09, $s),
            sq(0x06, $s),
            sq(0x03, $s),
            sq(0x12, 80),
        ]
    };
}

const INPUTS_293S: &[u8] = &[
    0x0d, 0x0a, 0x07, 0x04, 0x01, 0x10, 0x0e, 0x0b, 0x08, 0x05, 0x02, 0x11, 0x0f, 0x0c, 0x09, 0x06,
    0x03, 0x12,
];

const BASE: Model = Model {
    id: "",
    name: "",
    usb: &[],
    rows: 3,
    columns: 5,
    keys: grid15!(64),
    key_inputs: INPUTS_1_TO_15,
    press_only: &[],
    buttons: &[],
    dials: &[],
    strip: None,
    orientation: Orientation::Upright,
    packet: 1024,
    ring_leds: 0,
    vsd_device: None,
    tested: false,
    boot_logo: None,
};

pub const M18: Model = Model {
    id: "M18",
    name: "Stream Dock M18",
    usb: &[(0x5548, 0x1000)],
    // Confirmed on Devin's M18: left 0x25, middle 0x30, right 0x31.
    buttons: &[0x25, 0x30, 0x31],
    ring_leds: 24,
    vsd_device: Some("VSDM18"),
    tested: true,
    // 480x272 JPEG, upright: MiraBox SDK StreamDockM18.touchscreen_image_format.
    boot_logo: Some(BootLogo {
        width: 480,
        height: 272,
        orientation: Orientation::Upright,
    }),
    ..BASE
};

pub const M18V3: Model = Model {
    id: "M18V3",
    name: "Stream Dock M18 V3",
    usb: &[(0x6603, 0x1012)],
    ..M18
};

pub const D293V3: Model = Model {
    id: "293V3",
    name: "Stream Dock 293V3",
    usb: &[(0x6603, 0x1005), (0x6603, 0x1006)],
    keys: grid15!(112),
    orientation: Orientation::Rotate180,
    ..BASE
};

pub const N3: Model = Model {
    id: "N3",
    name: "Stream Dock N3",
    usb: &[(0x6602, 0x1003), (0x6603, 0x1003), (0x5548, 0x1001)],
    rows: 2,
    columns: 3,
    keys: &[
        sq(1, 64),
        sq(2, 64),
        sq(3, 64),
        sq(4, 64),
        sq(5, 64),
        sq(6, 64),
    ],
    key_inputs: &[1, 2, 3, 4, 5, 6],
    buttons: &[0x25, 0x30, 0x31],
    dials: &[
        Dial {
            press: 0x35,
            left: 0x50,
            right: 0x51,
        },
        Dial {
            press: 0x33,
            left: 0x90,
            right: 0x91,
        },
        Dial {
            press: 0x34,
            left: 0x60,
            right: 0x61,
        },
    ],
    ..BASE
};

pub const N4: Model = Model {
    id: "N4",
    name: "Stream Dock N4",
    usb: &[(0x6602, 0x1001), (0x6603, 0x1007), (0x5548, 0x1021)],
    rows: 2,
    columns: 5,
    keys: &[
        sq(0x0b, 112),
        sq(0x0c, 112),
        sq(0x0d, 112),
        sq(0x0e, 112),
        sq(0x0f, 112),
        sq(0x06, 112),
        sq(0x07, 112),
        sq(0x08, 112),
        sq(0x09, 112),
        sq(0x0a, 112),
    ],
    key_inputs: &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
    press_only: &[0x40, 0x41, 0x42, 0x43, 0x37, 0x35, 0x33, 0x36],
    dials: &[
        Dial {
            press: 0x37,
            left: 0xa0,
            right: 0xa1,
        },
        Dial {
            press: 0x35,
            left: 0x50,
            right: 0x51,
        },
        Dial {
            press: 0x33,
            left: 0x90,
            right: 0x91,
        },
        Dial {
            press: 0x36,
            left: 0x70,
            right: 0x71,
        },
    ],
    strip: Some(Strip {
        segments: &[
            Screen {
                id: 1,
                width: 176,
                height: 124,
            },
            Screen {
                id: 2,
                width: 176,
                height: 124,
            },
            Screen {
                id: 3,
                width: 176,
                height: 124,
            },
            Screen {
                id: 4,
                width: 176,
                height: 124,
            },
        ],
        taps: &[0x40, 0x41, 0x42, 0x43],
        swipe_left: 0x38,
        swipe_right: 0x39,
    }),
    orientation: Orientation::Rotate180,
    ..BASE
};

pub const D293S: Model = Model {
    id: "293S",
    name: "Stream Dock 293S",
    usb: &[(0x6602, 0x1014), (0x6603, 0x1014)],
    rows: 3,
    columns: 6,
    keys: grid293s!(100),
    key_inputs: INPUTS_293S,
    press_only: &[0x10, 0x11, 0x12],
    orientation: Orientation::Rotate270,
    ..BASE
};

pub const D293S_512: Model = Model {
    usb: &[(0x5548, 0x6670)],
    keys: grid293s!(85),
    press_only: INPUTS_293S,
    packet: 512,
    ..D293S
};

pub const D293S_96: Model = Model {
    usb: &[(0x1500, 0x3003)],
    keys: grid293s!(96),
    press_only: &[],
    ..D293S
};

pub const XL: Model = Model {
    id: "XL",
    name: "Stream Dock XL",
    usb: &[(0x5548, 0x1031), (0x5548, 0x1028)],
    rows: 4,
    columns: 8,
    keys: &[
        sq(0x19, 80),
        sq(0x1a, 80),
        sq(0x1b, 80),
        sq(0x1c, 80),
        sq(0x1d, 80),
        sq(0x1e, 80),
        sq(0x1f, 80),
        sq(0x20, 80),
        sq(0x11, 80),
        sq(0x12, 80),
        sq(0x13, 80),
        sq(0x14, 80),
        sq(0x15, 80),
        sq(0x16, 80),
        sq(0x17, 80),
        sq(0x18, 80),
        sq(0x09, 80),
        sq(0x0a, 80),
        sq(0x0b, 80),
        sq(0x0c, 80),
        sq(0x0d, 80),
        sq(0x0e, 80),
        sq(0x0f, 80),
        sq(0x10, 80),
        sq(0x01, 80),
        sq(0x02, 80),
        sq(0x03, 80),
        sq(0x04, 80),
        sq(0x05, 80),
        sq(0x06, 80),
        sq(0x07, 80),
        sq(0x08, 80),
    ],
    key_inputs: &[
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e,
        0x1f, 0x20,
    ],
    // Three down the left side, then three down the right.
    buttons: &[0x21, 0x22, 0x23, 0x24, 0x25, 0x26],
    // One colour for the side lights.
    ring_leds: 1,
    orientation: Orientation::Rotate180,
    ..BASE
};

pub static ALL: &[Model] = &[M18, M18V3, D293V3, N3, N4, D293S, D293S_512, D293S_96, XL];

/// The model with these USB ids.
pub fn by_usb(vid: u16, pid: u16) -> Option<&'static Model> {
    ALL.iter().find(|m| m.matches(vid, pid))
}

/// The first model with this id ("M18", "N4"...).
pub fn by_id(id: &str) -> Option<&'static Model> {
    ALL.iter().find(|m| m.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_model_is_consistent() {
        let mut usb = std::collections::HashSet::new();
        for m in ALL {
            assert_eq!(m.keys.len(), m.key_count(), "{} keys", m.name);
            assert_eq!(m.key_inputs.len(), m.key_count(), "{} inputs", m.name);
            for pair in m.usb {
                assert!(usb.insert(*pair), "{} reuses {pair:04x?}", m.name);
            }
            // No report id may mean two things.
            let mut ids: Vec<u8> = m.key_inputs.to_vec();
            ids.extend(m.buttons);
            for d in m.dials {
                ids.extend([d.press, d.left, d.right]);
            }
            if let Some(s) = m.strip {
                assert_eq!(s.segments.len(), m.dials.len(), "{} strip", m.name);
                assert_eq!(s.taps.len(), s.segments.len());
                ids.extend(s.taps);
                ids.extend([s.swipe_left, s.swipe_right]);
            }
            let n = ids.len();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), n, "{} has a repeated report id", m.name);
            let mut screens: Vec<u8> = m.keys.iter().map(|s| s.id).collect();
            screens.sort();
            screens.dedup();
            assert_eq!(
                screens.len(),
                m.key_count(),
                "{} repeats an image id",
                m.name
            );
        }
    }

    #[test]
    fn specs_carry_the_model_over() {
        let s = M18.spec();
        assert_eq!((s.id.as_str(), s.rows, s.columns), ("M18", 3, 5));
        assert_eq!((s.buttons, s.dials, s.ring_leds), (3, 0, 24));
        assert_eq!(s.key_size(), 64);
        assert!(s.tested && !s.has_strip());
        assert_eq!(s.vsd_device.as_deref(), Some("VSDM18"));
        let n4 = N4.spec();
        assert_eq!(n4.strip.len(), 4);
        assert_eq!((n4.strip[0].width, n4.strip[0].height), (176, 124));
        assert_eq!(n4.orientation, Orientation::Rotate180);
    }

    #[test]
    fn finds_models_by_usb_id() {
        assert_eq!(by_usb(0x5548, 0x1000).unwrap().id, "M18");
        assert_eq!(by_usb(0x6603, 0x1007).unwrap().id, "N4");
        assert_eq!(by_usb(0x5548, 0x6670).unwrap().packet, 512);
        assert!(by_usb(0x1234, 0x5678).is_none());
    }
}
