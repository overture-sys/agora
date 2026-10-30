//! Driver for the VSDinside / Mirabox Stream Dock M18.
//!
//! Confirmed on Devin's M18 (2026-10-29):
//! - USB 5548:1000, vendor interface on usage page 0xFFA0.
//! - 1024-byte output reports, 512-byte input reports, no report ids.
//! - Every command starts with `CRT\0\0`.
//! - Start: DIS, CONNECT, CLE 0xff, STP. Re-send LIG every 8 s or the dock
//!   falls back to its stock screen.
//! - Key image: 64x64 JPEG, sent as BAT + raw 1024-byte pages + STP. Sent
//!   upright (no rotation), confirmed 2026-10-30.
//! - Image ids count from the bottom row; event ids count from the top row.
//! - Light ring: SETLB with 24 RGB triples. Send colour before brightness.
//! - Close: CLE 00 44 43, then HAN. DELED resets the ring.

use crate::transport::Transport;
use crate::{Button, Control, Error, InputEvent, Layout, Result, Rgb};

pub const VID: u16 = 0x5548;
pub const PID: u16 = 0x1000;
pub const USAGE_PAGE: u16 = 0xFFA0;
pub const OUT_LEN: usize = 1024;
pub const IN_LEN: usize = 512;
pub const KEEPALIVE_MS: u64 = 8000;

const PREFIX: &[u8] = b"CRT\0\0";

pub const LAYOUT: Layout = Layout { rows: 3, columns: 5, key_size: 64, buttons: 3, ring_leds: 24 };

/// Which raw ids belong to the round buttons.
///
/// Confirmed one button at a time on Devin's M18 with `agora-m18 buttons`
/// (2026-10-30): left 0x25, middle 0x30, right 0x31, as in the public notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonMap {
    pub left: u8,
    pub middle: u8,
    pub right: u8,
}

impl Default for ButtonMap {
    fn default() -> Self {
        Self { left: 0x25, middle: 0x30, right: 0x31 }
    }
}

pub struct M18<T: Transport> {
    t: T,
    pub buttons: ButtonMap,
    brightness: u8,
}

impl<T: Transport> M18<T> {
    pub fn new(t: T) -> Self {
        Self { t, buttons: ButtonMap::default(), brightness: 80 }
    }

    pub fn layout(&self) -> Layout {
        LAYOUT
    }

    fn raw(&mut self, payload: &[u8]) -> Result<()> {
        if payload.len() > OUT_LEN {
            return Err(Error::TooLong { len: payload.len(), max: OUT_LEN });
        }
        self.t.write(payload)
    }

    fn cmd(&mut self, body: &[u8]) -> Result<()> {
        let mut msg = Vec::with_capacity(PREFIX.len() + body.len());
        msg.extend_from_slice(PREFIX);
        msg.extend_from_slice(body);
        self.raw(&msg)
    }

    /// Takes the dock over from its stock screen and blanks every key.
    pub fn open(&mut self) -> Result<()> {
        self.cmd(b"DIS")?;
        std::thread::sleep(std::time::Duration::from_millis(50));
        self.cmd(b"CONNECT")?;
        std::thread::sleep(std::time::Duration::from_millis(50));
        self.clear_all()?;
        self.set_brightness(self.brightness)
    }

    /// Sets screen brightness, 0 to 100. Also serves as the keepalive.
    pub fn set_brightness(&mut self, pct: u8) -> Result<()> {
        self.brightness = pct.min(100);
        self.cmd(&[b'L', b'I', b'G', 0, 0, self.brightness])
    }

    pub fn brightness(&self) -> u8 {
        self.brightness
    }

    /// Re-sends the last brightness so the dock stays on our screen.
    pub fn keepalive(&mut self) -> Result<()> {
        self.set_brightness(self.brightness)
    }

    pub fn clear_all(&mut self) -> Result<()> {
        self.cmd(&[b'C', b'L', b'E', 0, 0, 0, 0xff])?;
        self.cmd(b"STP")
    }

    /// Puts a ready 64x64 JPEG on a key. `key` counts from 0 at the top
    /// left, row by row. Use [`crate::keyimage`] to make the JPEG.
    pub fn set_key_jpeg(&mut self, key: usize, jpeg: &[u8]) -> Result<()> {
        let id = image_id(key)?;
        let len = u16::try_from(jpeg.len())
            .map_err(|_| Error::TooLong { len: jpeg.len(), max: u16::MAX as usize })?;
        let [hi, lo] = len.to_be_bytes();
        self.cmd(&[b'B', b'A', b'T', 0, 0, hi, lo, id])?;
        for page in jpeg.chunks(OUT_LEN) {
            self.raw(page)?;
        }
        self.cmd(b"STP")
    }

    /// Sets every ring LED. `colors` shorter than 24 leaves the rest dark.
    pub fn set_ring(&mut self, colors: &[Rgb]) -> Result<()> {
        let mut body = Vec::with_capacity(5 + LAYOUT.ring_leds * 3);
        body.extend_from_slice(b"SETLB");
        for i in 0..LAYOUT.ring_leds {
            let Rgb(r, g, b) = colors.get(i).copied().unwrap_or_default();
            body.extend_from_slice(&[r, g, b]);
        }
        self.cmd(&body)
    }

    pub fn ring_reset(&mut self) -> Result<()> {
        self.cmd(b"DELED")
    }

