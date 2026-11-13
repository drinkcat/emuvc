//! Transfer completion errors, independent of any transport.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Why a transfer (or one isochronous packet) did not complete normally.
///
/// Modelled on Linux URB statuses because both usbfs (behind libusb) and
/// USB/IP speak them; transports map these to their own codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsbError {
    /// Endpoint halted / request not supported (EPIPE).
    Stall,
    /// The device never completed the transfer; the host gave up (ETIMEDOUT).
    Timeout,
    /// Device sent more data than the host asked for: babble (EOVERFLOW).
    Overflow,
    /// Transaction error: CRC, bit stuffing, missing handshake (EPROTO).
    Protocol,
    /// Short transfer while the host required a full one (EREMOTEIO).
    ShortPacket,
    /// Device disconnected (ENODEV).
    NoDevice,
    /// Endpoint disabled under the transfer, e.g. by SET_INTERFACE (ESHUTDOWN).
    Shutdown,
    /// Cancelled by the host (ENOENT).
    Cancelled,
    /// Host-side misuse: unknown endpoint, bad parameters (EINVAL).
    Invalid,
}

impl UsbError {
    /// Negative Linux errno, as found in URB status fields (usbfs, USB/IP).
    pub fn errno(self) -> i32 {
        match self {
            UsbError::Stall => -32,        // EPIPE
            UsbError::Timeout => -110,     // ETIMEDOUT
            UsbError::Overflow => -75,     // EOVERFLOW
            UsbError::Protocol => -71,     // EPROTO
            UsbError::ShortPacket => -121, // EREMOTEIO
            UsbError::NoDevice => -19,     // ENODEV
            UsbError::Shutdown => -108,    // ESHUTDOWN
            UsbError::Cancelled => -2,     // ENOENT
            UsbError::Invalid => -22,      // EINVAL
        }
    }
}

impl fmt::Display for UsbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl std::error::Error for UsbError {}
