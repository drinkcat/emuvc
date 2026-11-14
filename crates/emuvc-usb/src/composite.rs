//! A generic USB device assembled from descriptors and *functions*.
//!
//! [`CompositeDevice`] implements the USB 2.0 chapter 9 state machine
//! (descriptors, addressing, configurations, alternate settings, endpoint
//! halt) and routes class/vendor requests and endpoint traffic to the
//! [`Function`] that owns the interface, e.g. a UVC camera function, a DFU
//! stub or a HID button. Device models are mostly "descriptors + functions".

use std::collections::{BTreeMap, BTreeSet};

use crate::desc::{self, ConfigView, DeviceDescriptor, dtype};
use crate::device::{
    BusTime, ControlReply, Ctx, EndpointInfo, InReply, IsoReply, OutReply, UsbDevice,
};
use crate::status::UsbError;
use crate::types::{
    Direction, Recipient, RequestKind, SetupPacket, Speed, TransferType, feature, std_request,
};

/// One USB function (set of interfaces) of a composite device.
///
/// All methods have "not supported" defaults so simple functions only
/// implement what they need.
pub trait Function: Send {
    /// Class/vendor request addressed to one of this function's interfaces or
    /// endpoints (wIndex identifies which).
    fn control(&mut self, _cx: &mut Ctx, _setup: &SetupPacket, _data: &[u8]) -> ControlReply {
        ControlReply::stall()
    }

    /// SET_INTERFACE for one of this function's interfaces, already validated
    /// against the descriptors. Returning an error stalls the request.
    fn set_alt(&mut self, _cx: &mut Ctx, _iface: u8, _alt: u8) -> Result<(), UsbError> {
        Ok(())
    }

    /// Bus reset or new configuration: every interface is back to alt 0.
    fn reset(&mut self, _cx: &mut Ctx) {}

    fn data_in(&mut self, _cx: &mut Ctx, _ep: u8, _max_len: usize) -> InReply {
        InReply::Error(UsbError::Stall)
    }

    fn data_out(&mut self, _cx: &mut Ctx, _ep: u8, _data: &[u8]) -> OutReply {
        OutReply::Error(UsbError::Stall)
    }

    fn iso_in(&mut self, _cx: &mut Ctx, _ep: u8, _max_len: usize) -> IsoReply {
        IsoReply::default()
    }

    fn iso_out(&mut self, _cx: &mut Ctx, _ep: u8, _data: &[u8]) -> IsoReply {
        IsoReply::default()
    }

    /// CLEAR_FEATURE(ENDPOINT_HALT) on one of this function's endpoints.
    fn clear_halt(&mut self, _cx: &mut Ctx, _ep: u8) {}

    fn next_wakeup(&self) -> Option<BusTime> {
        None
    }

    fn tick(&mut self, _cx: &mut Ctx) {}
}

/// Descriptor set of a device, as raw bytes.
#[derive(Clone, Debug, Default)]
pub struct Descriptors {
    /// The 18-byte device descriptor.
    pub device: Vec<u8>,
    /// Full configuration descriptors (index order).
    pub configs: Vec<Vec<u8>>,
    pub bos: Option<Vec<u8>>,
    /// String descriptors by index (index 0, the LANGID list, is built from `langids`).
    pub strings: BTreeMap<u8, String>,
    pub langids: Vec<u16>,
    /// Raw string descriptor overrides, for devices returning odd strings.
    pub raw_strings: BTreeMap<u8, Vec<u8>>,
}

struct Slot {
    interfaces: Vec<u8>,
    func: Box<dyn Function>,
}

/// See the module documentation.
pub struct CompositeDevice {
    name: String,
    speed: Speed,
    desc: Descriptors,
    views: Vec<ConfigView>,
    slots: Vec<Slot>,
    /// Receives class/vendor requests addressed to the device itself.
    device_slot: Option<usize>,
    self_powered: bool,
    // Chapter 9 state.
    address: u8,
    config: u8,
    alts: BTreeMap<u8, u8>,
    halted: BTreeSet<u8>,
    remote_wakeup: bool,
    /// Endpoint address -> (owning slot, info), for the current alt settings.
    eps: BTreeMap<u8, (Option<usize>, EndpointInfo)>,
}

