//! Finds supported devices on USB, of every family, and opens them with
//! the right driver.

use hidapi::{DeviceInfo, HidApi};

use crate::dock::Dock;
use crate::models::{self, Model};
use crate::spec::{DeviceSpec, Family};
use crate::transport::HidTransport;
use crate::{Error, Result, StreamDock};

/// A device found on USB.
#[derive(Debug, Clone)]
pub struct Found {
    pub family: Family,
    pub spec: DeviceSpec,
    /// The Stream Dock table entry, for Stream Docks.
    pub model: &'static Model,
    pub path: std::ffi::CString,
    pub serial: Option<String>,
    pub product: Option<String>,
}

impl Found {
    /// A name for this device that stays the same between runs, for its
    /// layout: the serial number when it has one (a blank or all-zero
    /// serial doesn't count), else the model id.
    pub fn key(&self) -> String {
        device_key(&self.spec.id, self.serial.as_deref())
    }
}

/// See [`Found::key`].
pub fn device_key(model_id: &str, serial: Option<&str>) -> String {
    let serial = serial.map(str::trim).unwrap_or("");
    let useful = serial
        .chars()
        .any(|c| c.is_ascii_alphanumeric() && c != '0');
    if useful {
        let clean: String = serial
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            .take(64)
            .collect();
        format!("{model_id}-{clean}")
    } else {
        model_id.to_string()
    }
}

/// The dock's command interface. Windows reports the vendor usage page
/// (0xFFA0 on the M18); Linux hidraw may report 0, so fall back to
/// interface 0 there, as Companion does.
fn is_vendor_interface(d: &DeviceInfo) -> bool {
    let page = d.usage_page();
    let iface = d.interface_number();
    page == 0xFFA0 || (page >= 0xFF00 && iface <= 0) || (page == 0 && iface == 0)
}

/// Lists supported devices, one entry per device.
pub fn find(api: &HidApi) -> Vec<Found> {
    // Only the Stream Dock family has a driver so far.
    find_stream_docks(api)
}

fn find_stream_docks(api: &HidApi) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    for d in api.device_list().filter(|d| is_vendor_interface(d)) {
        let Some(model) = models::by_usb(d.vendor_id(), d.product_id()) else {
            continue;
        };
        let serial = d.serial_number().map(str::to_owned);
        // Some systems list one interface more than once.
        if out
            .iter()
            .any(|f| std::ptr::eq(f.model, model) && f.serial == serial && serial.is_some())
        {
            continue;
        }
        out.push(Found {
            family: Family::StreamDock,
            spec: model.spec(),
            model,
            path: d.path().to_owned(),
            serial,
            product: d.product_string().map(str::to_owned),
        });
    }
    out
}

/// One line per HID device, for `--list --all` style debugging.
pub fn describe_all(api: &HidApi, only_vendor_pages: bool) -> Vec<String> {
    api.device_list()
        .filter(|d| {
            !only_vendor_pages
                || d.usage_page() >= 0xFF00
                || models::by_usb(d.vendor_id(), d.product_id()).is_some()
        })
        .map(|d| {
            format!(
                "{:04x}:{:04x}  usage_page=0x{:04x} usage=0x{:02x}  iface={}  \"{}\" / \"{}\"{}",
                d.vendor_id(),
                d.product_id(),
                d.usage_page(),
                d.usage(),
                d.interface_number(),
                d.manufacturer_string().unwrap_or(""),
                d.product_string().unwrap_or(""),
                models::by_usb(d.vendor_id(), d.product_id())
                    .map(|m| format!("  [{}]", m.name))
                    .unwrap_or_default(),
            )
        })
        .collect()
}

/// Opens a found device with its family's driver. Fails with a hint if
/// another app (VSD Craft) holds it.
pub fn open(api: &HidApi, found: &Found) -> Result<Box<dyn Dock>> {
    match found.family {
        Family::StreamDock => Ok(Box::new(open_stream_dock(api, found)?)),
        Family::StreamDeck => Err(Error::Unsupported),
    }
}

/// Opens a found Stream Dock with its own driver type.
pub fn open_stream_dock(api: &HidApi, found: &Found) -> Result<StreamDock<HidTransport>> {
    let dev = api.open_path(&found.path).map_err(|e| {
        Error::Usb(format!(
            "{e}. Is VSD Craft (Stream Dock) still running? Close it and try again."
        ))
    })?;
    Ok(StreamDock::new(
        found.model,
        HidTransport::new(dev, found.model.packet),
    ))
}

pub fn api() -> Result<HidApi> {
    HidApi::new().map_err(|e| Error::Usb(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::device_key;

    #[test]
    fn device_keys_use_real_serials_only() {
        assert_eq!(device_key("M18", Some("A1B2 ")), "M18-A1B2");
        assert_eq!(device_key("M18", Some("0000000")), "M18");
        assert_eq!(device_key("M18", Some("")), "M18");
        assert_eq!(device_key("M18", None), "M18");
        assert_eq!(device_key("N4", Some("x/y\\z")), "N4-xyz");
    }
}
