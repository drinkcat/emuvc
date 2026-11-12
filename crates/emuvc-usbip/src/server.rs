//! USB/IP server. See the crate documentation for the design.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use emuvc_usb::desc::ConfigView;
use emuvc_usb::host::CompletionFn;
use emuvc_usb::{
    Completion, DeviceId, DeviceInfo, Host, SetupPacket, Speed, TransferType, Urb, UrbId, UrbKind,
};
use log::{debug, info, warn};

use crate::client::RemoteDevice;
use crate::proto::*;

/// Largest data stage / transfer buffer accepted.
const MAX_TRANSFER: usize = 16 << 20;
const MAX_ISO_PACKETS: usize = 1024;

struct Server {
    host: Arc<Host>,
    busy: Mutex<HashSet<DeviceId>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Accept loop: one thread per connection. Only returns on a listener error.
pub fn serve(host: Arc<Host>, listener: TcpListener) -> io::Result<()> {
    let server = Arc::new(Server {
        host,
        busy: Mutex::new(HashSet::new()),
    });
    loop {
        let (stream, peer) = match listener.accept() {
            Ok(c) => c,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::ConnectionAborted
                ) =>
            {
                continue;
            }
            Err(e) => {
                warn!("usbip: accept failed: {e}");
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        let server = server.clone();
        let spawned = thread::Builder::new()
            .name(format!("usbip-{peer}"))
            .spawn(move || {
                debug!("usbip: connection from {peer}");
                match server.connection(stream) {
                    Ok(()) => debug!("usbip: {peer}: closed"),
                    Err(e) => info!("usbip: {peer}: closed: {e}"),
                }
            });
        if let Err(e) = spawned {
            warn!("usbip: cannot spawn connection thread: {e}");
        }
    }
}

fn speed_code(s: Speed) -> u32 {
    match s {
        Speed::Low => 1,
        Speed::Full => 2,
        Speed::High => 3,
        Speed::Super => 5,
        Speed::SuperPlus => 6,
    }
}

fn busid(info: &DeviceInfo) -> String {
    format!("{}-{}", info.bus_number, info.port)
}

fn describe(info: &DeviceInfo) -> Option<RemoteDevice> {
    let dd = info.descriptor()?;
    let active = info.active_config();
    let cfg = info
        .configs
        .iter()
        .filter_map(|c| ConfigView::parse(c).ok())
        .find(|v| v.header.value == active)
        .or_else(|| info.configs.first().and_then(|c| ConfigView::parse(c).ok()));
    let interfaces = cfg
        .map(|v| {
            v.interfaces
                .iter()
                .filter_map(|i| i.alts.iter().find(|a| a.desc.alt == 0).or(i.alts.first()))
                .map(|a| (a.desc.class, a.desc.subclass, a.desc.protocol))
                .collect()
        })
        .unwrap_or_default();
    Some(RemoteDevice {
        path: format!("/sys/devices/emuvc/usb{}/{}", info.bus_number, busid(info)),
        busid: busid(info),
        busnum: info.bus_number as u32,
        devnum: info.address as u32,
        speed: speed_code(info.speed),
        vendor_id: dd.vendor_id,
        product_id: dd.product_id,
        bcd_device: dd.bcd_device,
        class: dd.class,
        subclass: dd.subclass,
        protocol: dd.protocol,
        configuration_value: active,
        num_configurations: dd.num_configurations,
        interfaces,
    })
}

struct BusyGuard<'a>(&'a Server, DeviceId);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        lock(&self.0.busy).remove(&self.1);
    }
}

impl Server {
    fn connection(&self, mut stream: TcpStream) -> io::Result<()> {
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?; // operation phase only
        let h = read_vec(&mut stream, 8)?;
        let (version, code) = (be16(&h, 0), be16(&h, 2));
        if version != VERSION {
            debug!("usbip: client version {version:#06x}");
        }
        match code {
            OP_REQ_DEVLIST => {
                let devs: Vec<RemoteDevice> = self
                    .host
                    .devices()
                    .iter()
                    .filter_map(|d| describe(d))
                    .collect();
                let mut out = op_header(OP_REP_DEVLIST, 0);
                put32(&mut out, devs.len() as u32);
                for d in &devs {
                    out.extend_from_slice(&d.encode());
                    for &(c, s, p) in &d.interfaces {
                        out.extend_from_slice(&[c, s, p, 0]);
                    }
                }
                stream.write_all(&out)
            }
            OP_REQ_IMPORT => {
                let want = get_str(&read_vec(&mut stream, 32)?);
                self.import(stream, &want)
            }
            c => Err(bad(format!("unknown operation {c:#06x}"))),
        }
    }

