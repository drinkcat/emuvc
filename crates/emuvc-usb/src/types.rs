//! Basic USB protocol types shared by device models, the host engine and transports.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Bus speed of a device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speed {
    Low,
    Full,
    High,
    Super,
    SuperPlus,
}

impl Speed {
    /// True if periodic transfers are scheduled in 125 µs microframes (high speed and up).
    pub fn has_microframes(self) -> bool {
        !matches!(self, Speed::Low | Speed::Full)
    }
}

/// Transfer direction, from the host's point of view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    Out,
    In,
}

impl Direction {
    /// Direction of an endpoint address (bit 7 set = IN).
    pub fn of_endpoint(ep: u8) -> Self {
        if ep & 0x80 != 0 {
            Direction::In
        } else {
            Direction::Out
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TransferType {
    Control,
    Isochronous,
    Bulk,
    Interrupt,
}

impl TransferType {
    /// Decode bits 1..0 of an endpoint descriptor's `bmAttributes`.
    pub fn from_attributes(bm_attributes: u8) -> Self {
        match bm_attributes & 3 {
            0 => TransferType::Control,
            1 => TransferType::Isochronous,
            2 => TransferType::Bulk,
            _ => TransferType::Interrupt,
        }
    }
}

/// `bmRequestType` bits 6..5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestKind {
    Standard,
    Class,
    Vendor,
    Reserved,
}

/// `bmRequestType` bits 4..0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recipient {
    Device,
    Interface,
    Endpoint,
    Other,
    Reserved(u8),
}

/// The 8-byte SETUP packet starting every control transfer.
#[derive(Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct SetupPacket {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

impl SetupPacket {
    pub const fn new(request_type: u8, request: u8, value: u16, index: u16, length: u16) -> Self {
        SetupPacket {
            request_type,
            request,
            value,
            index,
            length,
        }
    }

    pub fn from_bytes(b: [u8; 8]) -> Self {
        SetupPacket {
            request_type: b[0],
            request: b[1],
            value: u16::from_le_bytes([b[2], b[3]]),
            index: u16::from_le_bytes([b[4], b[5]]),
            length: u16::from_le_bytes([b[6], b[7]]),
        }
    }

    pub fn to_bytes(&self) -> [u8; 8] {
        let v = self.value.to_le_bytes();
        let i = self.index.to_le_bytes();
        let l = self.length.to_le_bytes();
        [
            self.request_type,
            self.request,
            v[0],
            v[1],
            i[0],
            i[1],
            l[0],
            l[1],
        ]
    }

    pub fn direction(&self) -> Direction {
        if self.request_type & 0x80 != 0 {
            Direction::In
        } else {
            Direction::Out
        }
    }

    pub fn kind(&self) -> RequestKind {
        match (self.request_type >> 5) & 3 {
            0 => RequestKind::Standard,
            1 => RequestKind::Class,
            2 => RequestKind::Vendor,
            _ => RequestKind::Reserved,
        }
    }

    pub fn recipient(&self) -> Recipient {
        match self.request_type & 0x1f {
            0 => Recipient::Device,
            1 => Recipient::Interface,
            2 => Recipient::Endpoint,
            3 => Recipient::Other,
            r => Recipient::Reserved(r),
        }
    }
}

impl fmt::Debug for SetupPacket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SETUP[{:02x} {:02x} wValue={:04x} wIndex={:04x} wLength={}]",
            self.request_type, self.request, self.value, self.index, self.length
        )
    }
}

/// Standard request codes (USB 2.0 table 9-4, USB 3.x additions).
pub mod std_request {
    pub const GET_STATUS: u8 = 0x00;
    pub const CLEAR_FEATURE: u8 = 0x01;
    pub const SET_FEATURE: u8 = 0x03;
    pub const SET_ADDRESS: u8 = 0x05;
    pub const GET_DESCRIPTOR: u8 = 0x06;
    pub const SET_DESCRIPTOR: u8 = 0x07;
    pub const GET_CONFIGURATION: u8 = 0x08;
    pub const SET_CONFIGURATION: u8 = 0x09;
    pub const GET_INTERFACE: u8 = 0x0a;
    pub const SET_INTERFACE: u8 = 0x0b;
    pub const SYNCH_FRAME: u8 = 0x0c;
    pub const SET_SEL: u8 = 0x30;
    pub const SET_ISOCH_DELAY: u8 = 0x31;
}

/// Standard feature selectors.
pub mod feature {
    pub const ENDPOINT_HALT: u16 = 0;
    pub const DEVICE_REMOTE_WAKEUP: u16 = 1;
    pub const TEST_MODE: u16 = 2;
}
