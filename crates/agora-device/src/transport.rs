//! The byte pipe a driver talks through. Real docks use HID; tests use a
//! recording fake.

use crate::{Error, Result};

pub trait Transport: Send {
    /// Sends one output report. `data` excludes the report id byte.
    fn write(&mut self, data: &[u8]) -> Result<()>;

    /// Reads one input report into `buf`, waiting up to `timeout_ms`.
    /// Returns 0 when nothing arrived in time.
    fn read(&mut self, buf: &mut [u8], timeout_ms: i32) -> Result<usize>;

    /// Sends a feature report. Unlike [`Transport::write`], `data` starts
    /// with the report id. Stream Decks take brightness and resets this way;
    /// Stream Docks don't use them.
    fn send_feature(&mut self, _data: &[u8]) -> Result<()> {
        Err(Error::Unsupported)
    }

    /// Reads the feature report whose id is `buf[0]` into `buf` and returns
    /// its length (serial number and firmware version on Stream Decks).
    fn get_feature(&mut self, _buf: &mut [u8]) -> Result<usize> {
        Err(Error::Unsupported)
    }
}

#[cfg(feature = "hid")]
pub struct HidTransport {
    dev: hidapi::HidDevice,
    out_len: usize,
    buf: Vec<u8>,
}

#[cfg(feature = "hid")]
impl HidTransport {
    /// Wraps an open HID device whose output reports are `out_len` bytes.
    pub fn new(dev: hidapi::HidDevice, out_len: usize) -> Self {
        Self {
            dev,
            out_len,
            buf: Vec::with_capacity(out_len + 1),
        }
    }
}

#[cfg(feature = "hid")]
impl Transport for HidTransport {
    fn write(&mut self, data: &[u8]) -> Result<()> {
        if data.len() > self.out_len {
            return Err(crate::Error::TooLong {
                len: data.len(),
                max: self.out_len,
            });
        }
        // Report id 0, then the payload padded to the full report length.
        // The M18 drops anything shorter.
        self.buf.clear();
        self.buf.push(0);
        self.buf.extend_from_slice(data);
        self.buf.resize(self.out_len + 1, 0);
        self.dev.write(&self.buf).map_err(usb_err)?;
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8], timeout_ms: i32) -> Result<usize> {
        self.dev.read_timeout(buf, timeout_ms).map_err(usb_err)
    }

    fn send_feature(&mut self, data: &[u8]) -> Result<()> {
        self.dev.send_feature_report(data).map_err(usb_err)
    }

    fn get_feature(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.dev.get_feature_report(buf).map_err(usb_err)
    }
}

#[cfg(feature = "hid")]
fn usb_err(e: hidapi::HidError) -> crate::Error {
    crate::Error::Usb(e.to_string())
}

/// A transport that records writes and replays queued reads. For tests.
#[derive(Default)]
pub struct FakeTransport {
    pub written: Vec<Vec<u8>>,
    pub to_read: std::collections::VecDeque<Vec<u8>>,
    /// Feature reports sent, report id first.
    pub features_sent: Vec<Vec<u8>>,
    /// Feature reports to answer with, by report id.
    pub features: std::collections::HashMap<u8, Vec<u8>>,
}

impl Transport for FakeTransport {
    fn write(&mut self, data: &[u8]) -> Result<()> {
        self.written.push(data.to_vec());
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8], _timeout_ms: i32) -> Result<usize> {
        match self.to_read.pop_front() {
            Some(r) => {
                let n = r.len().min(buf.len());
                buf[..n].copy_from_slice(&r[..n]);
                Ok(n)
            }
            None => Ok(0),
        }
    }

    fn send_feature(&mut self, data: &[u8]) -> Result<()> {
        self.features_sent.push(data.to_vec());
        Ok(())
    }

    fn get_feature(&mut self, buf: &mut [u8]) -> Result<usize> {
        let id = *buf.first().ok_or(Error::Unsupported)?;
        let r = self.features.get(&id).ok_or(Error::Unsupported)?;
        let n = r.len().min(buf.len());
        buf[..n].copy_from_slice(&r[..n]);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_answers_feature_reports_by_id() {
        let mut t = FakeTransport::default();
        t.features.insert(0x06, vec![0x06, b'A', b'B']);
        let mut buf = [0u8; 32];
        buf[0] = 0x06;
        assert_eq!(t.get_feature(&mut buf).unwrap(), 3);
        assert_eq!(&buf[1..3], b"AB");
        buf[0] = 0x07;
        assert!(t.get_feature(&mut buf).is_err());
        t.send_feature(&[0x03, 0x08, 50]).unwrap();
        assert_eq!(t.features_sent, [vec![0x03, 0x08, 50]]);
    }
}
