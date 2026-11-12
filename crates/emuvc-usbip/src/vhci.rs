//! Attaching an imported device to the kernel's `vhci-hcd` (like `usbip attach`).

use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::os::fd::AsRawFd;

use crate::client::import;

const VHCI: &str = "/sys/devices/platform/vhci_hcd.0";

/// Picks a free port of the hub matching `speed` from the contents of the
/// vhci `status` file (`hub port sta spd dev sockfd local_busid`).
pub fn free_port(status: &str, speed: u32) -> Option<u32> {
    let want = if speed >= 5 { "ss" } else { "hs" };
    status.lines().skip(1).find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        // sta 004 = VDEV_ST_NULL (port free).
        (f.len() >= 3 && f[0] == want && f[2] == "004")
            .then(|| f[1].parse().ok())
            .flatten()
    })
}

/// Imports `busid` from `addr` and hands the socket to vhci-hcd. Returns the
/// vhci port. Needs root and the `vhci-hcd` module.
pub fn attach_vhci(addr: impl ToSocketAddrs, busid: &str) -> io::Result<u32> {
    let (stream, dev) = import(addr, busid)?;
    let status = std::fs::read_to_string(format!("{VHCI}/status"))?;
    let port = free_port(&status, dev.speed)
        .ok_or_else(|| io::Error::other(format!("no free vhci port for speed {}", dev.speed)))?;
    let line = format!(
        "{} {} {} {}",
        port,
        stream.as_raw_fd(),
        dev.devid(),
        dev.speed
    );
    std::fs::write(format!("{VHCI}/attach"), line)?;
    // The kernel holds its own reference to the socket now.
    drop::<TcpStream>(stream);
    Ok(port)
}

#[cfg(test)]
mod tests {
    #[test]
    fn ports() {
        let s = "hub port sta spd dev      sockfd local_busid\nhs  0000 004 000 00000000 000000 0-0\nhs  0001 004 000 00000000 000000 0-0\nss  0008 004 000 00000000 000000 0-0\n";
        assert_eq!(super::free_port(s, 3), Some(0));
        assert_eq!(super::free_port(s, 5), Some(8));
    }
}