impl CompositeDevice {
    pub fn new(name: impl Into<String>, speed: Speed, desc: Descriptors) -> Self {
        let views = desc
            .configs
            .iter()
            .filter_map(|c| ConfigView::parse(c).ok())
            .collect();
        let self_powered = desc
            .configs
            .first()
            .is_some_and(|c| c.len() > 7 && c[7] & 0x40 != 0);
        CompositeDevice {
            name: name.into(),
            speed,
            desc,
            views,
            slots: Vec::new(),
            device_slot: None,
            self_powered,
            address: 0,
            config: 0,
            alts: BTreeMap::new(),
            halted: BTreeSet::new(),
            remote_wakeup: false,
            eps: BTreeMap::new(),
        }
    }

    /// Registers a function owning `interfaces`.
    pub fn add_function(&mut self, interfaces: &[u8], func: Box<dyn Function>) -> &mut Self {
        self.slots.push(Slot {
            interfaces: interfaces.to_vec(),
            func,
        });
        self
    }

    /// Registers a function receiving class/vendor requests sent to the device recipient.
    pub fn set_device_function(&mut self, func: Box<dyn Function>) -> &mut Self {
        self.slots.push(Slot {
            interfaces: Vec::new(),
            func,
        });
        self.device_slot = Some(self.slots.len() - 1);
        self
    }

    pub fn descriptors(&self) -> &Descriptors {
        &self.desc
    }

    /// Rewrites the configuration descriptors the host reads. The device keeps
    /// behaving as the original descriptors say (interfaces, alternate
    /// settings, endpoints): for malformed-descriptor tests.
    pub fn patch_configs<E>(
        &mut self,
        mut f: impl FnMut(&mut Vec<u8>) -> Result<(), E>,
    ) -> Result<(), E> {
        self.desc.configs.iter_mut().try_for_each(&mut f)
    }

    fn view(&self) -> Option<&ConfigView> {
        self.views
            .iter()
            .find(|v| v.header.value == self.config && self.config != 0)
    }

