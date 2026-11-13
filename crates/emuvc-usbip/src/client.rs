//! Minimal USB/IP client (operation phase only).

use std::io::{self, Write};
use std::net::{TcpStream, ToSocketAddrs};

use crate::proto::*;

/// A device as described by `OP_REP_DEVLIST` / `OP_REP_IMPORT`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteDevice {
    pub path: String,
    pub busid: String,
    pub busnum: u32,
    pub devnum: u32,
    /// `USB_SPEED_*` (1 low, 2 full, 3 high, 5 super, 6 super plus).
    pub speed: u32,
    pub vendor_id: u16,
    pub product_id: u16,
    pub bcd_device: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub configuration_value: u8,
    pub num_configurations: u8,
    /// (class, subclass, protocol) per interface; empty after an import.
    pub interfaces: Vec<(u8, u8, u8)>,
}

impl RemoteDevice {
    pub fn devid(&self) -> u32 {
        self.busnum << 16 | self.devnum
    }

    /// `struct usbip_usb_device` (312 bytes), without interfaces.
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(DEVICE_LEN);
        put_str(&mut v, &self.path, 256);
        put_str(&mut v, &self.busid, 32);
        put32(&mut v, self.busnum);
        put32(&mut v, self.devnum);
        put32(&mut v, self.speed);
        put16(&mut v, self.vendor_id);
        put16(&mut v, self.product_id);
        put16(&mut v, self.bcd_device);
        v.extend_from_slice(&[
            self.class,
            self.subclass,
            self.protocol,
            self.configuration_value,
            self.num_configurations,
            self.interfaces.len() as u8,
        ]);
        debug_assert_eq!(v.len(), DEVICE_LEN);
        v
    }

    /// Decodes the 312 byte structure; returns the device and bNumInterfaces.
    pub fn decode(b: &[u8]) -> (RemoteDevice, usize) {
        let d = RemoteDevice {
            path: get_str(&b[..256]),
            busid: get_str(&b[256..288]),
            busnum: be32(b, 288),
            devnum: be32(b, 292),
            speed: be32(b, 296),
            vendor_id: be16(b, 300),
            product_id: be16(b, 302),
            bcd_device: be16(b, 304),
            class: b[306],
            subclass: b[307],
            protocol: b[308],
            configuration_value: b[309],
            num_configurations: b[310],
            interfaces: Vec::new(),
        };
        (d, b[311] as usize)
    }
}

fn connect(addr: impl ToSocketAddrs) -> io::Result<TcpStream> {
    let s = TcpStream::connect(addr)?;
    s.set_nodelay(true)?;
    Ok(s)
}

fn read_op_header(s: &mut TcpStream, code: u16) -> io::Result<u32> {
    let h = read_vec(s, 8)?;
    if be16(&h, 2) != code {
        return Err(bad(format!("unexpected reply code {:#06x}", be16(&h, 2))));
    }
    Ok(be32(&h, 4))
}

/// `OP_REQ_DEVLIST`.
pub fn list(addr: impl ToSocketAddrs) -> io::Result<Vec<RemoteDevice>> {
    let mut s = connect(addr)?;
    s.write_all(&op_header(OP_REQ_DEVLIST, 0))?;
    let status = read_op_header(&mut s, OP_REP_DEVLIST)?;
    if status != 0 {
        return Err(io::Error::other(format!("devlist failed: status {status}")));
    }
    let n = be32(&read_vec(&mut s, 4)?, 0);
    let mut out = Vec::new();
    for _ in 0..n {
        let (mut d, ni) = RemoteDevice::decode(&read_vec(&mut s, DEVICE_LEN)?);
        for _ in 0..ni {
            let i = read_vec(&mut s, 4)?;
            d.interfaces.push((i[0], i[1], i[2]));
        }
        out.push(d);
    }
    Ok(out)
}

/// `OP_REQ_IMPORT`: on success the returned socket is in the URB phase.
pub fn import(addr: impl ToSocketAddrs, busid: &str) -> io::Result<(TcpStream, RemoteDevice)> {
    let mut s = connect(addr)?;
    let mut req = op_header(OP_REQ_IMPORT, 0);
    put_str(&mut req, busid, 32);
    s.write_all(&req)?;
    let status = read_op_header(&mut s, OP_REP_IMPORT)?;
    if status != 0 {
        return Err(io::Error::other(format!(
            "import of {busid} refused: status {status}"
        )));
    }
    let (d, _) = RemoteDevice::decode(&read_vec(&mut s, DEVICE_LEN)?);
    Ok((s, d))
}
