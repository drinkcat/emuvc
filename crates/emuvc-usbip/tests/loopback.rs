//! In-process USB/IP client against the server over a real localhost socket.

use std::collections::HashMap;
use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use emuvc_devices::DeviceConfig;
use emuvc_usb::{ClockMode, Host, HostConfig};
use emuvc_usbip::proto::*;
use emuvc_uvc::host::Reassembler;
use emuvc_uvc::stream::StreamingControl;
use emuvc_video::PixelFormat;
use emuvc_video::stamp::decode_frame;

struct Ret {
    command: u32,
    seq: u32,
    status: i32,
    actual: u32,
    data: Vec<u8>,
    /// (offset, length, actual_length, status)
    iso: Vec<(u32, u32, u32, i32)>,
}

struct Client {
    s: TcpStream,
    devid: u32,
    next: u32,
    /// seq -> (is_in, iso packets) of submitted URBs.
    inflight: HashMap<u32, (bool, usize)>,
}

impl Client {
    fn new(s: TcpStream, devid: u32) -> Self {
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        Client {
            s,
            devid,
            next: 1,
            inflight: HashMap::new(),
        }
    }

    fn submit(
        &mut self,
        ep: u32,
        dir_in: bool,
        tbl: u32,
        setup: [u8; 8],
        out: &[u8],
        iso: &[(u32, u32)],
    ) -> u32 {
        let seq = self.next;
        self.next += 1;
        let mut v = Vec::new();
        for x in [
            CMD_SUBMIT,
            seq,
            self.devid,
            dir_in as u32,
            ep,
            0,
            tbl,
            0,
            if iso.is_empty() {
                NO_PACKETS
            } else {
                iso.len() as u32
            },
            0,
        ] {
            put32(&mut v, x);
        }
        v.extend_from_slice(&setup);
        v.extend_from_slice(out);
        for &(off, len) in iso {
            for x in [off, len, 0, 0] {
                put32(&mut v, x);
            }
        }
        self.s.write_all(&v).unwrap();
        self.inflight.insert(seq, (dir_in, iso.len()));
        seq
    }

    fn unlink(&mut self, victim: u32) -> u32 {
        let seq = self.next;
        self.next += 1;
        let mut v = Vec::new();
        for x in [CMD_UNLINK, seq, self.devid, 0, 0, victim] {
            put32(&mut v, x);
        }
        v.resize(URB_HEADER_LEN, 0);
        self.s.write_all(&v).unwrap();
        seq
    }

    fn recv(&mut self) -> Ret {
        let h = read_vec(&mut self.s, URB_HEADER_LEN).unwrap();
        let (command, seq) = (be32(&h, 0), be32(&h, 4));
        let status = be32(&h, 20) as i32;
        if command == RET_UNLINK {
            return Ret {
                command,
                seq,
                status,
                actual: 0,
                data: vec![],
                iso: vec![],
            };
        }
        assert_eq!(command, RET_SUBMIT);
        let actual = be32(&h, 24);
        let (is_in, np) = self.inflight.remove(&seq).expect("reply to unknown seq");
        let data = if is_in {
            read_vec(&mut self.s, actual as usize).unwrap()
        } else {
            vec![]
        };
        let mut iso = Vec::new();
        if np > 0 {
            let raw = read_vec(&mut self.s, np * 16).unwrap();
            for k in 0..np {
                iso.push((
                    be32(&raw, k * 16),
                    be32(&raw, k * 16 + 4),
                    be32(&raw, k * 16 + 8),
                    be32(&raw, k * 16 + 12) as i32,
                ));
            }
            let errs = iso.iter().filter(|p| p.3 != 0).count() as u32;
            assert_eq!(be32(&h, 36), errs, "error_count");
        }
        Ret {
            command,
            seq,
            status,
            actual,
            data,
            iso,
        }
    }

    /// Control transfer; other replies arriving meanwhile are collected.
    fn control(&mut self, setup: [u8; 8], out: &[u8], others: &mut Vec<Ret>) -> Ret {
        let dir_in = setup[0] & 0x80 != 0;
        let len = u16::from_le_bytes([setup[6], setup[7]]) as u32;
        let seq = self.submit(0, dir_in, len, setup, if dir_in { &[] } else { out }, &[]);
        loop {
            let r = self.recv();
            if r.seq == seq {
                return r;
            }
            others.push(r);
        }
    }

    fn ctl(&mut self, setup: [u8; 8], out: &[u8]) -> Ret {
        let r = self.control(setup, out, &mut Vec::new());
        assert_eq!(r.status, 0, "control {setup:02x?}");
        r
    }
}

fn setup(rt: u8, req: u8, value: u16, index: u16, len: u16) -> [u8; 8] {
    emuvc_usb::SetupPacket::new(rt, req, value, index, len).to_bytes()
}

fn start() -> (Arc<Host>, SocketAddr) {
    let host = Arc::new(Host::new(HostConfig {
        clock: ClockMode::Virtual,
        ..Default::default()
    }));
    host.attach(emuvc_devices::build(&DeviceConfig::model("luxvisions-30c9-0052")).unwrap())
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let h = host.clone();
    std::thread::spawn(move || emuvc_usbip::serve(h, listener));
    (host, addr)
}

