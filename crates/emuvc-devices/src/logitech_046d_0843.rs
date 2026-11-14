//! Logitech Webcam C930e (`046d:0843`), high speed, with two configurations:
//!
//! 1. UVC 1.0: YUY2 (19 frames) and MJPEG (17 frames) on interface 1, a UAC1
//!    microphone on interfaces 2+3.
//! 2. UVC 1.5: the same plus an encoding unit (ID 5, between the processing unit
//!    and output terminal 7) feeding a second VideoStreaming interface (2) with
//!    H.264 (34 frames); the microphone moves to interfaces 3+4.
//!
//! Hosts take configuration index 0. On the real camera that is the UVC 1.0 one,
//! as UVC 1.5 3.4.1 requires (configurations listed by increasing version), so
//! libuvc (which only ever parses index 0) never sees the encoding unit.
//! [`NAME_UVC15`] keeps only the UVC 1.5 configuration (renumbered 1): a
//! compliant UVC 1.5 camera with an encoding unit, whose YUY2/MJPEG interface a
//! host that doesn't know encoding units can still stream from.
//!
//! Descriptors: `tests/fixtures/046d_0843.descriptors.bin`, rebuilt from the
//! `lsusb -v` text `tests/fixtures/lsusb/046d_0843.txt` (a public dump of a
//! real camera: <https://gist.github.com/dalbani/08f295fe436b76fa70bd29e1f51321cc>,
//! the serial number replaced in the model) plus the 5-byte
//! class-specific VC interrupt endpoint descriptor of each configuration,
//! which that lsusb didn't print but wTotalLength counts. Controls get the
//! generic default ranges (see [`crate::generic`]); the H.264 interface has no
//! streamable format yet.

use emuvc_usb::{CompositeDevice, Descriptors, Speed};

use crate::config::DeviceConfig;
use crate::generic::{from_descriptors, split_dump};

pub const NAME: &str = "logitech-046d-0843";
pub const DESCRIPTION: &str =
    "Logitech Webcam C930e: UVC 1.0 configuration first, UVC 1.5 + H.264 encoding unit second";
pub const NAME_UVC15: &str = "logitech-046d-0843-uvc15";
pub const DESCRIPTION_UVC15: &str =
    "Logitech Webcam C930e with only its UVC 1.5 configuration (H.264 encoding unit)";

/// Raw dump (device descriptor, then configurations 1 and 2).
pub const DUMP: &[u8] = include_bytes!("../../../tests/fixtures/046d_0843.descriptors.bin");

const STR_SERIAL: u8 = 1;
const STR_PRODUCT: u8 = 2;

/// Descriptors: both configurations, or only the UVC 1.5 one if `uvc15_only`.
pub fn descriptors(uvc15_only: bool) -> Descriptors {
    let (mut device, mut configs) = split_dump(DUMP).expect("valid C930e dump");
    if uvc15_only {
        configs.remove(0);
        configs[0][5] = 1; // bConfigurationValue
        device[17] = 1; // bNumConfigurations
    }
    Descriptors {
        device,
        configs,
        strings: [
            (STR_SERIAL, "C930E001".to_string()),
            (STR_PRODUCT, "Logitech Webcam C930e".to_string()),
        ]
        .into(),
        langids: vec![0x0409],
        ..Default::default()
    }
}

fn build_with(cfg: &DeviceConfig, uvc15_only: bool, name: &str) -> Result<CompositeDevice, String> {
    let mut dev = from_descriptors(descriptors(uvc15_only), Some(Speed::High), cfg)?;
    dev.set_name(name);
    Ok(dev)
}

pub fn build(cfg: &DeviceConfig) -> Result<CompositeDevice, String> {
    build_with(cfg, false, NAME)
}

pub fn build_uvc15(cfg: &DeviceConfig) -> Result<CompositeDevice, String> {
    build_with(cfg, true, NAME_UVC15)
}
