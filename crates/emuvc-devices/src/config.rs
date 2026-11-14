//! Bus/device configuration (TOML file or environment shortcuts).
//!
//! ```toml
//! clock = "realtime"     # or "virtual": run streams as fast as the host reads them
//! seed = 1               # fault injection randomness
//! kernel_driver = true   # libusb shim: pretend uvcvideo is bound to video interfaces
//!
//! [[device]]
//! model = "luxvisions-30c9-0052"
//! pattern = "bars"       # bars | gradient | checkerboard | solid:RRGGBB
//! serial = "0001"
//! uvc = { idle = "zlp", max_payload_transfer_size = 0 }
//!
//! [[device.fault]]
//! on = { iso_in = {} }
//! action = { error = "protocol" }
//! probability = 0.001
//! ```
//!
//! Without a file: `EMUVC_DEVICE=model[,model...]` (default
//! `luxvisions-30c9-0052`; `lsusb:/path/to/lsusb-v.txt[#N]` emulates a device from `lsusb -v` text;
//! `generic:/path/to/descriptors` emulates any
//! device from a raw descriptor dump such as
//! `/sys/bus/usb/devices/<dev>/descriptors`). `EMUVC_CLOCK` and
//! `EMUVC_SEED` override the file.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use emuvc_usb::fault::FaultRule;
use emuvc_usb::{ClockMode, Descriptors, Speed};
use emuvc_uvc::{EventSpec, StreamQuirks};
use emuvc_video::{FrameSource, PatternKind, TestPattern, Threaded};

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BusConfig {
    #[serde(default)]
    pub clock: ClockMode,
    #[serde(default)]
    pub seed: u64,
    #[serde(default = "yes")]
    pub kernel_driver: bool,
    #[serde(default, rename = "device")]
    pub devices: Vec<DeviceConfig>,
}

