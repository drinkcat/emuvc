//! The device side of the emulator: what a transport/host engine talks to.
//!
//! A [`UsbDevice`] is a passive state machine. It never spawns threads or
//! sleeps: the host engine calls it with the current bus time, and it answers
//! immediately. That keeps device models deterministic and lets the same model
//! run in real time (libusb shim, USB/IP) or in virtual time (tests).

use crate::rng::Rng;
use crate::status::UsbError;
use crate::types::{SetupPacket, Speed, TransferType};

/// Bus time, in 125 µs microframes since the virtual bus started.
pub type BusTime = u64;

pub const UFRAME_NS: u64 = 125_000;

/// Converts a duration in nanoseconds to whole microframes (rounding up).
pub fn ns_to_uframes(ns: u64) -> BusTime {
    ns.div_ceil(UFRAME_NS)
}

/// Things a device can ask the host to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceEvent {
    /// The device drops off the bus (cable pulled, firmware crash).
    Disconnect,
}

/// Per-call context handed to devices by the host engine.
pub struct Ctx<'a> {
    now: BusTime,
    rng: &'a mut Rng,
    events: &'a mut Vec<DeviceEvent>,
}

impl<'a> Ctx<'a> {
    pub fn new(now: BusTime, rng: &'a mut Rng, events: &'a mut Vec<DeviceEvent>) -> Self {
        Ctx { now, rng, events }
    }

    /// Current bus time. For isochronous calls, the (micro)frame being serviced.
    pub fn now(&self) -> BusTime {
        self.now
    }

    pub fn now_ns(&self) -> u64 {
        self.now * UFRAME_NS
    }

    /// Deterministic per-device random numbers (seeded from the host seed).
    pub fn rng(&mut self) -> &mut Rng {
        self.rng
    }

    pub fn emit(&mut self, event: DeviceEvent) {
        self.events.push(event);
    }

    /// Re-borrows the context with a different current time.
    pub fn at(&mut self, now: BusTime) -> Ctx<'_> {
        Ctx {
            now,
            rng: self.rng,
            events: self.events,
        }
    }
}

/// Answer to a control transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlReply {
    /// IN: data returned (truncated to wLength by the host). OUT: empty on success.
    /// `Err(UsbError::Timeout)` means the device never completes the status stage.
    pub result: Result<Vec<u8>, UsbError>,
    /// How long the device NAKs before answering, in microseconds.
    pub latency_us: u32,
}

impl ControlReply {
    pub fn data(data: Vec<u8>) -> Self {
        ControlReply {
            result: Ok(data),
            latency_us: 0,
        }
    }

    pub fn ack() -> Self {
        Self::data(Vec::new())
    }

    pub fn stall() -> Self {
        Self::error(UsbError::Stall)
    }

    pub fn error(e: UsbError) -> Self {
        ControlReply {
            result: Err(e),
            latency_us: 0,
        }
    }

    pub fn with_latency_us(mut self, us: u32) -> Self {
        self.latency_us = us;
        self
    }
}

/// Answer to a bulk or interrupt IN transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InReply {
    /// One complete transfer (terminated by a short packet or exactly filling
    /// the host's buffer). More bytes than requested is reported as babble.
    Data(Vec<u8>),
    /// Nothing to send yet; the host will poll again.
    Nak,
    Error(UsbError),
}

/// Answer to a bulk or interrupt OUT transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutReply {
    Ack,
    Nak,
    Error(UsbError),
}

/// Answer for one isochronous service interval.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IsoReply {
    pub data: Vec<u8>,
    pub error: Option<UsbError>,
}

impl IsoReply {
    pub fn data(data: Vec<u8>) -> Self {
        IsoReply { data, error: None }
    }
}

/// Scheduling information about an endpoint of the current configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointInfo {
    pub address: u8,
    pub transfer_type: TransferType,
    /// Service interval in microframes (periodic endpoints).
    pub interval: u32,
    /// Maximum bytes per service interval (iso/interrupt) or per packet (bulk).
    pub max_bytes: usize,
}

/// An emulated USB device, as seen from the bus.
///
/// Implementations must be quick and non-blocking; all calls happen on the
/// host engine thread.
pub trait UsbDevice: Send {
    /// Short human readable name, for logs.
    fn name(&self) -> String;

    fn speed(&self) -> Speed;

    /// Port reset. The device returns to the default (unaddressed, unconfigured) state.
    fn reset(&mut self, cx: &mut Ctx);

    /// A control transfer on endpoint 0. `data` is the OUT data stage.
    fn control(&mut self, cx: &mut Ctx, setup: &SetupPacket, data: &[u8]) -> ControlReply;

    /// Endpoints usable in the current configuration/alternate settings.
    fn endpoint(&self, address: u8) -> Option<EndpointInfo>;

    fn data_in(&mut self, _cx: &mut Ctx, _ep: u8, _max_len: usize) -> InReply {
        InReply::Error(UsbError::Stall)
    }

    fn data_out(&mut self, _cx: &mut Ctx, _ep: u8, _data: &[u8]) -> OutReply {
        OutReply::Error(UsbError::Stall)
    }

    /// One isochronous IN service interval; `cx.now()` is its (micro)frame.
    fn iso_in(&mut self, _cx: &mut Ctx, _ep: u8, _max_len: usize) -> IsoReply {
        IsoReply::default()
    }

    fn iso_out(&mut self, _cx: &mut Ctx, _ep: u8, _data: &[u8]) -> IsoReply {
        IsoReply::default()
    }

    /// Next bus time at which the device's state changes on its own (a frame
    /// becomes ready, a scheduled event fires). The engine polls pending
    /// bulk/interrupt transfers and calls [`UsbDevice::tick`] then.
    fn next_wakeup(&self) -> Option<BusTime> {
        None
    }

    fn tick(&mut self, _cx: &mut Ctx) {}
}

impl UsbDevice for Box<dyn UsbDevice> {
    fn name(&self) -> String {
        (**self).name()
    }
    fn speed(&self) -> Speed {
        (**self).speed()
    }
    fn reset(&mut self, cx: &mut Ctx) {
        (**self).reset(cx)
    }
    fn control(&mut self, cx: &mut Ctx, setup: &SetupPacket, data: &[u8]) -> ControlReply {
        (**self).control(cx, setup, data)
    }
    fn endpoint(&self, address: u8) -> Option<EndpointInfo> {
        (**self).endpoint(address)
    }
    fn data_in(&mut self, cx: &mut Ctx, ep: u8, max_len: usize) -> InReply {
        (**self).data_in(cx, ep, max_len)
    }
    fn data_out(&mut self, cx: &mut Ctx, ep: u8, data: &[u8]) -> OutReply {
        (**self).data_out(cx, ep, data)
    }
    fn iso_in(&mut self, cx: &mut Ctx, ep: u8, max_len: usize) -> IsoReply {
        (**self).iso_in(cx, ep, max_len)
    }
    fn iso_out(&mut self, cx: &mut Ctx, ep: u8, data: &[u8]) -> IsoReply {
        (**self).iso_out(cx, ep, data)
    }
    fn next_wakeup(&self) -> Option<BusTime> {
        (**self).next_wakeup()
    }
    fn tick(&mut self, cx: &mut Ctx) {
        (**self).tick(cx)
    }
}
