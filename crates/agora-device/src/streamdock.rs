//! One driver for every Stream Dock model. The commands are the same on all
//! of them; what differs (key ids, picture sizes, knobs) lives in
//! [`crate::models`].
//!
//! Confirmed on Devin's M18 (2026-10-29):
//! - USB 5548:1000, vendor interface on usage page 0xFFA0.
//! - 1024-byte output reports, 512-byte input reports, no report ids.
//! - Every command starts with `CRT\0\0`.
//! - Start: DIS, CONNECT, CLE 0xff, STP. Re-send LIG every 8 s or the dock
//!   falls back to its stock screen.
//! - Key image: JPEG, sent as BAT + raw pages + STP. Sent upright on the
//!   M18 (no rotation), confirmed 2026-10-30.
//! - Image ids count from the bottom row; event ids count from the top row.
//! - Light ring: SETLB with 24 RGB triples, LBLIG for its brightness. A
//!   brightness write re-renders the strip and wipes a SETLB sent just
//!   before it lands, so colour always goes first and brightness is only
//!   sent when it changes (bidoofgoo/streamdock-m18 PROTOCOL.md section 8).
//! - Close: CLE 00 44 43, then HAN. DELED resets the ring.
//!
//! Other models (from Companion's MIT driver): pictures of at most 10 KB,
//! CONNECT as the keepalive, knobs report a turn as one id per direction,
//! and some controls report only a press.

use std::collections::VecDeque;

use std::time::Duration;

use crate::dock::Dock;
use crate::models::{Model, Screen};
use crate::spec::DeviceSpec;
use crate::transport::Transport;
use crate::{Action, Control, Error, InputEvent, Layout, Result, Rgb};

pub const IN_LEN: usize = 512;
pub const KEEPALIVE_MS: u64 = 8000;
/// The largest picture the docks take.
pub const MAX_IMAGE: usize = 10 * 1024;

const PREFIX: &[u8] = b"CRT\0\0";

pub struct StreamDock<T: Transport> {
    t: T,
    model: &'static Model,
    spec: DeviceSpec,
    brightness: u8,
    /// Last SETLB frame (RGB triples), re-sent with the keepalive.
    ring_frame: Option<Vec<u8>>,
    /// Last LBLIG sent; `None` until the ring is first used.
    ring_brightness: Option<u8>,
    /// Releases owed for controls that only report a press.
    pending: VecDeque<InputEvent>,
}

impl<T: Transport> StreamDock<T> {
    pub fn new(model: &'static Model, t: T) -> Self {
        Self {
            t,
            model,
            spec: model.spec(),
            brightness: 80,
            ring_frame: None,
            ring_brightness: None,
            pending: VecDeque::new(),
        }
    }

