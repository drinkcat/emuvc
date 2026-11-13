//! Host-side helpers (a minimal UVC driver) for tests and tools: probe/commit
//! negotiation and payload reassembly over an `emuvc_usb::Host`.

use emuvc_usb::{DeviceId, Host, SetupPacket, UsbError};

use crate::consts::*;
use crate::stream::StreamingControl;
use crate::topology::VideoStreaming;

/// Negotiates and commits a format, like libuvc does (probe SET/GET, commit SET).
pub fn probe_commit(
    host: &Host,
    dev: DeviceId,
    interface: u8,
    format_index: u8,
    frame_index: u8,
    frame_interval: u32,
    len: u16,
) -> Result<StreamingControl, UsbError> {
    let req = StreamingControl {
        hint: HINT_FRAME_INTERVAL,
        format_index,
        frame_index,
        frame_interval,
        ..Default::default()
    };
    let setup =
        |rt: u8, r: u8, cs: u8| SetupPacket::new(rt, r, (cs as u16) << 8, interface as u16, len);
    host.control(
        dev,
        setup(0x21, SET_CUR, VS_PROBE_CONTROL),
        &req.to_bytes(len as usize),
        None,
    )?;
    let cur = StreamingControl::from_bytes(&host.control(
        dev,
        setup(0xa1, GET_CUR, VS_PROBE_CONTROL),
        &[],
        None,
    )?);
    host.control(
        dev,
        setup(0x21, SET_CUR, VS_COMMIT_CONTROL),
        &cur.to_bytes(len as usize),
        None,
    )?;
    Ok(cur)
}

/// Reassembles frames from UVC payloads (FID/EOF framing, like libuvc).
#[derive(Default, Debug)]
pub struct Reassembler {
    fid: Option<bool>,
    buf: Vec<u8>,
    buf_error: bool,
    /// Complete frames, with whether any payload had the ERR bit.
    pub frames: Vec<(Vec<u8>, bool)>,
    pub bad_headers: usize,
    pub payloads: usize,
}

impl Reassembler {
    pub fn push(&mut self, payload: &[u8]) {
        if payload.is_empty() {
            return;
        }
        let hlen = payload[0] as usize;
        if hlen < 2 || hlen > payload.len() {
            self.bad_headers += 1;
            return;
        }
        self.payloads += 1;
        let info = payload[1];
        let fid = info & HDR_FID != 0;
        if self.fid.is_some_and(|f| f != fid) && !self.buf.is_empty() {
            self.flush();
        }
        self.fid = Some(fid);
        self.buf_error |= info & HDR_ERR != 0;
        self.buf.extend_from_slice(&payload[hlen..]);
        if info & HDR_EOF != 0 && !self.buf.is_empty() {
            self.flush();
        }
    }

    fn flush(&mut self) {
        let f = std::mem::take(&mut self.buf);
        self.frames.push((f, std::mem::take(&mut self.buf_error)));
    }
}

/// Result of [`capture`].
pub struct Capture {
    pub commit: StreamingControl,
    /// Alternate setting used (isochronous) or 0 (bulk).
    pub alt: u8,
    pub frames: Vec<(Vec<u8>, bool)>,
    /// Isochronous packets that completed with an error.
    pub packet_errors: usize,
}

fn set_interface(host: &Host, dev: DeviceId, iface: u8, alt: u8) -> Result<(), String> {
    host.control(
        dev,
        SetupPacket::new(0x01, 0x0b, alt as u16, iface as u16, 0),
        &[],
        None,
    )
    .map(drop)
    .map_err(|e| format!("SET_INTERFACE {iface}/{alt}: {e}"))
}

/// Streams `n` frames of a format/frame from a VideoStreaming interface,
/// choosing the alternate setting like libuvc (smallest one carrying
/// dwMaxPayloadTransferSize), with 8 URBs of 32 packets in flight.
pub fn capture(
    host: &Host,
    dev: DeviceId,
    vs: &VideoStreaming,
    format: u8,
    frame: u8,
    n: usize,
) -> Result<Capture, String> {
    use std::sync::mpsc;
    use std::time::Duration;

    use emuvc_usb::{Completion, Urb};

    use crate::topology::Transfer;

    let fr = vs
        .format(format)
        .and_then(|f| f.frame(frame))
        .ok_or("no such format/frame")?;
    let len = StreamingControl::len_for(0x0150) as u16;
    let commit = probe_commit(
        host,
        dev,
        vs.interface,
        format,
        frame,
        fr.default_interval,
        len,
    )
    .map_err(|e| format!("probe/commit: {e}"))?;
    let (tx, rx) = mpsc::channel::<Completion>();
    let mut r = Reassembler::default();
    let mut packet_errors = 0;
    let timeout = Duration::from_secs(10);
    let cb = |tx: &mpsc::Sender<Completion>| {
        let tx = tx.clone();
        Box::new(move |c| drop(tx.send(c))) as emuvc_usb::host::CompletionFn
    };
    let alt = match &vs.transfer {
        Transfer::Isochronous { alts } => {
            let need = commit.max_payload_transfer_size as usize;
            let alt = alts
                .iter()
                .find(|a| a.bytes_per_interval >= need)
                .or(alts.last())
                .ok_or("no isochronous alt setting")?;
            set_interface(host, dev, vs.interface, alt.alt)?;
            let urb = || Urb::iso_in(dev, vs.endpoint, vec![alt.bytes_per_interval; 32]);
            for _ in 0..8 {
                host.submit(urb(), cb(&tx)).map_err(|e| e.to_string())?;
            }
            while r.frames.len() < n {
                let c = rx
                    .recv_timeout(timeout)
                    .map_err(|_| format!("timeout after {} frames", r.frames.len()))?;
                for p in &c.iso {
                    match p.status {
                        Ok(()) => r.push(&p.data),
                        Err(_) => packet_errors += 1,
                    }
                }
                host.submit(urb(), cb(&tx)).map_err(|e| e.to_string())?;
            }
            // Outstanding URBs are flushed by the alt setting change.
            set_interface(host, dev, vs.interface, 0)?;
            alt.alt
        }
        Transfer::Bulk { .. } => {
            let size = (commit.max_payload_transfer_size as usize).max(512);
            let mut ids = Vec::new();
            for _ in 0..4 {
                ids.push(
                    host.submit(Urb::bulk_in(dev, vs.endpoint, size), cb(&tx))
                        .map_err(|e| e.to_string())?,
                );
            }
            while r.frames.len() < n {
                let c = rx
                    .recv_timeout(timeout)
                    .map_err(|_| format!("timeout after {} frames", r.frames.len()))?;
                if c.status.is_ok() {
                    r.push(&c.data);
                }
                ids.push(
                    host.submit(Urb::bulk_in(dev, vs.endpoint, size), cb(&tx))
                        .map_err(|e| e.to_string())?,
                );
            }
            for id in ids {
                host.cancel(id);
            }
            let _ = host.control(
                dev,
                SetupPacket::new(0x02, 0x01, 0, vs.endpoint as u16, 0),
                &[],
                None,
            );
            0
        }
    };
    Ok(Capture {
        commit,
        alt,
        frames: r.frames,
        packet_errors,
    })
}
