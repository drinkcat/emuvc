//! Standard USB descriptors: typed structs, serialisation, a configuration
//! builder and a lenient parsed view of configuration descriptors.
//!
//! Devices keep their descriptors as raw bytes (so device models and fault
//! rules can produce exactly what a real, possibly buggy, device sends); the
//! typed structs here are conveniences to build and inspect them.

use crate::types::{Direction, Speed, TransferType};

/// Descriptor type codes (`bDescriptorType`).
pub mod dtype {
    pub const DEVICE: u8 = 0x01;
    pub const CONFIG: u8 = 0x02;
    pub const STRING: u8 = 0x03;
    pub const INTERFACE: u8 = 0x04;
    pub const ENDPOINT: u8 = 0x05;
    pub const DEVICE_QUALIFIER: u8 = 0x06;
    pub const OTHER_SPEED_CONFIG: u8 = 0x07;
    pub const INTERFACE_POWER: u8 = 0x08;
    pub const OTG: u8 = 0x09;
    pub const DEBUG: u8 = 0x0a;
    pub const INTERFACE_ASSOCIATION: u8 = 0x0b;
    pub const BOS: u8 = 0x0f;
    pub const DEVICE_CAPABILITY: u8 = 0x10;
    pub const CS_DEVICE: u8 = 0x21;
    pub const CS_CONFIG: u8 = 0x22;
    pub const CS_STRING: u8 = 0x23;
    pub const CS_INTERFACE: u8 = 0x24;
    pub const CS_ENDPOINT: u8 = 0x25;
    pub const SS_ENDPOINT_COMPANION: u8 = 0x30;
    pub const SSP_ISOC_ENDPOINT_COMPANION: u8 = 0x31;
}

/// Error while parsing descriptors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DescError {
    /// Malformed descriptor (bad `bLength`, truncated, wrong type) at byte offset.
    Malformed { offset: usize, what: &'static str },
}

impl std::fmt::Display for DescError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DescError::Malformed { offset, what } => {
                write!(f, "malformed descriptor at offset {offset}: {what}")
            }
        }
    }
}

impl std::error::Error for DescError {}

/// Splits concatenated descriptors into `(offset, bytes)` slices.
///
/// Yields `Err` once and stops at a descriptor whose `bLength` is < 2 or
/// overruns the buffer.
pub fn split(bytes: &[u8]) -> impl Iterator<Item = Result<(usize, &[u8]), DescError>> {
    let mut off = 0;
    let mut done = false;
    std::iter::from_fn(move || {
        if done || off >= bytes.len() {
            return None;
        }
        let len = bytes[off] as usize;
        if len < 2 || off + len > bytes.len() {
            done = true;
            return Some(Err(DescError::Malformed {
                offset: off,
                what: "bad bLength",
            }));
        }
        let item = (off, &bytes[off..off + len]);
        off += len;
        Some(Ok(item))
    })
}

fn le16(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

/// Device descriptor (USB 2.0 §9.6.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceDescriptor {
    pub bcd_usb: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub max_packet_size0: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub bcd_device: u16,
    pub i_manufacturer: u8,
    pub i_product: u8,
    pub i_serial: u8,
    pub num_configurations: u8,
}

impl DeviceDescriptor {
    pub const LEN: usize = 18;

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = vec![18, dtype::DEVICE];
        v.extend_from_slice(&self.bcd_usb.to_le_bytes());
        v.extend_from_slice(&[
            self.class,
            self.subclass,
            self.protocol,
            self.max_packet_size0,
        ]);
        v.extend_from_slice(&self.vendor_id.to_le_bytes());
        v.extend_from_slice(&self.product_id.to_le_bytes());
        v.extend_from_slice(&self.bcd_device.to_le_bytes());
        v.extend_from_slice(&[
            self.i_manufacturer,
            self.i_product,
            self.i_serial,
            self.num_configurations,
        ]);
        v
    }

    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < 18 || b[1] != dtype::DEVICE {
            return None;
        }
        Some(DeviceDescriptor {
            bcd_usb: le16(b, 2),
            class: b[4],
            subclass: b[5],
            protocol: b[6],
            max_packet_size0: b[7],
            vendor_id: le16(b, 8),
            product_id: le16(b, 10),
            bcd_device: le16(b, 12),
            i_manufacturer: b[14],
            i_product: b[15],
            i_serial: b[16],
            num_configurations: b[17],
        })
    }
}