    fn slot_for_interface(&self, iface: u8) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.interfaces.contains(&iface))
    }

    fn rebuild_endpoints(&mut self) {
        let mut eps = BTreeMap::new();
        if let Some(view) = self.view() {
            for iface in &view.interfaces {
                let alt = self.alts.get(&iface.number).copied().unwrap_or(0);
                let Some(a) = iface.alts.iter().find(|a| a.desc.alt == alt) else {
                    continue;
                };
                let slot = self
                    .slots
                    .iter()
                    .position(|s| s.interfaces.contains(&iface.number));
                for ep in &a.endpoints {
                    let info = EndpointInfo {
                        address: ep.desc.address,
                        transfer_type: ep.desc.transfer_type(),
                        interval: ep.desc.interval_uframes(self.speed),
                        max_bytes: ep.bytes_per_interval(self.speed),
                    };
                    eps.insert(ep.desc.address, (slot, info));
                }
            }
        }
        self.eps = eps;
    }

    fn reset_functions(&mut self, cx: &mut Ctx) {
        for s in &mut self.slots {
            s.func.reset(cx);
        }
    }

    fn get_descriptor(&self, setup: &SetupPacket) -> ControlReply {
        let (ty, index) = ((setup.value >> 8) as u8, setup.value as u8);
        let data = match ty {
            dtype::DEVICE => Some(self.desc.device.clone()),
            dtype::CONFIG => self.desc.configs.get(index as usize).cloned(),
            dtype::STRING if index == 0 => {
                let langs = if self.desc.langids.is_empty() {
                    vec![0x0409]
                } else {
                    self.desc.langids.clone()
                };
                Some(
                    self.desc
                        .raw_strings
                        .get(&0)
                        .cloned()
                        .unwrap_or_else(|| desc::langid_descriptor(&langs)),
                )
            }
            dtype::STRING => self.desc.raw_strings.get(&index).cloned().or_else(|| {
                self.desc
                    .strings
                    .get(&index)
                    .map(|s| desc::string_descriptor(s))
            }),
            dtype::BOS => self.desc.bos.clone(),
            dtype::DEVICE_QUALIFIER if self.speed == Speed::High => {
                DeviceDescriptor::parse(&self.desc.device).map(|d| {
                    let b = d.bcd_usb.to_le_bytes();
                    vec![
                        10,
                        dtype::DEVICE_QUALIFIER,
                        b[0],
                        b[1],
                        d.class,
                        d.subclass,
                        d.protocol,
                        d.max_packet_size0,
                        d.num_configurations,
                        0,
                    ]
                })
            }
            _ => None,
        };
        match data {
            Some(mut d) => {
                d.truncate(setup.length as usize);
                ControlReply::data(d)
            }
            None => ControlReply::stall(),
        }
    }

    fn standard(&mut self, cx: &mut Ctx, setup: &SetupPacket, data: &[u8]) -> ControlReply {
        use std_request::*;
        let rcpt = setup.recipient();
        let (lo, val) = (setup.index as u8, setup.value);
        match (setup.request, rcpt) {
            (GET_DESCRIPTOR, Recipient::Device) => self.get_descriptor(setup),
            (GET_DESCRIPTOR, Recipient::Interface) => self.route_interface(cx, setup, data),
            (SET_ADDRESS, Recipient::Device) => {
                self.address = (val & 0x7f) as u8;
                ControlReply::ack()
            }
            (GET_CONFIGURATION, Recipient::Device) => ControlReply::data(vec![self.config]),
            (SET_CONFIGURATION, Recipient::Device) => {
                let value = val as u8;
                if value != 0 && !self.views.iter().any(|v| v.header.value == value) {
                    return ControlReply::stall();
                }
                self.config = value;
                self.alts.clear();
                self.halted.clear();
                self.reset_functions(cx);
                self.rebuild_endpoints();
                ControlReply::ack()
            }
            (GET_INTERFACE, Recipient::Interface) => {
                match self.view().and_then(|v| v.interface(lo)) {
                    Some(_) => ControlReply::data(vec![self.alts.get(&lo).copied().unwrap_or(0)]),
                    None => ControlReply::stall(),
                }
            }
            (SET_INTERFACE, Recipient::Interface) => {
                let alt = val as u8;
                let Some(view) = self.view() else {
                    return ControlReply::stall();
                };
                let Some(iface) = view.interface(lo) else {
                    return ControlReply::stall();
                };
                if !iface.alts.iter().any(|a| a.desc.alt == alt) {
                    return ControlReply::stall();
                }
                let eps: Vec<u8> = iface
                    .alts
                    .iter()
                    .flat_map(|a| a.endpoints.iter().map(|e| e.desc.address))
                    .collect();
                if let Some(slot) = self.slot_for_interface(lo)
                    && self.slots[slot].func.set_alt(cx, lo, alt).is_err()
                {
                    return ControlReply::stall();
                }
                self.alts.insert(lo, alt);
                for ep in eps {
                    self.halted.remove(&ep);
                }
                self.rebuild_endpoints();
                ControlReply::ack()
            }
            (GET_STATUS, Recipient::Device) => ControlReply::data(vec![
                self.self_powered as u8 | (self.remote_wakeup as u8) << 1,
                0,
            ]),
            (GET_STATUS, Recipient::Interface) => match self.view().and_then(|v| v.interface(lo)) {
                Some(_) => ControlReply::data(vec![0, 0]),
                None => ControlReply::stall(),
            },
            (GET_STATUS, Recipient::Endpoint) => {
                if lo & 0x7f == 0 || self.eps.contains_key(&lo) {
                    ControlReply::data(vec![self.halted.contains(&lo) as u8, 0])
                } else {
                    ControlReply::stall()
                }
            }
            (CLEAR_FEATURE, Recipient::Endpoint) if val == feature::ENDPOINT_HALT => {
                if lo & 0x7f == 0 {
                    return ControlReply::ack();
                }
                match self.eps.get(&lo) {
                    Some(&(slot, _)) => {
                        self.halted.remove(&lo);
                        if let Some(s) = slot {
                            self.slots[s].func.clear_halt(cx, lo);
                        }
                        ControlReply::ack()
                    }
                    None => ControlReply::stall(),
                }
            }
            (SET_FEATURE, Recipient::Endpoint) if val == feature::ENDPOINT_HALT => {
                match self.eps.get(&lo) {
                    Some((_, info))
                        if matches!(
                            info.transfer_type,
                            TransferType::Bulk | TransferType::Interrupt
                        ) =>
                    {
                        self.halted.insert(lo);
                        ControlReply::ack()
                    }
                    _ => ControlReply::stall(),
                }
            }
            (CLEAR_FEATURE, Recipient::Device) if val == feature::DEVICE_REMOTE_WAKEUP => {
                self.remote_wakeup = false;
                ControlReply::ack()
            }
            (SET_FEATURE, Recipient::Device) if val == feature::DEVICE_REMOTE_WAKEUP => {
                self.remote_wakeup = true;
                ControlReply::ack()
            }
            (SET_FEATURE, Recipient::Device) if val == feature::TEST_MODE => ControlReply::ack(),
            (SET_SEL | SET_ISOCH_DELAY, Recipient::Device) => ControlReply::ack(),
            (SYNCH_FRAME, Recipient::Endpoint) => self.route_endpoint(cx, setup, data),
            _ => ControlReply::stall(),
        }
    }

    fn route_interface(&mut self, cx: &mut Ctx, setup: &SetupPacket, data: &[u8]) -> ControlReply {
        match self.slot_for_interface(setup.index as u8) {
            Some(s) => self.slots[s].func.control(cx, setup, data),
            None => ControlReply::stall(),
        }
    }

    fn route_endpoint(&mut self, cx: &mut Ctx, setup: &SetupPacket, data: &[u8]) -> ControlReply {
        match self.eps.get(&(setup.index as u8)) {
            Some(&(Some(s), _)) => self.slots[s].func.control(cx, setup, data),
            _ => ControlReply::stall(),
        }
    }

    fn slot_for_endpoint(&self, ep: u8) -> Result<usize, UsbError> {
        match self.eps.get(&ep) {
            Some(&(Some(s), _)) => Ok(s),
            Some(&(None, _)) => Err(UsbError::Stall),
            None => Err(UsbError::Shutdown),
        }
    }
}

