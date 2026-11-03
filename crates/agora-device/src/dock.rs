//! The one interface every device driver offers, whatever its family. The
//! worker, the app and the tests only talk to a [`Dock`].

use std::time::Duration;

use crate::spec::DeviceSpec;
use crate::{Error, InputEvent, Result, Rgb};

/// A device driver. [`crate::StreamDock`] serves every Stream Dock model;
/// other families get their own driver behind this trait. Controls a device
/// doesn't have keep the default (do nothing, or refuse).
pub trait Dock: Send {
    /// What the device is: grid, screens, buttons, knobs.
    fn spec(&self) -> &DeviceSpec;

    /// Takes the device over and blanks it.
    fn open(&mut self) -> Result<()>;

    /// Screen brightness, 0 to 100.
    fn set_brightness(&mut self, pct: u8) -> Result<()>;

    /// How often [`Dock::keepalive`] must run, if the device needs it at all.
    /// Stream Docks fall back to their stock screen without one.
    fn keepalive_interval(&self) -> Option<Duration> {
        None
    }

    fn keepalive(&mut self) -> Result<()> {
        Ok(())
    }

    fn clear_all(&mut self) -> Result<()>;

    /// Puts a picture on a key. `image` is already encoded for the device
    /// ([`DeviceSpec::encode`]).
    fn set_key_image(&mut self, key: usize, image: &[u8]) -> Result<()>;

    /// Puts a picture on one segment of the touch strip.
    fn set_strip_image(&mut self, segment: usize, _image: &[u8]) -> Result<()> {
        Err(Error::NoSuchKey(segment))
    }

    fn set_ring(&mut self, _colors: &[Rgb]) -> Result<()> {
        Ok(())
    }

    fn set_ring_brightness(&mut self, _pct: u8) -> Result<()> {
        Ok(())
    }

    fn ring_off(&mut self) -> Result<()> {
        Ok(())
    }

    /// Hands the lights back to the device's own lighting.
    fn ring_reset(&mut self) -> Result<()> {
        Ok(())
    }

    /// Hands the device back to its stock screen.
    fn close(&mut self) -> Result<()>;

    /// Reads one event, waiting up to `timeout_ms`.
    fn read_event(&mut self, timeout_ms: i32) -> Result<Option<InputEvent>>;
}

impl Dock for Box<dyn Dock> {
    fn spec(&self) -> &DeviceSpec {
        (**self).spec()
    }
    fn open(&mut self) -> Result<()> {
        (**self).open()
    }
    fn set_brightness(&mut self, pct: u8) -> Result<()> {
        (**self).set_brightness(pct)
    }
    fn keepalive_interval(&self) -> Option<Duration> {
        (**self).keepalive_interval()
    }
    fn keepalive(&mut self) -> Result<()> {
        (**self).keepalive()
    }
    fn clear_all(&mut self) -> Result<()> {
        (**self).clear_all()
    }
    fn set_key_image(&mut self, key: usize, image: &[u8]) -> Result<()> {
        (**self).set_key_image(key, image)
    }
    fn set_strip_image(&mut self, segment: usize, image: &[u8]) -> Result<()> {
        (**self).set_strip_image(segment, image)
    }
    fn set_ring(&mut self, colors: &[Rgb]) -> Result<()> {
        (**self).set_ring(colors)
    }
    fn set_ring_brightness(&mut self, pct: u8) -> Result<()> {
        (**self).set_ring_brightness(pct)
    }
    fn ring_off(&mut self) -> Result<()> {
        (**self).ring_off()
    }
    fn ring_reset(&mut self) -> Result<()> {
        (**self).ring_reset()
    }
    fn close(&mut self) -> Result<()> {
        (**self).close()
    }
    fn read_event(&mut self, timeout_ms: i32) -> Result<Option<InputEvent>> {
        (**self).read_event(timeout_ms)
    }
}