/// The 9-byte header of a configuration descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigHeader {
    pub total_length: u16,
    pub num_interfaces: u8,
    pub value: u8,
    pub i_configuration: u8,
    pub attributes: u8,
    pub max_power: u8,
}

impl ConfigHeader {
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < 9 || b[1] != dtype::CONFIG {
            return None;
        }
        Some(ConfigHeader {
            total_length: le16(b, 2),
            num_interfaces: b[4],
            value: b[5],
            i_configuration: b[6],
            attributes: b[7],
            max_power: b[8],
        })
    }
}

/// Interface descriptor (USB 2.0 §9.6.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterfaceDescriptor {
    pub number: u8,
    pub alt: u8,
    pub num_endpoints: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub i_interface: u8,
}

impl InterfaceDescriptor {
    pub fn to_bytes(&self) -> Vec<u8> {
        vec![
            9,
            dtype::INTERFACE,
            self.number,
            self.alt,
            self.num_endpoints,
            self.class,
            self.subclass,
            self.protocol,
            self.i_interface,
        ]
    }

    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < 9 || b[1] != dtype::INTERFACE {
            return None;
        }
        Some(InterfaceDescriptor {
            number: b[2],
            alt: b[3],
            num_endpoints: b[4],
            class: b[5],
            subclass: b[6],
            protocol: b[7],
            i_interface: b[8],
        })
    }
}

/// Endpoint descriptor (USB 2.0 §9.6.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointDescriptor {
    pub address: u8,
    pub attributes: u8,
    /// Raw `wMaxPacketSize`, including the high-bandwidth multiplier bits 12..11.
    pub max_packet_size: u16,
    pub interval: u8,
}

impl EndpointDescriptor {
    pub fn to_bytes(&self) -> Vec<u8> {
        let m = self.max_packet_size.to_le_bytes();
        vec![
            7,
            dtype::ENDPOINT,
            self.address,
            self.attributes,
            m[0],
            m[1],
            self.interval,
        ]
    }

    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < 7 || b[1] != dtype::ENDPOINT {
            return None;
        }
        Some(EndpointDescriptor {
            address: b[2],
            attributes: b[3],
            max_packet_size: le16(b, 4),
            interval: b[6],
        })
    }

    pub fn transfer_type(&self) -> TransferType {
        TransferType::from_attributes(self.attributes)
    }

    pub fn direction(&self) -> Direction {
        Direction::of_endpoint(self.address)
    }

    /// Bytes per service interval at high speed: packet size times the
    /// high-bandwidth multiplier (1..3).
    pub fn hs_bytes_per_interval(&self) -> usize {
        let size = (self.max_packet_size & 0x7ff) as usize;
        let mult = ((self.max_packet_size >> 11) & 3) as usize + 1;
        size * mult
    }

    /// Service interval of a periodic endpoint, in microframes.
    pub fn interval_uframes(&self, speed: Speed) -> u32 {
        let exp = |b: u8| 1u32 << (b.clamp(1, 16) - 1);
        match (self.transfer_type(), speed.has_microframes()) {
            (TransferType::Isochronous, true) | (TransferType::Interrupt, true) => {
                exp(self.interval)
            }
            (TransferType::Isochronous, false) => exp(self.interval) * 8,
            (TransferType::Interrupt, false) => (self.interval.max(1) as u32) * 8,
            _ => 1,
        }
    }
}

/// Interface association descriptor (IAD).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IadDescriptor {
    pub first_interface: u8,
    pub interface_count: u8,
    pub function_class: u8,
    pub function_subclass: u8,
    pub function_protocol: u8,
    pub i_function: u8,
}

impl IadDescriptor {
    pub fn to_bytes(&self) -> Vec<u8> {
        vec![
            8,
            dtype::INTERFACE_ASSOCIATION,
            self.first_interface,
            self.interface_count,
            self.function_class,
            self.function_subclass,
            self.function_protocol,
            self.i_function,
        ]
    }
}

/// SuperSpeed endpoint companion descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SsEndpointCompanion {
    pub max_burst: u8,
    pub attributes: u8,
    pub bytes_per_interval: u16,
}