    /// Hands the dock back to its stock screen.
    pub fn close(&mut self) -> Result<()> {
        self.cmd(&[b'C', b'L', b'E', 0, 0x44, 0x43])?;
        self.cmd(b"HAN")
    }

    /// Reads one event, waiting up to `timeout_ms`. `Ok(None)` on timeout or
    /// for reports that aren't key events.
    pub fn read_event(&mut self, timeout_ms: i32) -> Result<Option<InputEvent>> {
        let mut buf = [0u8; IN_LEN];
        let n = self.t.read(&mut buf, timeout_ms)?;
        Ok(parse_event(&buf[..n], &self.buttons))
    }

    pub fn into_transport(self) -> T {
        self.t
    }
}

/// Maps a grid index (0 = top left) to the M18's image id (bottom row first).
pub fn image_id(key: usize) -> Result<u8> {
    if key >= LAYOUT.key_count() {
        return Err(Error::NoSuchKey(key));
    }
    let (row, col) = (key / LAYOUT.columns, key % LAYOUT.columns);
    Ok(((LAYOUT.rows - 1 - row) * LAYOUT.columns + col + 1) as u8)
}

/// Parses an input report: `ACK` at the start, id at byte 9, 1/0 at byte 10.
pub fn parse_event(report: &[u8], buttons: &ButtonMap) -> Option<InputEvent> {
    if report.len() < 11 || &report[..3] != b"ACK" {
        return None;
    }
    let (id, state) = (report[9], report[10]);
    let control = match id {
        1..=15 => Control::Key(id as usize - 1),
        _ if id == buttons.left => Control::Button(Button::Left),
        _ if id == buttons.middle => Control::Button(Button::Middle),
        _ if id == buttons.right => Control::Button(Button::Right),
        _ => Control::Unknown(id),
    };
    Some(InputEvent { control, pressed: state == 1, raw_id: id })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::FakeTransport;

    fn dock() -> M18<FakeTransport> {
        M18::new(FakeTransport::default())
    }

    #[test]
    fn top_left_key_is_image_id_0x0b() {
        // Seen on the real dock: image id 0x0B lit the top-left key.
        assert_eq!(image_id(0).unwrap(), 0x0B);
        assert_eq!(image_id(4).unwrap(), 0x0F);
        assert_eq!(image_id(10).unwrap(), 0x01);
        assert_eq!(image_id(14).unwrap(), 0x05);
        assert!(image_id(15).is_err());
    }

    #[test]
    fn commands_match_the_tested_python_script() {
        let mut d = dock();
        d.set_brightness(80).unwrap();
        d.clear_all().unwrap();
        d.close().unwrap();
        let w = d.into_transport().written;
        assert_eq!(w[0], b"CRT\0\0LIG\0\0\x50");
        assert_eq!(w[1], b"CRT\0\0CLE\0\0\0\xff");
        assert_eq!(w[2], b"CRT\0\0STP");
        assert_eq!(w[3], b"CRT\0\0CLE\0\x44\x43");
        assert_eq!(w[4], b"CRT\0\0HAN");
    }

    #[test]
    fn brightness_is_capped_at_100() {
        let mut d = dock();
        d.set_brightness(250).unwrap();
        assert_eq!(d.brightness(), 100);
    }

    #[test]
    fn key_image_is_bat_pages_stp() {
        let mut d = dock();
        let jpeg = vec![0xAB; 2500];
        d.set_key_jpeg(0, &jpeg).unwrap();
        let w = d.into_transport().written;
        assert_eq!(w[0], b"CRT\0\0BAT\0\0\x09\xc4\x0b"); // 2500 = 0x09C4, id 0x0B
        assert_eq!(w[1].len(), 1024);
        assert_eq!(w[2].len(), 1024);
        assert_eq!(w[3].len(), 452);
        assert_eq!(w[4], b"CRT\0\0STP");
    }

    #[test]
    fn ring_sends_24_triples() {
        let mut d = dock();
        d.set_ring(&[Rgb(0, 80, 255); 24]).unwrap();
        let w = d.into_transport().written;
        assert_eq!(w[0].len(), 5 + 5 + 72);
        assert_eq!(&w[0][5..10], b"SETLB");
        assert_eq!(&w[0][10..13], &[0, 80, 255]);
    }

    fn report(id: u8, state: u8) -> Vec<u8> {
        let mut r = b"ACK\0\0OK\0\0".to_vec();
        r.push(id);
        r.push(state);
        r
    }

    #[test]
    fn events_map_to_keys_and_buttons() {
        let b = ButtonMap::default();
        let e = parse_event(&report(0x01, 1), &b).unwrap();
        assert_eq!(e.control, Control::Key(0));
        assert!(e.pressed);
        assert_eq!(parse_event(&report(0x0F, 0), &b).unwrap().control, Control::Key(14));
        assert_eq!(parse_event(&report(0x25, 1), &b).unwrap().control, Control::Button(Button::Left));
        assert_eq!(parse_event(&report(0x30, 1), &b).unwrap().control, Control::Button(Button::Middle));
        assert_eq!(parse_event(&report(0x31, 1), &b).unwrap().control, Control::Button(Button::Right));
        assert_eq!(parse_event(&report(0x40, 1), &b).unwrap().control, Control::Unknown(0x40));
        assert!(parse_event(b"NAK", &b).is_none());
    }
}