    fn import(&self, mut stream: TcpStream, want: &str) -> io::Result<()> {
        let refuse = |stream: &mut TcpStream, why: &str| -> io::Result<()> {
            info!("usbip: import {want}: {why}");
            stream.write_all(&op_header(OP_REP_IMPORT, 1))
        };
        let Some(dev) = self.host.devices().into_iter().find(|d| busid(d) == want) else {
            return refuse(&mut stream, "no such device");
        };
        if !lock(&self.busy).insert(dev.id) {
            return refuse(&mut stream, "device busy");
        }
        let _guard = BusyGuard(self, dev.id);
        if let Err(e) = self.host.reset_device(dev.id) {
            return refuse(&mut stream, &format!("reset failed: {e}"));
        }
        let Some(desc) = describe(&dev) else {
            return refuse(&mut stream, "bad device descriptor");
        };
        let mut out = op_header(OP_REP_IMPORT, 0);
        out.extend_from_slice(&desc.encode());
        stream.write_all(&out)?;
        info!("usbip: imported {} ({want})", dev.name);
        stream.set_read_timeout(None)?;
        let r = self.urb_phase(stream, dev.clone());
        info!("usbip: {want} released");
        r
    }

    fn urb_phase(&self, stream: TcpStream, dev: Arc<DeviceInfo>) -> io::Result<()> {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let wstream = stream.try_clone()?;
        let shutdown = stream.try_clone()?;
        thread::Builder::new()
            .name("usbip-writer".into())
            .spawn(move || {
                let mut w = BufWriter::new(wstream);
                while let Ok(first) = rx.recv() {
                    let mut res = w.write_all(&first);
                    while res.is_ok() {
                        match rx.try_recv() {
                            Ok(more) => res = w.write_all(&more),
                            Err(_) => break,
                        }
                    }
                    if res.and_then(|_| w.flush()).is_err() {
                        let _ = shutdown.shutdown(Shutdown::Both);
                        break;
                    }
                }
            })?;
        let conn = Conn {
            host: &self.host,
            dev,
            tx,
            pending: Arc::new(Mutex::new(HashMap::new())),
        };
        let r = conn.run(BufReader::new(stream.try_clone()?));
        // Cancel everything still pending; completions are discarded.
        let ids: Vec<UrbId> = {
            let mut p = lock(&conn.pending);
            let ids = p.values().filter_map(|e| e.urb).collect();
            p.clear();
            ids
        };
        for id in ids {
            self.host.cancel(id);
        }
        let _ = stream.shutdown(Shutdown::Both);
        r
    }
}

struct Entry {
    urb: Option<UrbId>,
    unlinked: bool,
}

type Pending = Arc<Mutex<HashMap<u32, Entry>>>;

struct Conn<'a> {
    host: &'a Host,
    dev: Arc<DeviceInfo>,
    tx: Sender<Vec<u8>>,
    pending: Pending,
}

/// What the reply to a CMD_SUBMIT needs to remember about the request.
struct Req {
    seq: u32,
    is_in: bool,
    start_frame: u32,
    number_of_packets: u32,
    /// Isochronous: (offset, length) of each packet.
    iso: Vec<(u32, u32)>,
}

fn ret_header(command: u32, seq: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(URB_HEADER_LEN);
    put32(&mut v, command);
    put32(&mut v, seq);
    v.extend_from_slice(&[0; 12]); // devid, direction, ep
    v
}

fn status_code(r: Result<(), emuvc_usb::UsbError>) -> i32 {
    r.map_or_else(|e| e.errno(), |_| 0)
}

fn encode_ret_submit(req: &Req, c: &Completion) -> Vec<u8> {
    let status = status_code(c.status);
    let mut data: Vec<u8> = Vec::new();
    let mut descs: Vec<u8> = Vec::new();
    let mut errors = 0u32;
    let mut actual = c.actual_length as u32;
    if req.number_of_packets != NO_PACKETS && !req.iso.is_empty() {
        for (k, &(off, len)) in req.iso.iter().enumerate() {
            let (act, st) = match c.iso.get(k) {
                Some(r) => {
                    if req.is_in {
                        let n = r.data.len().min(r.actual_length).min(len as usize);
                        data.extend_from_slice(&r.data[..n]);
                    }
                    (r.actual_length as u32, status_code(r.status))
                }
                None => (0, status),
            };
            errors += (st != 0) as u32;
            put32(&mut descs, off);
            put32(&mut descs, len);
            put32(&mut descs, act);
            put32(&mut descs, st as u32);
        }
        if req.is_in {
            actual = data.len() as u32;
        }
    } else if req.is_in {
        data = c.data.clone();
        actual = data.len() as u32;
    }
    let mut v = ret_header(RET_SUBMIT, req.seq);
    put32(&mut v, status as u32);
    put32(&mut v, actual);
    put32(&mut v, req.start_frame);
    put32(&mut v, req.number_of_packets);
    put32(&mut v, errors);
    v.extend_from_slice(&[0; 8]);
    v.extend_from_slice(&data);
    v.extend_from_slice(&descs);
    v
}