impl SsEndpointCompanion {
    pub fn to_bytes(&self) -> Vec<u8> {
        let b = self.bytes_per_interval.to_le_bytes();
        vec![
            6,
            dtype::SS_ENDPOINT_COMPANION,
            self.max_burst,
            self.attributes,
            b[0],
            b[1],
        ]
    }

    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < 6 || b[1] != dtype::SS_ENDPOINT_COMPANION {
            return None;
        }
        Some(SsEndpointCompanion {
            max_burst: b[2],
            attributes: b[3],
            bytes_per_interval: le16(b, 4),
        })
    }
}

/// Builds a configuration descriptor from descriptors appended in order.
///
/// `wTotalLength` and `bNumInterfaces` are computed by [`ConfigBuilder::build`]
/// unless overridden (handy to model broken devices).
#[derive(Clone, Debug)]
pub struct ConfigBuilder {
    value: u8,
    i_configuration: u8,
    attributes: u8,
    max_power: u8,
    body: Vec<u8>,
    interfaces: Vec<u8>,
    num_interfaces: Option<u8>,
    total_length: Option<u16>,
}

impl ConfigBuilder {
    /// New bus-powered configuration (`bmAttributes` 0x80, 100 mA).
    pub fn new(value: u8) -> Self {
        ConfigBuilder {
            value,
            i_configuration: 0,
            attributes: 0x80,
            max_power: 50,
            body: Vec::new(),
            interfaces: Vec::new(),
            num_interfaces: None,
            total_length: None,
        }
    }

    pub fn attributes(mut self, bm_attributes: u8) -> Self {
        self.attributes = bm_attributes;
        self
    }

    /// `bMaxPower`, in units of 2 mA (USB 2) or 8 mA (USB 3).
    pub fn max_power(mut self, units: u8) -> Self {
        self.max_power = units;
        self
    }

    pub fn i_configuration(mut self, index: u8) -> Self {
        self.i_configuration = index;
        self
    }

    pub fn override_num_interfaces(&mut self, n: u8) -> &mut Self {
        self.num_interfaces = Some(n);
        self
    }

    pub fn override_total_length(&mut self, n: u16) -> &mut Self {
        self.total_length = Some(n);
        self
    }

    /// Appends descriptor bytes verbatim (class-specific descriptors etc.).
    pub fn raw(&mut self, desc: &[u8]) -> &mut Self {
        self.body.extend_from_slice(desc);
        self
    }

    pub fn iad(&mut self, d: &IadDescriptor) -> &mut Self {
        self.raw(&d.to_bytes())
    }

    pub fn interface(&mut self, d: &InterfaceDescriptor) -> &mut Self {
        if !self.interfaces.contains(&d.number) {
            self.interfaces.push(d.number);
        }
        self.raw(&d.to_bytes())
    }

    pub fn endpoint(&mut self, d: &EndpointDescriptor) -> &mut Self {
        self.raw(&d.to_bytes())
    }

    pub fn ss_companion(&mut self, d: &SsEndpointCompanion) -> &mut Self {
        self.raw(&d.to_bytes())
    }

    pub fn build(&self) -> Vec<u8> {
        let total = self.total_length.unwrap_or((9 + self.body.len()) as u16);
        let num_if = self.num_interfaces.unwrap_or(self.interfaces.len() as u8);
        let t = total.to_le_bytes();
        let mut v = vec![
            9,
            dtype::CONFIG,
            t[0],
            t[1],
            num_if,
            self.value,
            self.i_configuration,
            self.attributes,
            self.max_power,
        ];
        v.extend_from_slice(&self.body);
        v
    }
}

