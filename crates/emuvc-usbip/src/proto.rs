//! Wire constants and (de)serialisation helpers. All fields are big-endian.

use std::io::{self, Read};

pub const VERSION: u16 = 0x0111;
pub const OP_REQ_IMPORT: u16 = 0x8003;
pub const OP_REP_IMPORT: u16 = 0x0003;
pub const OP_REQ_DEVLIST: u16 = 0x8005;
pub const OP_REP_DEVLIST: u16 = 0x0005;

pub const CMD_SUBMIT: u32 = 1;
pub const CMD_UNLINK: u32 = 2;
pub const RET_SUBMIT: u32 = 3;
pub const RET_UNLINK: u32 = 4;

/// Size of every URB-phase PDU header (basic header + command specific part).
pub const URB_HEADER_LEN: usize = 48;
/// `struct usbip_usb_device`.
pub const DEVICE_LEN: usize = 312;

pub const URB_SHORT_NOT_OK: u32 = 1;
pub const ECONNRESET: i32 = 104;
/// `number_of_packets` of non-isochronous PDUs.
pub const NO_PACKETS: u32 = 0xffff_ffff;

pub fn be32(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes(b[off..off + 4].try_into().unwrap())
}

pub fn be16(b: &[u8], off: usize) -> u16 {
    u16::from_be_bytes(b[off..off + 2].try_into().unwrap())
}

pub fn put32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}

pub fn put16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_be_bytes());
}

/// Appends `s` as a NUL padded field of `n` bytes.
pub fn put_str(v: &mut Vec<u8>, s: &str, n: usize) {
    let b = s.as_bytes();
    let k = b.len().min(n - 1);
    v.extend_from_slice(&b[..k]);
    v.resize(v.len() + n - k, 0);
}

pub fn get_str(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

pub fn read_vec<R: Read>(r: &mut R, n: usize) -> io::Result<Vec<u8>> {
    let mut v = vec![0; n];
    r.read_exact(&mut v)?;
    Ok(v)
}

/// `op_common` header: version, code, status.
pub fn op_header(code: u16, status: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(8);
    put16(&mut v, VERSION);
    put16(&mut v, code);
    put32(&mut v, status);
    v
}

pub fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}