    pub fn model(&self) -> &'static Model {
        self.model
    }

    pub fn layout(&self) -> Layout {
        Layout::of(&self.spec)
    }

    fn raw(&mut self, payload: &[u8]) -> Result<()> {
        if payload.len() > self.model.packet {
            return Err(Error::TooLong {
                len: payload.len(),
                max: self.model.packet,
            });
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
        self.ring_frame = None;
        self.ring_brightness = None;
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

    /// Re-sends the ring colours and the screen brightness so the dock
    /// stays on our screen. Never re-sends the ring brightness: that would
    /// wipe the colours. Models we haven't tried also get CONNECT, the
    /// heartbeat Companion sends them.
    pub fn keepalive(&mut self) -> Result<()> {
        if !self.model.tested {
            self.cmd(b"CONNECT")?;
        }
        if let Some(frame) = self.ring_frame.clone() {
            self.send_ring_frame(&frame)?;
        }
        self.set_brightness(self.brightness)
    }

    pub fn clear_all(&mut self) -> Result<()> {
        self.cmd(&[b'C', b'L', b'E', 0, 0, 0, 0xff])?;
        self.cmd(b"STP")
    }

    /// Puts a ready JPEG on a key. `key` counts from 0 at the top left, row
    /// by row. Use [`crate::keyimage`] to make the JPEG at the key's size
    /// ([`Model::keys`]).
    pub fn set_key_jpeg(&mut self, key: usize, jpeg: &[u8]) -> Result<()> {
        let screen = *self.model.keys.get(key).ok_or(Error::NoSuchKey(key))?;
        self.send_image(screen, jpeg)
    }

    /// Puts a ready JPEG on one segment of the touch strip (the N4).
    pub fn set_strip_jpeg(&mut self, segment: usize, jpeg: &[u8]) -> Result<()> {
        let screen = *self
            .model
            .strip
            .and_then(|s| s.segments.get(segment))
            .ok_or(Error::NoSuchKey(segment))?;
        self.send_image(screen, jpeg)
    }

    fn send_image(&mut self, screen: Screen, jpeg: &[u8]) -> Result<()> {
        let len = u16::try_from(jpeg.len()).map_err(|_| Error::TooLong {
            len: jpeg.len(),
            max: u16::MAX as usize,
        })?;
        let [hi, lo] = len.to_be_bytes();
        self.cmd(&[b'B', b'A', b'T', 0, 0, hi, lo, screen.id])?;
        for page in jpeg.chunks(self.model.packet) {
            self.raw(page)?;
        }
        self.cmd(b"STP")
    }

    fn send_ring_frame(&mut self, frame: &[u8]) -> Result<()> {
        let mut body = Vec::with_capacity(5 + frame.len());
        body.extend_from_slice(b"SETLB");
        body.extend_from_slice(frame);
        self.cmd(&body)
    }

    /// Sets every ring LED (on the M18, 0-21 the ring and 22-23 the front).
    /// `colors` shorter than the ring leaves the rest dark. The first time,
    /// also turns the ring's brightness up, after the colours so they
    /// aren't wiped. Does nothing on a model without lights.
    pub fn set_ring(&mut self, colors: &[Rgb]) -> Result<()> {
        let leds = self.model.ring_leds;
        if leds == 0 {
            return Ok(());
        }
        let mut frame = Vec::with_capacity(leds * 3);
        for i in 0..leds {
            let Rgb(r, g, b) = colors.get(i).copied().unwrap_or_default();
            frame.extend_from_slice(&[r, g, b]);
        }
        self.send_ring_frame(&frame)?;
        self.ring_frame = Some(frame);
        if self.ring_brightness.is_none() {
            self.send_ring_brightness(100)?;
        }
        Ok(())
    }

    /// Ring brightness, 0 to 100. Skipped when unchanged; the colours are
    /// sent again first so the brightness render picks them up.
    pub fn set_ring_brightness(&mut self, pct: u8) -> Result<()> {
        let pct = pct.min(100);
        if self.model.ring_leds == 0 || self.ring_brightness == Some(pct) {
            return Ok(());
        }
        if let Some(frame) = self.ring_frame.clone() {
            self.send_ring_frame(&frame)?;
        }
        self.send_ring_brightness(pct)
    }

    fn send_ring_brightness(&mut self, pct: u8) -> Result<()> {
        self.cmd(&[b'L', b'B', b'L', b'I', b'G', pct])?;
        self.ring_brightness = Some(pct);
        Ok(())
    }

    /// Turns every LED off (and keeps it off through keepalives).
    pub fn ring_off(&mut self) -> Result<()> {
        self.set_ring(&[])
    }

    /// Hands the strip back to the dock's own lighting.
    pub fn ring_reset(&mut self) -> Result<()> {
        if self.model.ring_leds == 0 {
            return Ok(());
        }
        self.ring_frame = None;
        self.cmd(b"DELED")
    }

    /// Writes `jpeg` as the dock's boot logo, the picture it keeps in its
    /// own memory and shows when it starts or no app drives it.
    ///
    /// Byte for byte what MiraBox's transport library sends
    /// (`Transport::setBackgroundImgStream`, read from its libtransport.so):
    /// `CRT\0\0LOG`, the size as u32 big endian, `0x01`, then the JPEG in
    /// raw pages, then a wait for the dock's answer. No STP. The picture
    /// must be a JPEG of exactly the model's logo size: MiraBox warns that
    /// anything else can brick a dock, so it is checked here once more.
    pub fn set_boot_logo(&mut self, jpeg: &[u8]) -> Result<()> {
        let logo = self.model.boot_logo.ok_or(Error::Unsupported)?;
        if jpeg.len() > crate::keyimage::MAX_BOOT_LOGO {
            return Err(Error::TooLong {
                len: jpeg.len(),
                max: crate::keyimage::MAX_BOOT_LOGO,
            });
        }
        let (w, h) = crate::keyimage::jpeg_size(jpeg)
            .ok_or_else(|| Error::Image("The boot logo isn't a JPEG".into()))?;
        let want = match logo.orientation {
            crate::keyimage::Orientation::Rotate90
            | crate::keyimage::Orientation::Rotate270
            | crate::keyimage::Orientation::Transpose => (logo.height, logo.width),
            _ => (logo.width, logo.height),
        };
        if (w, h) != want {
            return Err(Error::Image(format!(
                "The boot logo must be {}x{}, not {w}x{h}",
                want.0, want.1
            )));
        }
        let len = (jpeg.len() as u32).to_be_bytes();
        self.cmd(&[b'L', b'O', b'G', len[0], len[1], len[2], len[3], 0x01])?;
        for page in jpeg.chunks(self.model.packet) {
            self.raw(page)?;
        }
        // The dock answers once the picture is stored. Wait for it (up to
        // ten seconds) so nothing else is sent while it writes its memory.
        let mut buf = [0u8; IN_LEN];
        let until = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < until {
            let n = self.t.read(&mut buf, 250)?;
            if n == 0 {
                continue;
            }
            // "ACK\0\0OK" is the answer; a key pressed meanwhile is kept.
            if n >= 7 && &buf[5..7] == b"OK" {
                break;
            }
            match parse_event(&buf[..n], self.model) {
                Some(ev) if !matches!(ev.control, Control::Unknown(_)) => self.pending.push_back(ev),
                _ => break,
            }
        }
        Ok(())
    }

    /// Hands the dock back to its stock screen.
    pub fn close(&mut self) -> Result<()> {
        self.cmd(&[b'C', b'L', b'E', 0, 0x44, 0x43])?;
        self.cmd(b"HAN")
    }

    /// Reads one event, waiting up to `timeout_ms`. `Ok(None)` on timeout or
    /// for reports that aren't input.
    pub fn read_event(&mut self, timeout_ms: i32) -> Result<Option<InputEvent>> {
        if let Some(ev) = self.pending.pop_front() {
            return Ok(Some(ev));
        }
        let mut buf = [0u8; IN_LEN];
        let n = self.t.read(&mut buf, timeout_ms)?;
        let ev = parse_event(&buf[..n], self.model);
        if let Some(e) = ev {
            if e.action == Action::Press && self.model.press_only.contains(&e.raw_id) {
                self.pending.push_back(InputEvent {
                    action: Action::Release,
                    ..e
                });
            }
        }
        Ok(ev)
    }

    pub fn into_transport(self) -> T {
        self.t
    }
}

impl<T: Transport> Dock for StreamDock<T> {
    fn spec(&self) -> &DeviceSpec {
        &self.spec
    }
    fn open(&mut self) -> Result<()> {
        StreamDock::open(self)
    }
    fn set_brightness(&mut self, pct: u8) -> Result<()> {
        StreamDock::set_brightness(self, pct)
    }
    fn keepalive_interval(&self) -> Option<Duration> {
        Some(Duration::from_millis(KEEPALIVE_MS))
    }
    fn keepalive(&mut self) -> Result<()> {
        StreamDock::keepalive(self)
    }
    fn clear_all(&mut self) -> Result<()> {
        StreamDock::clear_all(self)
    }
    fn set_key_image(&mut self, key: usize, image: &[u8]) -> Result<()> {
        self.set_key_jpeg(key, image)
    }
    fn set_strip_image(&mut self, segment: usize, image: &[u8]) -> Result<()> {
        self.set_strip_jpeg(segment, image)
    }
    fn set_ring(&mut self, colors: &[Rgb]) -> Result<()> {
        StreamDock::set_ring(self, colors)
    }
    fn set_ring_brightness(&mut self, pct: u8) -> Result<()> {
        StreamDock::set_ring_brightness(self, pct)
    }
    fn ring_off(&mut self) -> Result<()> {
        StreamDock::ring_off(self)
    }
    fn ring_reset(&mut self) -> Result<()> {
        StreamDock::ring_reset(self)
    }
    fn set_boot_logo(&mut self, jpeg: &[u8]) -> Result<()> {
        StreamDock::set_boot_logo(self, jpeg)
    }
    fn close(&mut self) -> Result<()> {
        StreamDock::close(self)
    }
    fn read_event(&mut self, timeout_ms: i32) -> Result<Option<InputEvent>> {
        StreamDock::read_event(self, timeout_ms)
    }
}

/// Parses an input report: `ACK` at the start, id at byte 9, state at
/// byte 10 (0 up, anything else down).
pub fn parse_event(report: &[u8], model: &Model) -> Option<InputEvent> {
    if report.len() < 11 || &report[..3] != b"ACK" {
        return None;
    }
    let (id, state) = (report[9], report[10]);
    let button = if state == 0 {
        Action::Release
    } else {
        Action::Press
    };
    let (control, action) = if let Some(k) = model.key_inputs.iter().position(|&i| i == id) {
        (Control::Key(k), button)
    } else if let Some(b) = model.buttons.iter().position(|&i| i == id) {
        (Control::Button(b), button)
    } else if let Some((d, dial)) = model
        .dials
        .iter()
        .enumerate()
        .find(|(_, d)| [d.press, d.left, d.right].contains(&id))
    {
        let action = if id == dial.left {
            Action::Turn(-1)
        } else if id == dial.right {
            Action::Turn(1)
        } else {
            button
        };
        (Control::Dial(d), action)
    } else {
        match model.strip {
            Some(s) if s.taps.contains(&id) => (
                Control::Touch(s.taps.iter().position(|&i| i == id).unwrap_or(0)),
                Action::Press,
            ),
            Some(s) if id == s.swipe_left => (Control::Strip, Action::Swipe(-1)),
            Some(s) if id == s.swipe_right => (Control::Strip, Action::Swipe(1)),
            _ => (Control::Unknown(id), button),
        }
    };
    Some(InputEvent {
        control,
        action,
        raw_id: id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{self, M18};
    use crate::transport::FakeTransport;

    fn dock() -> StreamDock<FakeTransport> {
        StreamDock::new(&M18, FakeTransport::default())
    }

    fn picture(w: u32, h: u32) -> crate::keyimage::DynamicImage {
        crate::keyimage::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x * y) % 256) as u8])
        }))
    }

    #[test]
    fn boot_logo_goes_out_as_mirabox_sends_it() {
        let jpeg = M18.spec().encode_boot_logo(&picture(1000, 700)).unwrap();
        assert_eq!(crate::keyimage::jpeg_size(&jpeg), Some((480, 272)));
        let mut d = dock();
        d.t.to_read.push_back(b"ACK\0\0OK\0".to_vec());
        d.set_boot_logo(&jpeg).unwrap();
        let w = &d.t.written;
        let n = jpeg.len() as u32;
        let mut head = b"CRT\0\0LOG".to_vec();
        head.extend_from_slice(&n.to_be_bytes());
        head.push(0x01);
        assert_eq!(w[0], head);
        assert_eq!(w.len(), 1 + jpeg.len().div_ceil(1024));
        assert_eq!(w[1..].concat(), jpeg);
        // No STP after it, as in MiraBox's library.
        assert!(!w.last().unwrap().starts_with(b"CRT"));
    }

    #[test]
    fn boot_logo_refuses_the_wrong_size_or_model() {
        let mut d = dock();
        let small = crate::keyimage::to_jpeg(&picture(64, 64), 64, 64, crate::keyimage::Orientation::Upright).unwrap();
        assert!(d.set_boot_logo(&small).is_err());
        assert!(d.set_boot_logo(b"not a jpeg").is_err());
        assert!(d.t.written.is_empty());
        let mut n4 = StreamDock::new(&models::N4, FakeTransport::default());
        let jpeg = M18.spec().encode_boot_logo(&picture(480, 272)).unwrap();
        assert!(matches!(n4.set_boot_logo(&jpeg), Err(Error::Unsupported)));
        assert!(models::N4.spec().encode_boot_logo(&picture(10, 10)).is_err());
    }

    #[test]
    fn a_key_pressed_while_the_logo_is_stored_is_kept() {
        let jpeg = M18.spec().encode_boot_logo(&picture(480, 272)).unwrap();
        let mut d = dock();
        let mut press = vec![0u8; 16];
        press[..3].copy_from_slice(b"ACK");
        press[9] = 1;
        press[10] = 1;
        d.t.to_read.push_back(press);
        d.t.to_read.push_back(b"ACK\0\0OK\0".to_vec());
        d.set_boot_logo(&jpeg).unwrap();
        assert!(d.read_event(0).unwrap().is_some());
    }

    #[test]
    fn top_left_key_is_image_id_0x0b() {
        // Seen on the real dock: image id 0x0B lit the top-left key.
        let id = |k: usize| M18.keys[k].id;
        assert_eq!(id(0), 0x0B);
        assert_eq!(id(4), 0x0F);
        assert_eq!(id(10), 0x01);
        assert_eq!(id(14), 0x05);
        assert!(dock().set_key_jpeg(15, &[1]).is_err());
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
    fn small_packet_models_send_smaller_pages() {
        let mut d = StreamDock::new(&models::D293S_512, FakeTransport::default());
        d.set_key_jpeg(0, &[0; 1100]).unwrap();
        let w = d.into_transport().written;
        assert_eq!(w[0][12], 0x0d); // the 293S's top-left image id
        assert_eq!(
            w[1..4].iter().map(Vec::len).collect::<Vec<_>>(),
            [512, 512, 76]
        );
    }

    #[test]
    fn strip_segments_have_their_own_ids() {
        let mut d = StreamDock::new(&models::N4, FakeTransport::default());
        d.set_strip_jpeg(2, &[0; 10]).unwrap();
        assert!(d.set_strip_jpeg(4, &[0; 10]).is_err());
        assert!(dock().set_strip_jpeg(0, &[0; 10]).is_err());
        let w = d.into_transport().written;
        assert_eq!(w[0], b"CRT\0\0BAT\0\0\x00\x0a\x03");
    }

    #[test]
    fn ring_sends_24_triples() {
        let mut d = dock();
        d.set_ring(&[Rgb(0, 80, 255); 24]).unwrap();
        let w = d.into_transport().written;
        assert_eq!(w[0].len(), 5 + 5 + 72);
        assert_eq!(&w[0][5..10], b"SETLB");
        assert_eq!(&w[0][10..13], &[0, 80, 255]);
        // First use turns the ring brightness up, after the colours.
        assert_eq!(w[1], b"CRT\0\0LBLIG\x64");
        assert_eq!(w.len(), 2);
    }

    #[test]
    fn models_without_lights_ignore_the_ring() {
        let mut d = StreamDock::new(&models::N3, FakeTransport::default());
        d.set_ring(&[Rgb(1, 2, 3)]).unwrap();
        d.set_ring_brightness(50).unwrap();
        d.ring_reset().unwrap();
        assert!(d.into_transport().written.is_empty());
    }

    #[test]
    fn ring_colour_always_goes_before_its_brightness() {
        let mut d = dock();
        d.set_ring(&[Rgb(255, 0, 0); 24]).unwrap();
        d.set_ring_brightness(40).unwrap();
        d.set_ring_brightness(40).unwrap(); // unchanged: nothing sent
        d.keepalive().unwrap();
        let w = d.into_transport().written;
        let names: Vec<&[u8]> = w.iter().map(|m| &m[5..10]).collect();
        assert_eq!(
            names,
            [
                &b"SETLB"[..],
                b"LBLIG",
                b"SETLB",
                b"LBLIG",
                b"SETLB",
                b"LIG\0\0"
            ]
        );
        assert_eq!(w[3], b"CRT\0\0LBLIG\x28");
    }

    fn report(id: u8, state: u8) -> Vec<u8> {
        let mut r = b"ACK\0\0OK\0\0".to_vec();
        r.push(id);
        r.push(state);
        r
    }

    #[test]
    fn events_map_to_keys_and_buttons() {
        let ev = |id, state| parse_event(&report(id, state), &M18).unwrap();
        let e = ev(0x01, 1);
        assert_eq!(e.control, Control::Key(0));
        assert_eq!(e.action, Action::Press);
        assert_eq!(ev(0x0F, 0).control, Control::Key(14));
        assert_eq!(ev(0x0F, 0).action, Action::Release);
        assert_eq!(ev(0x25, 1).control, Control::Button(0));
        assert_eq!(ev(0x30, 1).control, Control::Button(1));
        assert_eq!(ev(0x31, 1).control, Control::Button(2));
        assert_eq!(ev(0x40, 1).control, Control::Unknown(0x40));
        assert!(parse_event(b"NAK", &M18).is_none());
    }

    #[test]
    fn knobs_turn_and_press() {
        let ev = |id, state| parse_event(&report(id, state), &models::N3).unwrap();
        assert_eq!(ev(0x50, 0).control, Control::Dial(0));
        assert_eq!(ev(0x50, 0).action, Action::Turn(-1));
        assert_eq!(ev(0x91, 0).action, Action::Turn(1));
        assert_eq!(ev(0x91, 0).control, Control::Dial(1));
        assert_eq!(ev(0x34, 1).control, Control::Dial(2));
        assert_eq!(ev(0x34, 1).action, Action::Press);
        assert_eq!(ev(0x25, 1).control, Control::Button(0));
    }

    #[test]
    fn press_only_controls_get_a_release() {
        let mut fake = FakeTransport::default();
        fake.to_read.push_back(report(0x41, 1)); // N4 strip, second segment
        fake.to_read.push_back(report(0x39, 1)); // swipe right
        let mut d = StreamDock::new(&models::N4, fake);
        let a = d.read_event(0).unwrap().unwrap();
        let b = d.read_event(0).unwrap().unwrap();
        let c = d.read_event(0).unwrap().unwrap();
        assert_eq!((a.control, a.action), (Control::Touch(1), Action::Press));
        assert_eq!((b.control, b.action), (Control::Touch(1), Action::Release));
        assert_eq!((c.control, c.action), (Control::Strip, Action::Swipe(1)));
        assert!(d.read_event(0).unwrap().is_none());
    }
}
