//! Finds supported docks on USB.

use hidapi::{DeviceInfo, HidApi};

use crate::models::{self, Model};
use crate::transport::HidTransport;
use crate::{Error, Result, StreamDock};

/// A dock found on USB.
#[derive(Debug, Clone)]
pub struct Found {
    pub model: &'static Model,
    pub path: std::ffi::CString,
    pub serial: Option<String>,
    pub product: Option<String>,
}

/// The dock's command interface. Windows reports the vendor usage page
/// (0xFFA0 on the M18); Linux hidraw may report 0, so fall back to
/// interface 0 there, as Companion does.
fn is_vendor_interface(d: &DeviceInfo) -> bool {
    let page = d.usage_page();
    let iface = d.interface_number();
    page == 0xFFA0 || (page >= 0xFF00 && iface <= 0) || (page == 0 && iface == 0)
}

/// Lists supported docks, one entry per dock.
pub fn find(api: &HidApi) -> Vec<Found> {
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

/// Opens a found dock. Fails with a hint if another app (VSD Craft) holds it.
pub fn open(api: &HidApi, found: &Found) -> Result<StreamDock<HidTransport>> {
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