impl Default for BusConfig {
    fn default() -> Self {
        BusConfig {
            clock: ClockMode::Realtime,
            seed: 0,
            kernel_driver: true,
            devices: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeviceConfig {
    /// Catalogue name (`emuvc models`), or "generic" with `descriptors`.
    pub model: String,
    /// Raw descriptor dump (device descriptor followed by configuration descriptors).
    pub descriptors: Option<PathBuf>,
    /// Which device of an `lsusb -v` text (model "lsusb"); default: the first UVC device.
    pub index: Option<usize>,
    /// Speed override (generic devices default to what bcdUSB suggests).
    pub speed: Option<Speed>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
    /// Test pattern: bars (default), gradient, checkerboard, solid:RRGGBB.
    pub pattern: Option<String>,
    pub jpeg_quality: Option<u8>,
    /// Send MJPEG frames without Huffman tables (DHT), like many webcams.
    pub strip_dht: bool,
    /// Frame stamp (frame counter + CRC band at the top of each frame).
    pub no_stamp: bool,
    pub uvc: StreamQuirks,
    #[serde(rename = "fault")]
    pub faults: Vec<FaultRule>,
    /// Device-initiated events (status interrupts) of the first UVC function.
    #[serde(rename = "event")]
    pub events: Vec<EventSpec>,
    /// Edits of the configuration descriptor the host reads (see [`crate::patch`]).
    #[serde(rename = "patch")]
    pub patches: Vec<crate::patch::DescriptorPatch>,
    /// Options of the "synthetic" model.
    pub synthetic: crate::synthetic::SyntheticConfig,
}

impl DeviceConfig {
    pub fn model(name: &str) -> Self {
        DeviceConfig {
            model: name.to_string(),
            ..Default::default()
        }
    }

    /// `model`, `generic:/path` (raw dump) or `lsusb:/path[#N]` (lsusb -v text, device N).
    pub fn from_spec(spec: &str) -> Self {
        match spec.split_once(':') {
            Some(("generic", path)) => DeviceConfig {
                descriptors: Some(path.into()),
                ..DeviceConfig::model("generic")
            },
            Some(("lsusb", rest)) => {
                let (path, index) = match rest.rsplit_once('#') {
                    Some((p, n)) if n.parse::<usize>().is_ok() => (p, n.parse().ok()),
                    _ => (rest, None),
                };
                DeviceConfig {
                    descriptors: Some(path.into()),
                    index,
                    ..DeviceConfig::model("lsusb")
                }
            }
            _ => DeviceConfig::model(spec),
        }
    }

    pub fn frame_source(&self) -> Box<dyn FrameSource> {
        let kind = match self.pattern.as_deref().unwrap_or("bars") {
            "gradient" => PatternKind::Gradient,
            "checkerboard" => PatternKind::Checkerboard,
            s if s.starts_with("solid:") => {
                let v = u32::from_str_radix(&s[6..], 16).unwrap_or(0x808080);
                PatternKind::Solid {
                    r: (v >> 16) as u8,
                    g: (v >> 8) as u8,
                    b: v as u8,
                }
            }
            _ => PatternKind::ColorBars,
        };
        let pattern = TestPattern::new(kind)
            .with_stamp(!self.no_stamp)
            .with_jpeg_quality(self.jpeg_quality.unwrap_or(85))
            .with_strip_dht(self.strip_dht);
        // Render ahead of the bus, like a camera's ISP pipeline.
        Box::new(Threaded::new(Box::new(pattern)))
    }

    /// Overrides manufacturer/product/serial strings (when the device has them).
    pub fn apply_strings(&self, desc: &mut Descriptors) {
        let Some(dd) = emuvc_usb::desc::DeviceDescriptor::parse(&desc.device) else {
            return;
        };
        for (index, value) in [
            (dd.i_manufacturer, &self.manufacturer),
            (dd.i_product, &self.product),
            (dd.i_serial, &self.serial),
        ] {
            if let (true, Some(v)) = (index != 0, value) {
                desc.strings.insert(index, v.clone());
            }
        }
    }
}

impl BusConfig {
    pub fn from_toml(s: &str) -> Result<Self, String> {
        toml::from_str(s).map_err(|e| e.to_string())
    }

    /// Reads `EMUVC_CONFIG`, `EMUVC_DEVICE`, `EMUVC_CLOCK`, `EMUVC_SEED`.
    pub fn from_env() -> Result<Self, String> {
        let mut cfg = match std::env::var_os("EMUVC_CONFIG") {
            Some(path) => {
                let s = std::fs::read_to_string(&path)
                    .map_err(|e| format!("{}: {e}", path.to_string_lossy()))?;
                Self::from_toml(&s).map_err(|e| format!("{}: {e}", path.to_string_lossy()))?
            }
            None => {
                let models = std::env::var("EMUVC_DEVICE")
                    .unwrap_or_else(|_| crate::DEFAULT_MODEL.to_string());
                let devices = models
                    .split(',')
                    .filter(|m| !m.is_empty())
                    .map(DeviceConfig::from_spec)
                    .collect();
                BusConfig {
                    devices,
                    ..Default::default()
                }
            }
        };
        if let Ok(c) = std::env::var("EMUVC_CLOCK") {
            cfg.clock = match c.as_str() {
                "virtual" => ClockMode::Virtual,
                "realtime" => ClockMode::Realtime,
                other => return Err(format!("EMUVC_CLOCK: unknown clock {other:?}")),
            };
        }
        if let Ok(s) = std::env::var("EMUVC_SEED") {
            cfg.seed = s
                .parse()
                .map_err(|_| format!("EMUVC_SEED: not a number: {s:?}"))?;
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_example() {
        let doc = include_str!("config.rs");
        let toml: String = doc
            .lines()
            .skip_while(|l| !l.starts_with("//! ```toml"))
            .skip(1)
            .take_while(|l| !l.starts_with("//! ```"))
            .map(|l| l.trim_start_matches("//!").trim_start().to_string() + "\n")
            .collect();
        let cfg = BusConfig::from_toml(&toml).unwrap();
        assert_eq!(cfg.devices.len(), 1);
        assert_eq!(cfg.devices[0].uvc.max_payload_transfer_size, Some(0));
        assert_eq!(cfg.devices[0].faults.len(), 1);
    }
}
