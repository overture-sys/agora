//! Finds supported devices on USB, of every family, and opens them with
//! the right driver.

use hidapi::{DeviceInfo, HidApi};

use elgato_streamdeck::info::Kind;

use crate::dock::Dock;
use crate::models::{self, Model};
use crate::spec::{DeviceSpec, Family};
use crate::streamdeck::{self, ElgatoDeck};
use crate::transport::HidTransport;
use crate::{Error, Result, StreamDock};

/// A device found on USB.
#[derive(Debug, Clone)]
pub struct Found {
    pub family: Family,
    pub spec: DeviceSpec,
    /// The Stream Dock table entry, for Stream Docks.
    pub model: Option<&'static Model>,
    /// The Stream Deck model, for Stream Decks.
    pub deck: Option<Kind>,
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
    let mut out = find_stream_docks(api);
    out.extend(find_stream_decks(api));
    out
}

fn find_stream_decks(api: &HidApi) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    for d in api.device_list() {
        let Some(kind) = Kind::from_vid_pid(d.vendor_id(), d.product_id()) else {
            continue;
        };
        if !streamdeck::supported(kind) {
            continue;
        }
        let serial = d.serial_number().map(str::to_owned);
        if out
            .iter()
            .any(|f| f.deck == Some(kind) && f.serial == serial)
        {
            continue;
        }
        out.push(Found {
            family: Family::StreamDeck,
            spec: streamdeck::spec(kind),
            model: None,
            deck: Some(kind),
            path: d.path().to_owned(),
            serial,
            product: d.product_string().map(str::to_owned),
        });
    }
    out
}

fn find_stream_docks(api: &HidApi) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    // Where a model shows its command interface on 0xFFA0 (the M18 on
    // Windows), other vendor pages of it are extra collections of the same
    // dock, not more docks.
    let exact: Vec<(u16, u16)> = api
        .device_list()
        .filter(|d| d.usage_page() == 0xFFA0)
        .map(|d| (d.vendor_id(), d.product_id()))
        .collect();
    for d in api.device_list().filter(|d| {
        is_vendor_interface(d)
            && (d.usage_page() == 0xFFA0 || !exact.contains(&(d.vendor_id(), d.product_id())))
    }) {
        let Some(model) = models::by_usb(d.vendor_id(), d.product_id()) else {
            continue;
        };
        let serial = d.serial_number().map(str::to_owned);
        // Some systems list one interface more than once.
        if out
            .iter()
            .any(|f| f.model.is_some_and(|m| std::ptr::eq(m, model)) && f.serial == serial && serial.is_some())
        {
            continue;
        }
        out.push(Found {
            family: Family::StreamDock,
            spec: model.spec(),
            model: Some(model),
            deck: None,
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
                || Kind::from_vid_pid(d.vendor_id(), d.product_id()).is_some()
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
                    .map(|m| m.name.to_string())
                    .or_else(|| {
                        Kind::from_vid_pid(d.vendor_id(), d.product_id())
                            .map(|k| streamdeck::spec(k).name)
                    })
                    .map(|n| format!("  [{n}]"))
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
        Family::StreamDeck => {
            let kind = found.deck.ok_or(Error::Unsupported)?;
            // The crate opens a deck by its serial number.
            let serial = found.serial.as_deref().filter(|s| !s.is_empty()).ok_or_else(|| {
                Error::Usb(format!("{} has no serial number, so it can't be opened", found.spec.name))
            })?;
            Ok(Box::new(ElgatoDeck::connect(api, kind, serial)?))
        }
    }
}

/// Opens a found Stream Dock with its own driver type.
pub fn open_stream_dock(api: &HidApi, found: &Found) -> Result<StreamDock<HidTransport>> {
    let model = found.model.ok_or(Error::Unsupported)?;
    let dev = api.open_path(&found.path).map_err(|e| {
        Error::Usb(format!(
            "{e}. Is VSD Craft (Stream Dock) still running? Close it and try again."
        ))
    })?;
    Ok(StreamDock::new(
        model,
        HidTransport::new(dev, model.packet),
    ))
}

/// A model of either family by its short id ("M18", "SDMINI").
pub fn spec_by_id(id: &str) -> Option<DeviceSpec> {
    models::by_id(id)
        .map(Model::spec)
        .or_else(|| streamdeck::by_id(id).map(streamdeck::spec))
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