impl Conn<'_> {
    fn run(&self, mut r: BufReader<TcpStream>) -> io::Result<()> {
        loop {
            let mut h = [0u8; URB_HEADER_LEN];
            match r.read_exact(&mut h) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
                Err(e) => return Err(e),
            }
            let (command, seq) = (be32(&h, 0), be32(&h, 4));
            match command {
                CMD_SUBMIT => self.submit(&mut r, &h, seq)?,
                CMD_UNLINK => self.unlink(seq, be32(&h, 20)),
                c => return Err(bad(format!("unknown URB command {c}"))),
            }
        }
    }

    fn submit(&self, r: &mut impl Read, h: &[u8; URB_HEADER_LEN], seq: u32) -> io::Result<()> {
        let (dir_in, ep) = (be32(h, 12) != 0, be32(h, 16));
        let flags = be32(h, 20);
        let tbl = be32(h, 24) as usize;
        let start_frame = be32(h, 28);
        let np = be32(h, 32);
        let setup: [u8; 8] = h[40..48].try_into().unwrap();
        if ep > 15 || tbl > MAX_TRANSFER {
            return Err(bad(format!("bad submit: ep {ep}, length {tbl}")));
        }
        let addr = ep as u8 | if dir_in { 0x80 } else { 0 };
        let iso_n = if np == NO_PACKETS || np == 0 {
            0
        } else {
            np as usize
        };
        if iso_n > MAX_ISO_PACKETS {
            return Err(bad(format!("too many iso packets: {np}")));
        }
        let mut req = Req {
            seq,
            is_in: dir_in,
            start_frame,
            number_of_packets: np,
            iso: Vec::new(),
        };
        let mut urb = if ep == 0 {
            let s = SetupPacket::from_bytes(setup);
            req.is_in = s.request_type & 0x80 != 0;
            let mut u = Urb::control(self.dev.id, s, Vec::new());
            if req.is_in {
                u.length = tbl;
            }
            u
        } else if iso_n > 0 {
            Urb {
                device: self.dev.id,
                endpoint: addr,
                kind: UrbKind::Iso(Vec::new()),
                data: Vec::new(),
                length: 0,
                timeout: None,
                short_not_ok: false,
            }
        } else {
            let kind = match self.dev.endpoint(addr).map(|e| e.transfer_type) {
                Some(TransferType::Interrupt) => UrbKind::Interrupt,
                _ => UrbKind::Bulk,
            };
            Urb {
                device: self.dev.id,
                endpoint: addr,
                kind,
                data: Vec::new(),
                length: tbl,
                timeout: None,
                short_not_ok: false,
            }
        };
        urb.short_not_ok = flags & URB_SHORT_NOT_OK != 0;
        // Payload: OUT data first, then (iso) the packet descriptors.
        if !req.is_in {
            urb.data = read_vec(r, tbl)?;
        }
        if iso_n > 0 {
            let raw = read_vec(r, iso_n * 16)?;
            let mut lens = Vec::new();
            let mut next = 0u64;
            for k in 0..iso_n {
                let (off, len) = (be32(&raw, k * 16), be32(&raw, k * 16 + 4));
                if off as u64 != next {
                    return Err(bad("non-contiguous iso packets are not supported"));
                }
                next += len as u64;
                req.iso.push((off, len));
                lens.push(len as usize);
            }
            if next > tbl as u64 {
                return Err(bad("iso packets exceed transfer buffer"));
            }
            urb.length = next as usize;
            urb.kind = UrbKind::Iso(lens);
        }
        // Register before submitting: the completion may beat us to it.
        {
            let mut p = lock(&self.pending);
            if p.insert(
                seq,
                Entry {
                    urb: None,
                    unlinked: false,
                },
            )
            .is_some()
            {
                return Err(bad(format!("duplicate seqnum {seq}")));
            }
        }
        let pending = self.pending.clone();
        let tx = self.tx.clone();
        let done: CompletionFn = Box::new(move |c| {
            let unlinked = match lock(&pending).remove(&req.seq) {
                Some(e) => e.unlinked,
                None => return, // connection gone
            };
            if !unlinked {
                let _ = tx.send(encode_ret_submit(&req, &c));
            }
        });
        // `req` moved into the closure; keep what the error path needs.
        match self.host.submit(urb, done) {
            Ok(id) => {
                if let Some(e) = lock(&self.pending).get_mut(&seq) {
                    e.urb = Some(id);
                }
            }
            Err(e) => {
                // The callback was dropped unused.
                lock(&self.pending).remove(&seq);
                let mut v = ret_header(RET_SUBMIT, seq);
                put32(&mut v, e.errno() as u32);
                put32(&mut v, 0);
                put32(&mut v, start_frame);
                put32(&mut v, np);
                put32(&mut v, 0);
                v.extend_from_slice(&[0; 8]);
                // Isochronous replies always carry their descriptors.
                if iso_n > 0 {
                    for _ in 0..iso_n {
                        v.extend_from_slice(&[0; 16]);
                    }
                }
                let _ = self.tx.send(v);
            }
        }
        Ok(())
    }

    fn unlink(&self, seq: u32, victim: u32) {
        let status = {
            let mut p = lock(&self.pending);
            match p.get_mut(&victim) {
                Some(e) => {
                    e.unlinked = true;
                    if let Some(id) = e.urb {
                        self.host.cancel(id);
                    }
                    -ECONNRESET
                }
                None => 0,
            }
        };
        let mut v = ret_header(RET_UNLINK, seq);
        put32(&mut v, status as u32);
        v.extend_from_slice(&[0; 24]);
        let _ = self.tx.send(v);
    }
}
