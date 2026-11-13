//! Emulates any device from a raw descriptor dump.
//!
//! Input: the device descriptor followed by configuration descriptors — the
//! format of `/sys/bus/usb/devices/<dev>/descriptors`. Every VideoControl
//! interface becomes a [`UvcFunction`] with default control ranges; other
//! interfaces exist (SET_INTERFACE works) but stall class requests.

use emuvc_usb::desc::{ConfigHeader, ConfigView, DeviceDescriptor};
use emuvc_usb::{CompositeDevice, Descriptors, Speed};
use emuvc_uvc::UvcFunction;
use emuvc_uvc::topology::{find_video_control_interfaces, parse_function};

use crate::config::DeviceConfig;

pub const NAME: &str = "generic";
pub const DESCRIPTION: &str = "any device from a raw descriptor dump (descriptors = \"path\")";

/// Splits a dump into the device descriptor and configuration descriptors.
pub fn split_dump(bytes: &[u8]) -> Result<(Vec<u8>, Vec<Vec<u8>>), String> {
    let dd = DeviceDescriptor::parse(bytes).ok_or("dump doesn't start with a device descriptor")?;
    let mut configs = Vec::new();
    let mut rest = &bytes[18.min(bytes.len())..];
    while configs.len() < dd.num_configurations as usize && !rest.is_empty() {
        let h = ConfigHeader::parse(rest).ok_or("malformed configuration descriptor")?;
        let n = (h.total_length as usize).clamp(9, rest.len());
        configs.push(rest[..n].to_vec());
        rest = &rest[n..];
    }
    if configs.is_empty() {
        return Err("no configuration descriptor in dump".into());
    }
    Ok((bytes[..18].to_vec(), configs))
}

pub fn build(cfg: &DeviceConfig) -> Result<CompositeDevice, String> {
    let path = cfg
        .descriptors
        .as_ref()
        .ok_or("generic device needs `descriptors`")?;
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    from_dump(&bytes, cfg)
}

pub fn from_dump(bytes: &[u8], cfg: &DeviceConfig) -> Result<CompositeDevice, String> {
    let (device, configs) = split_dump(bytes)?;
    let dd = DeviceDescriptor::parse(&device).unwrap();
    let speed = cfg.speed.unwrap_or(match dd.bcd_usb {
        0x0300.. => Speed::Super,
        0x0200.. => Speed::High,
        _ => Speed::Full,
    });
    let mut desc = Descriptors {
        device,
        configs,
        langids: vec![0x0409],
        ..Default::default()
    };
    // Placeholder strings so hosts reading them get something sensible.
    let name = format!("{:04x}:{:04x}", dd.vendor_id, dd.product_id);
    for (i, s) in [
        (dd.i_manufacturer, "emuvc"),
        (dd.i_product, name.as_str()),
        (dd.i_serial, "0001"),
    ] {
        if i != 0 {
            desc.strings.insert(i, s.to_string());
        }
    }
    cfg.apply_strings(&mut desc);
    let view = ConfigView::parse(&desc.configs[0]).map_err(|e| e.to_string())?;
    let mut dev = CompositeDevice::new(format!("generic-{name}"), speed, desc);
    for vc in find_video_control_interfaces(&view) {
        let f = parse_function(&view, vc, speed).map_err(|e| format!("VC interface {vc}: {e}"))?;
        let uvc = UvcFunction::new(f, speed, cfg.uvc.clone(), &mut |_| cfg.frame_source());
        dev.add_function(&uvc.interfaces(), Box::new(uvc));
    }
    Ok(dev)
}