impl UsbDevice for CompositeDevice {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn speed(&self) -> Speed {
        self.speed
    }

    fn reset(&mut self, cx: &mut Ctx) {
        self.address = 0;
        self.config = 0;
        self.alts.clear();
        self.halted.clear();
        self.remote_wakeup = false;
        self.reset_functions(cx);
        self.rebuild_endpoints();
    }

    fn control(&mut self, cx: &mut Ctx, setup: &SetupPacket, data: &[u8]) -> ControlReply {
        match (setup.kind(), setup.recipient()) {
            (RequestKind::Standard, _) => self.standard(cx, setup, data),
            (_, Recipient::Interface) => self.route_interface(cx, setup, data),
            (_, Recipient::Endpoint) => self.route_endpoint(cx, setup, data),
            _ => match self.device_slot {
                Some(s) => self.slots[s].func.control(cx, setup, data),
                None => ControlReply::stall(),
            },
        }
    }

    fn endpoint(&self, address: u8) -> Option<EndpointInfo> {
        self.eps.get(&address).map(|(_, info)| *info)
    }

    fn data_in(&mut self, cx: &mut Ctx, ep: u8, max_len: usize) -> InReply {
        if self.halted.contains(&ep) {
            return InReply::Error(UsbError::Stall);
        }
        let s = match self.slot_for_endpoint(ep) {
            Ok(s) => s,
            Err(e) => return InReply::Error(e),
        };
        let r = self.slots[s].func.data_in(cx, ep, max_len);
        if r == InReply::Error(UsbError::Stall) {
            self.halted.insert(ep);
        }
        r
    }

    fn data_out(&mut self, cx: &mut Ctx, ep: u8, data: &[u8]) -> OutReply {
        if self.halted.contains(&ep) {
            return OutReply::Error(UsbError::Stall);
        }
        let s = match self.slot_for_endpoint(ep) {
            Ok(s) => s,
            Err(e) => return OutReply::Error(e),
        };
        let r = self.slots[s].func.data_out(cx, ep, data);
        if r == OutReply::Error(UsbError::Stall) {
            self.halted.insert(ep);
        }
        r
    }

    fn iso_in(&mut self, cx: &mut Ctx, ep: u8, max_len: usize) -> IsoReply {
        match self.slot_for_endpoint(ep) {
            Ok(s) => self.slots[s].func.iso_in(cx, ep, max_len),
            Err(e) => IsoReply {
                data: Vec::new(),
                error: Some(e),
            },
        }
    }

    fn iso_out(&mut self, cx: &mut Ctx, ep: u8, data: &[u8]) -> IsoReply {
        debug_assert_eq!(Direction::of_endpoint(ep), Direction::Out);
        match self.slot_for_endpoint(ep) {
            Ok(s) => self.slots[s].func.iso_out(cx, ep, data),
            Err(e) => IsoReply {
                data: Vec::new(),
                error: Some(e),
            },
        }
    }

    fn next_wakeup(&self) -> Option<BusTime> {
        self.slots.iter().filter_map(|s| s.func.next_wakeup()).min()
    }

    fn tick(&mut self, cx: &mut Ctx) {
        for s in &mut self.slots {
            s.func.tick(cx);
        }
    }
}
