//! Finds supported docks on USB.

use hidapi::{DeviceInfo, HidApi};

use crate::transport::HidTransport;
use crate::{m18, Error, Result, M18};

/// A dock found on USB.
#[derive(Debug, Clone)]
pub struct Found {
    pub model: &'static str,
    pub path: std::ffi::CString,
    pub serial: Option<String>,
    pub product: Option<String>,
}

fn is_m18_vendor_interface(d: &DeviceInfo) -> bool {
    d.vendor_id() == m18::VID
        && d.product_id() == m18::PID
        // Windows reports the usage page. Linux hidraw may report 0, so fall
        // back to interface 0 there.
        && (d.usage_page() == m18::USAGE_PAGE || (d.usage_page() == 0 && d.interface_number() == 0))
}

/// Lists supported docks. Only the M18 for now.
pub fn find(api: &HidApi) -> Vec<Found> {
    api.device_list()
        .filter(|d| is_m18_vendor_interface(d))
        .map(|d| Found {
            model: "M18",
            path: d.path().to_owned(),
            serial: d.serial_number().map(str::to_owned),
            product: d.product_string().map(str::to_owned),
        })
        .collect()
}

/// One line per HID device, for `--list --all` style debugging.
pub fn describe_all(api: &HidApi, only_vendor_pages: bool) -> Vec<String> {
    api.device_list()
        .filter(|d| !only_vendor_pages || d.usage_page() >= 0xFF00 || d.vendor_id() == m18::VID)
        .map(|d| {
            format!(
                "{:04x}:{:04x}  usage_page=0x{:04x} usage=0x{:02x}  iface={}  \"{}\" / \"{}\"",
                d.vendor_id(),
                d.product_id(),
                d.usage_page(),
                d.usage(),
                d.interface_number(),
                d.manufacturer_string().unwrap_or(""),
                d.product_string().unwrap_or(""),
            )
        })
        .collect()
}

/// Opens a found M18. Fails with a hint if another app (VSD Craft) holds it.
pub fn open_m18(api: &HidApi, found: &Found) -> Result<M18<HidTransport>> {
    let dev = api.open_path(&found.path).map_err(|e| {
        Error::Usb(format!("{e}. Is VSD Craft (Stream Dock) still running? Close it and try again."))
    })?;
    Ok(M18::new(HidTransport::new(dev, m18::OUT_LEN)))
}

pub fn api() -> Result<HidApi> {
    HidApi::new().map_err(|e| Error::Usb(e.to_string()))
}