#[test]
fn usbip_loopback() {
    let (_host, addr) = start();

    // -- devlist --
    let devs = emuvc_usbip::list(addr).unwrap();
    assert_eq!(devs.len(), 1);
    let d = &devs[0];
    assert_eq!(d.busid, "1-1");
    assert_eq!((d.vendor_id, d.product_id), (0x30c9, 0x0052));
    assert_eq!(d.interfaces.len(), 5);
    assert_eq!(d.speed, 3);
    assert_eq!(d.interfaces[0], (14, 1, 1));

    // -- import --
    let (stream, rd) = emuvc_usbip::import(addr, "1-1").unwrap();
    assert_eq!(
        (rd.busid.as_str(), rd.vendor_id, rd.speed),
        ("1-1", 0x30c9, 3)
    );
    assert!(
        emuvc_usbip::import(addr, "1-1").is_err(),
        "second import must be refused"
    );
    assert!(emuvc_usbip::import(addr, "9-9").is_err());
    let mut c = Client::new(stream, rd.devid());

    // -- descriptors --
    let r = c.ctl(setup(0x80, 6, 0x0100, 0, 18), &[]);
    assert_eq!((r.data.len(), r.data[1]), (18, 1));
    assert_eq!(u16::from_le_bytes([r.data[8], r.data[9]]), 0x30c9);
    let r = c.ctl(setup(0x80, 6, 0x0200, 0, 9), &[]);
    let total = u16::from_le_bytes([r.data[2], r.data[3]]);
    let r = c.ctl(setup(0x80, 6, 0x0200, 0, total), &[]);
    assert_eq!(r.data.len(), total as usize);
    assert_eq!(r.data[1], 2);

    // -- probe / commit (format 2 frame 5: YUY2 640x480) --
    let len = StreamingControl::len_for(0x0150) as u16;
    let req = StreamingControl {
        hint: 1,
        format_index: 2,
        frame_index: 5,
        frame_interval: 333_333,
        ..Default::default()
    };
    let sc = |rt, r, cs: u16| setup(rt, r, cs << 8, 1, len);
    c.ctl(sc(0x21, 0x01, 1), &req.to_bytes(len as usize));
    let cur = StreamingControl::from_bytes(&c.ctl(sc(0xa1, 0x81, 1), &[]).data);
    c.ctl(sc(0x21, 0x01, 2), &cur.to_bytes(len as usize));
    assert_eq!((cur.format_index, cur.frame_index), (2, 5));

    // -- SET_INTERFACE alt 10, isochronous streaming --
    c.ctl(setup(0x01, 0x0b, 10, 1, 0), &[]);
    let iso: Vec<(u32, u32)> = (0..32).map(|k| (k * 2880, 2880)).collect();
    let submit_iso = |c: &mut Client| c.submit(1, true, 32 * 2880, [0; 8], &[], &iso);
    for _ in 0..6 {
        submit_iso(&mut c);
    }
    let mut r = Reassembler::default();
    while r.frames.len() < 4 {
        let ret = c.recv();
        assert_eq!(ret.status, 0);
        assert_eq!(ret.iso.len(), 32);
        let mut pos = 0;
        for p in &ret.iso {
            assert_eq!((p.0 % 2880, p.1), (0, 2880));
            assert_eq!(p.3, 0);
            r.push(&ret.data[pos..pos + p.2 as usize]);
            pos += p.2 as usize;
        }
        assert_eq!(pos, ret.data.len());
        assert_eq!(ret.actual as usize, pos);
        submit_iso(&mut c);
    }
    let mut prev = None;
    for (data, err) in &r.frames {
        assert!(!err);
        assert_eq!(data.len(), 640 * 480 * 2);
        let s = decode_frame(PixelFormat::Yuyv, data, 640, 480).expect("stamp");
        if let Some(p) = prev {
            assert_eq!(s, p + 1, "consecutive stamps");
        }
        prev = Some(s);
    }
    // Back to alt 0: outstanding URBs are flushed with an error status.
    let mut others = Vec::new();
    let r0 = c.control(setup(0x01, 0x0b, 0, 1, 0), &[], &mut others);
    assert_eq!(r0.status, 0);
    while !c.inflight.is_empty() {
        let o = c.recv();
        assert!(o.status != 0 || o.iso.len() == 32);
    }

    // -- unlink of a pending interrupt URB --
    let intr = c.submit(7, true, 16, [0; 8], &[], &[]);
    std::thread::sleep(Duration::from_millis(50));
    let un = c.unlink(intr);
    let ru = c.recv();
    assert_eq!(
        (ru.command, ru.seq, ru.status),
        (RET_UNLINK, un, -ECONNRESET)
    );
    c.inflight.remove(&intr);
    // Next traffic is the control reply, not a RET_SUBMIT for the interrupt URB.
    let r = c.ctl(setup(0x80, 6, 0x0100, 0, 18), &[]);
    assert_eq!(r.data.len(), 18);
    // Unlinking something that is gone answers 0.
    let un = c.unlink(intr);
    let ru = c.recv();
    assert_eq!((ru.command, ru.seq, ru.status), (RET_UNLINK, un, 0));

    // -- release: a new import succeeds once the connection is gone --
    drop(c);
    let t = Instant::now();
    loop {
        match emuvc_usbip::import(addr, "1-1") {
            Ok(_) => break,
            Err(e) => assert!(t.elapsed() < Duration::from_secs(5), "never released: {e}"),
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
