//! Transport-agnostic USB device model and virtual host controller.
//!
//! Layers, bottom up:
//! - [`types`], [`status`], [`desc`]: protocol vocabulary and descriptors.
//! - [`device::UsbDevice`]: what an emulated device is, from the bus' point of view.
//! - [`composite::CompositeDevice`]: chapter 9 framework routing traffic to [`composite::Function`]s.
//! - [`fault::FaultInjector`]: generic fault injection wrapper around any device.
//! - [`host::Host`]: the virtual host controller transports talk to.
//!
//! See `docs/DESIGN.md` at the repository root.

pub mod composite;
pub mod desc;
pub mod device;
pub mod fault;
pub mod host;
pub mod rng;
pub mod status;
pub mod types;

pub use composite::{CompositeDevice, Descriptors, Function};
pub use device::{
    BusTime, ControlReply, Ctx, DeviceEvent, EndpointInfo, InReply, IsoReply, OutReply, UsbDevice,
};
pub use host::{
    ClockMode, Completion, DeviceId, DeviceInfo, Host, HostConfig, Urb, UrbId, UrbKind,
};
pub use status::UsbError;
pub use types::{Direction, SetupPacket, Speed, TransferType};
