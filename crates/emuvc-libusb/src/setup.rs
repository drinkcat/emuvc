//! Builds the emulated bus from the environment (see `emuvc_devices::config`).

use emuvc_usb::{Host, HostConfig, SetupPacket};
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
            Ok(info) => {
                info!(
                    "emuvc: {} on bus {:03} device {:03}",
                    info.name, info.bus_number, info.address
                );
                // The configuration the system selected, if not the first.
                if let Some(v) = d.configuration {
                    let set = SetupPacket::new(0x00, 0x09, v as u16, 0, 0);
                    if let Err(e) = host.control(info.id, set, &[], None) {
                        warn!("emuvc: {}: SET_CONFIGURATION {v}: {e}", info.name);
                    }
                }
            }
            Err(e) => warn!("emuvc: {} failed enumeration: {e}", d.model),
        }
    }
    let usbfs = cfg.usbfs_memory_mb as u64 * 1024 * 1024;
    Ok(Bus::new(
        host,
        cfg.kernel_driver,
        usbfs,
        cfg.usbfs_max_urb_bytes.map(u64::from),
    ))
}
