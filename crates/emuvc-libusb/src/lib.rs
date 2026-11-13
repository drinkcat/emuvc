//! LD_PRELOAD-able libusb-1.0 replacement backed by emuvc's emulated devices.
//!
//! Every public libusb 1.0.30 symbol is exported. Programs see the devices
//! configured through the environment (see `setup.rs`) instead of real ones;
//! their traffic goes to an `emuvc_usb::Host`.

// The exported functions' safety contracts are libusb's documented API.
#![allow(clippy::missing_safety_doc)]

pub mod core;
pub mod descriptors;
pub mod faults;
pub mod ffi;
mod misc;
mod setup;
mod stubs;
pub mod transfer;

/// Logs go to stderr, filtered by `EMUVC_LOG` (env_logger syntax, default "warn").
pub(crate) fn init_logging() {
    let _ = env_logger::Builder::from_env(env_logger::Env::new().filter_or("EMUVC_LOG", "warn"))
        .format_timestamp_millis()
        .try_init();
}
