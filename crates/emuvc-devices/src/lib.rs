//! Catalogue of emulated devices and their configuration.
//!
//! A device model is a function `&DeviceConfig -> CompositeDevice`: real
//! descriptors, plus functions (UVC, DFU...) tuned with values measured on
//! the real device. [`build`] adds the generic fault injection layer.

pub mod config;
pub mod dfu;
pub mod generic;
pub mod logitech_046d_0843;
pub mod lsusb;
pub mod luxvisions_30c9_0052;
pub mod patch;
pub mod synthetic;

pub use config::{BusConfig, DeviceConfig};

use emuvc_usb::fault::FaultInjector;
use emuvc_usb::{CompositeDevice, UsbDevice};

pub const DEFAULT_MODEL: &str = luxvisions_30c9_0052::NAME;

pub struct Model {
    pub name: &'static str,
    pub description: &'static str,
    pub build: fn(&DeviceConfig) -> Result<CompositeDevice, String>,
}

pub const MODELS: &[Model] = &[
    Model {
        name: luxvisions_30c9_0052::NAME,
        description: luxvisions_30c9_0052::DESCRIPTION,
        build: luxvisions_30c9_0052::build,
    },
    Model {
        name: logitech_046d_0843::NAME,
        description: logitech_046d_0843::DESCRIPTION,
        build: logitech_046d_0843::build,
    },
    Model {
        name: logitech_046d_0843::NAME_UVC15,
        description: logitech_046d_0843::DESCRIPTION_UVC15,
        build: logitech_046d_0843::build_uvc15,
    },
    Model {
        name: generic::NAME,
        description: generic::DESCRIPTION,
        build: generic::build,
    },
    Model {
        name: lsusb::NAME,
        description: lsusb::DESCRIPTION,
        build: lsusb::build,
    },
    Model {
        name: synthetic::NAME,
        description: synthetic::DESCRIPTION,
        build: synthetic::build,
    },
];

/// Builds a configured device, wrapped in a fault injector when it has fault rules.
pub fn build(cfg: &DeviceConfig) -> Result<Box<dyn UsbDevice>, String> {
    let model = MODELS.iter().find(|m| m.name == cfg.model).ok_or_else(|| {
        format!(
            "unknown device model {:?} (known: {})",
            cfg.model,
            MODELS.iter().map(|m| m.name).collect::<Vec<_>>().join(", ")
        )
    })?;
    let mut dev = (model.build)(cfg)?;
    dev.patch_configs(|c| patch::apply(c, &cfg.patches))?;
    Ok(if cfg.faults.is_empty() {
        Box::new(dev)
    } else {
        Box::new(FaultInjector::new(dev, cfg.faults.clone()))
    })
}