/// Parsed view of a configuration descriptor, grouped like libusb does:
/// interfaces → alternate settings → endpoints, with class-specific
/// descriptors kept as raw `extra` bytes next to what they follow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigView {
    pub header: ConfigHeader,
    /// Descriptors between the configuration header and the first interface (e.g. IADs).
    pub extra: Vec<u8>,
    /// Sorted by interface number.
    pub interfaces: Vec<InterfaceView>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterfaceView {
    pub number: u8,
    /// In descriptor order.
    pub alts: Vec<AltView>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AltView {
    pub desc: InterfaceDescriptor,
    /// Descriptors after the interface descriptor, before its first endpoint.
    pub extra: Vec<u8>,
    pub endpoints: Vec<EndpointView>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointView {
    pub desc: EndpointDescriptor,
    pub companion: Option<SsEndpointCompanion>,
    /// Descriptors following the endpoint (excluding the SS companion).
    pub extra: Vec<u8>,
}

impl EndpointView {
    /// Maximum bytes the endpoint moves per service interval.
    pub fn bytes_per_interval(&self, speed: Speed) -> usize {
        match (&self.companion, speed) {
            (Some(c), Speed::Super | Speed::SuperPlus) => match self.desc.transfer_type() {
                TransferType::Isochronous | TransferType::Interrupt => {
                    c.bytes_per_interval as usize
                }
                _ => (self.desc.max_packet_size & 0x7ff) as usize * (c.max_burst as usize + 1),
            },
            (_, s) if s.has_microframes() => self.desc.hs_bytes_per_interval(),
            _ => (self.desc.max_packet_size & 0x7ff) as usize,
        }
    }
}

impl ConfigView {
    /// Parses a configuration descriptor. Lenient: trailing garbage after a
    /// malformed descriptor is ignored, like most hosts do.
    pub fn parse(bytes: &[u8]) -> Result<Self, DescError> {
        let header = ConfigHeader::parse(bytes).ok_or(DescError::Malformed {
            offset: 0,
            what: "config header",
        })?;
        let end = (header.total_length as usize).min(bytes.len());
        let mut view = ConfigView {
            header,
            extra: Vec::new(),
            interfaces: Vec::new(),
        };
        // Index (in `view.interfaces`) of the interface owning the current alt
        // setting, and whether an endpoint follows it: unknown descriptors are
        // appended to whatever they follow.
        let mut cur: Option<usize> = None;
        let mut in_endpoint = false;
        for item in split(&bytes[9.min(end)..end]) {
            let Ok((_, d)) = item else { break };
            match (d[1], cur) {
                (dtype::INTERFACE, _) => {
                    let Some(desc) = InterfaceDescriptor::parse(d) else {
                        continue;
                    };
                    let pos = match view.interfaces.iter().position(|i| i.number == desc.number) {
                        Some(p) => p,
                        None => {
                            view.interfaces.push(InterfaceView {
                                number: desc.number,
                                alts: Vec::new(),
                            });
                            view.interfaces.len() - 1
                        }
                    };
                    view.interfaces[pos].alts.push(AltView {
                        desc,
                        extra: Vec::new(),
                        endpoints: Vec::new(),
                    });
                    cur = Some(pos);
                    in_endpoint = false;
                }
                (dtype::ENDPOINT, Some(i)) => {
                    let Some(desc) = EndpointDescriptor::parse(d) else {
                        continue;
                    };
                    let alt = view.interfaces[i].alts.last_mut().unwrap();
                    alt.endpoints.push(EndpointView {
                        desc,
                        companion: None,
                        extra: Vec::new(),
                    });
                    in_endpoint = true;
                }
                (dtype::SS_ENDPOINT_COMPANION, Some(i)) if in_endpoint => {
                    let ep = view.interfaces[i]
                        .alts
                        .last_mut()
                        .unwrap()
                        .endpoints
                        .last_mut()
                        .unwrap();
                    ep.companion = SsEndpointCompanion::parse(d);
                }
                (_, None) => view.extra.extend_from_slice(d),
                (_, Some(i)) => {
                    let alt = view.interfaces[i].alts.last_mut().unwrap();
                    match alt.endpoints.last_mut() {
                        Some(ep) if in_endpoint => ep.extra.extend_from_slice(d),
                        _ => alt.extra.extend_from_slice(d),
                    }
                }
            }
        }
        view.interfaces.sort_by_key(|i| i.number);
        Ok(view)
    }

    pub fn interface(&self, number: u8) -> Option<&InterfaceView> {
        self.interfaces.iter().find(|i| i.number == number)
    }

    pub fn alt(&self, iface: u8, alt: u8) -> Option<&AltView> {
        self.interface(iface)?
            .alts
            .iter()
            .find(|a| a.desc.alt == alt)
    }
}

/// Builds a string descriptor (UTF-16LE), truncated to the 255-byte maximum.
pub fn string_descriptor(s: &str) -> Vec<u8> {
    let mut v = vec![0, dtype::STRING];
    for u in s.encode_utf16() {
        if v.len() + 2 > 255 {
            break;
        }
        v.extend_from_slice(&u.to_le_bytes());
    }
    v[0] = v.len() as u8;
    v
}

/// String descriptor zero: the list of supported LANGIDs.
pub fn langid_descriptor(langids: &[u16]) -> Vec<u8> {
    let mut v = vec![0, dtype::STRING];
    for l in langids.iter().take(126) {
        v.extend_from_slice(&l.to_le_bytes());
    }
    v[0] = v.len() as u8;
    v
}

/// Decodes a string descriptor; lossy for invalid UTF-16.
pub fn parse_string_descriptor(b: &[u8]) -> Option<String> {
    if b.len() < 2 || b[1] != dtype::STRING {
        return None;
    }
    let len = (b[0] as usize).min(b.len());
    let units: Vec<u16> = b[2..len]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    Some(String::from_utf16_lossy(&units))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../../../tests/fixtures/30c9_0052.descriptors.bin");

    #[test]
    fn parse_real_webcam() {
        let dev = DeviceDescriptor::parse(FIXTURE).unwrap();
        assert_eq!((dev.vendor_id, dev.product_id), (0x30c9, 0x0052));
        assert_eq!(DeviceDescriptor::parse(&dev.to_bytes()), Some(dev));
        let cfg = ConfigView::parse(&FIXTURE[18..]).unwrap();
        assert_eq!(cfg.header.num_interfaces, 5);
        assert_eq!(cfg.interfaces.len(), 5);
        assert_eq!(cfg.interface(1).unwrap().alts.len(), 12);
        let ep = &cfg.alt(1, 11).unwrap().endpoints[0];
        assert_eq!(ep.desc.address, 0x81);
        assert_eq!(ep.bytes_per_interval(Speed::High), 3 * 1020);
        assert_eq!(ep.desc.interval_uframes(Speed::High), 1);
        let vc = cfg.alt(0, 0).unwrap();
        assert_eq!(&vc.extra[..3], &[0x0d, 0x24, 0x01]);
        assert_eq!(vc.endpoints[0].extra, vec![5, 0x25, 3, 0x05, 0x04]);
        assert_eq!(vc.endpoints[0].desc.interval_uframes(Speed::High), 128);
        assert_eq!(&cfg.extra[..2], &[8, dtype::INTERFACE_ASSOCIATION]);
    }

    #[test]
    fn builder_roundtrip() {
        let mut b = ConfigBuilder::new(1);
        b.interface(&InterfaceDescriptor {
            number: 0,
            alt: 0,
            num_endpoints: 1,
            class: 0xff,
            subclass: 0,
            protocol: 0,
            i_interface: 0,
        });
        b.endpoint(&EndpointDescriptor {
            address: 0x81,
            attributes: 2,
            max_packet_size: 512,
            interval: 0,
        });
        b.interface(&InterfaceDescriptor {
            number: 0,
            alt: 1,
            num_endpoints: 0,
            class: 0xff,
            subclass: 0,
            protocol: 0,
            i_interface: 0,
        });
        let bytes = b.build();
        assert_eq!(bytes.len(), 9 + 9 + 7 + 9);
        let v = ConfigView::parse(&bytes).unwrap();
        assert_eq!(v.header.num_interfaces, 1);
        assert_eq!(v.header.total_length as usize, bytes.len());
        assert_eq!(v.interface(0).unwrap().alts.len(), 2);
    }

    #[test]
    fn strings() {
        let d = string_descriptor("Caméra");
        assert_eq!(parse_string_descriptor(&d).as_deref(), Some("Caméra"));
        assert_eq!(langid_descriptor(&[0x0409]), vec![4, 3, 0x09, 0x04]);
    }

    #[test]
    fn split_stops_on_garbage() {
        let items: Vec<_> = split(&[2, 1, 0, 5]).collect();
        assert!(items[0].is_ok());
        assert!(items[1].is_err());
        assert_eq!(items.len(), 2);
    }
}
