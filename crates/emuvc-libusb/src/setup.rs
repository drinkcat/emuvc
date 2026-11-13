//! Builds the emulated bus from the environment (see `emuvc_devices::config`).

use emuvc_usb::{Host, HostConfig};
use log::{info, warn};

use crate::core::Bus;

pub fn create_bus() -> Result<Bus, String> {
    let cfg = emuvc_devices::BusConfig::from_env()?;
    let host = Host::new(HostConfig {
        clock: cfg.clock,
        seed: cfg.seed,
        ..Default::default()
    });
    for d in &cfg.devices {
        let dev = emuvc_devices::build(d)?;
        // Like real hardware, a device failing enumeration just doesn't show up.
        match host.attach(dev) {
            Ok(info) => info!(
                "emuvc: {} on bus {:03} device {:03}",
                info.name, info.bus_number, info.address
            ),
            Err(e) => warn!("emuvc: {} failed enumeration: {e}", d.model),
        }
    }
    Ok(Bus::new(host, cfg.kernel_driver))
}
