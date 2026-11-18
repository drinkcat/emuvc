//! The synthetic camera: variants stream, still capture works, events fire.

use std::sync::mpsc;

use emuvc_devices::synthetic::SyntheticConfig;
use emuvc_devices::{DeviceConfig, build};
use emuvc_usb::desc::ConfigView;
use emuvc_usb::*;
use emuvc_uvc::consts::*;
use emuvc_uvc::host::{Reassembler, capture, probe_commit};
use emuvc_uvc::stream::{StillControl, pixel_format};
use emuvc_uvc::topology::{Transfer, parse_function};
use emuvc_uvc::{ControlSpec, EventKind, EventSpec};
use emuvc_video::stamp;

fn host() -> Host {
    Host::new(HostConfig {
        clock: ClockMode::Virtual,
        ..Default::default()
    })
}

fn synthetic(s: SyntheticConfig) -> DeviceConfig {
    DeviceConfig {
        synthetic: s,
        ..DeviceConfig::model("synthetic")
    }
}

/// Streams 2 frames of the first frame size of every format of every VS interface.
fn stream_all(cfg: &DeviceConfig) {
    let host = host();
    let info = host.attach(build(cfg).unwrap()).unwrap();
    let view = ConfigView::parse(&info.configs[0]).unwrap();
    let f = parse_function(&view, 0, info.speed).unwrap();
    assert_eq!(f.streaming.len(), cfg.synthetic.streams as usize);
    for vs in &f.streaming {
        for fmt in &vs.formats {
            let Some(pf) = pixel_format(&fmt.kind) else {
                continue;
            };
            let fr = &fmt.frames[0];
            let c = capture(&host, info.id, vs, fmt.index, fr.index, 2)
                .unwrap_or_else(|e| panic!("{:?} {pf:?}: {e}", cfg.synthetic));
            for (data, err) in &c.frames {
                assert!(!err);
                if let Some(n) = pf.frame_size(fr.width, fr.height) {
                    assert_eq!(data.len(), n, "{pf:?}");
                }
                assert!(
                    stamp::decode_frame(pf, data, fr.width as usize, fr.height as usize).is_some(),
                    "{pf:?} stamp"
                );
            }
        }
    }
}

#[test]
fn variants_stream() {
    let base = SyntheticConfig::default;
    stream_all(&synthetic(base()));
    stream_all(&synthetic(SyntheticConfig {
        speed: Speed::Super,
        ..base()
    }));
    stream_all(&synthetic(SyntheticConfig {
        speed: Speed::Super,
        transfer: "bulk".into(),
        ..base()
    }));
    stream_all(&synthetic(SyntheticConfig {
        transfer: "bulk".into(),
        still: 3,
        ..base()
    }));
    stream_all(&synthetic(SyntheticConfig {
        uvc: "1.0".into(),
        speed: Speed::Full,
        sizes: vec![(160, 120)],
        fps: vec![5],
        ..base()
    }));
    stream_all(&synthetic(SyntheticConfig {
        uvc: "1.1".into(),
        streams: 2,
        ..base()
    }));
    stream_all(&synthetic(SyntheticConfig {
        continuous: true,
        ..base()
    }));
}

#[test]
fn all_formats_build() {
    let all = [
        "mjpeg", "h264", "yuyv", "uyvy", "nv12", "nv21", "i420", "p010", "grey", "grey16", "bgr24",
        "rgb24", "by8", "ba81", "grbg8", "gbrg8", "rggb8", "bggr8",
    ];
    let s = SyntheticConfig {
        formats: all.map(String::from).to_vec(),
        sizes: vec![(64, 48)],
        fps: vec![30],
        extra_formats: true,
        ..Default::default()
    };
    stream_all(&synthetic(s));
}

fn uvc_set(
    host: &Host,
    dev: DeviceId,
    cs: u8,
    iface: u8,
    data: &[u8],
) -> Result<Vec<u8>, UsbError> {
    host.control(
        dev,
        SetupPacket::new(
            0x21,
            SET_CUR,
            (cs as u16) << 8,
            iface as u16,
            data.len() as u16,
        ),
        data,
        None,
    )
}

#[test]
fn still_method_2_and_3() {
    for method in [2u8, 3] {
        let cfg = synthetic(SyntheticConfig {
            still: method,
            formats: vec!["yuyv".into()],
            ..Default::default()
        });
        let host = host();
        let dev = host.attach(build(&cfg).unwrap()).unwrap().id;
        // Video at 160x120 (frame 3), still at 640x480 (still frame index 1).
        let commit = probe_commit(&host, dev, 1, 1, 3, 333333, 48).unwrap();
        let still = StillControl {
            format_index: 1,
            frame_index: 1,
            ..Default::default()
        };
        uvc_set(&host, dev, VS_STILL_PROBE_CONTROL, 1, &still.to_bytes()).unwrap();
        uvc_set(&host, dev, VS_STILL_COMMIT_CONTROL, 1, &still.to_bytes()).unwrap();
        host.control(dev, SetupPacket::new(0x01, 0x0b, 5, 1, 0), &[], None)
            .unwrap(); // alt 5: 3x1024
        let (tx, rx) = mpsc::channel();
        let submit = |tx: &mpsc::Sender<Completion>| {
            let tx = tx.clone();
            host.submit(
                Urb::iso_in(dev, 0x81, vec![3072; 32]),
                Box::new(move |c| drop(tx.send(c))),
            )
            .unwrap();
        };
        for _ in 0..4 {
            submit(&tx);
        }
        uvc_set(
            &host,
            dev,
            VS_STILL_IMAGE_TRIGGER_CONTROL,
            1,
            &[if method == 2 { 1 } else { 2 }],
        )
        .unwrap();
        let mut r = Reassembler::default();
        let mut stills = 0;
        if method == 3 {
            // The still comes on the dedicated bulk endpoint.
            let mut s = Reassembler::default();
            while s.frames.is_empty() {
                let c = host.transfer(Urb::bulk_in(dev, 0x88, 32768));
                s.push(&c.data);
            }
            assert_eq!(s.frames[0].0.len(), 640 * 480 * 2);
            stills += 1;
        }
        while r.frames.len() < 6 {
            let c = rx.recv().unwrap();
            for p in &c.iso {
                r.push(&p.data);
            }
            submit(&tx);
        }
        stills += r
            .frames
            .iter()
            .filter(|(f, _)| f.len() == 640 * 480 * 2)
            .count();
        assert_eq!(stills, 1, "method {method}: exactly one still image");
        assert!(
            r.frames
                .iter()
                .any(|(f, _)| f.len() == commit.max_video_frame_size as usize)
        );
    }
}

#[test]
fn events_fire() {
    let mut cfg = synthetic(SyntheticConfig::default());
    cfg.events = vec![
        EventSpec {
            at_ms: 10,
            every_ms: Some(20),
            count: Some(2),
            kind: EventKind::Button,
        },
        EventSpec {
            at_ms: 5,
            every_ms: None,
            count: None,
            kind: EventKind::Control {
                entity: 3,
                selector: PU_BRIGHTNESS_CONTROL,
                value: vec![7, 0],
                attribute: STATUS_ATTR_VALUE,
            },
        },
        EventSpec {
            at_ms: 60,
            every_ms: None,
            count: None,
            kind: EventKind::Raw(vec![1, 2]),
        },
    ];
    let host = host();
    let dev = host.attach(build(&cfg).unwrap()).unwrap().id;
    let mut got = Vec::new();
    for _ in 0..6 {
        got.push(host.transfer(Urb::interrupt_in(dev, 0x87, 16)).data);
    }
    assert_eq!(
        got[0],
        vec![
            STATUS_ORIGINATOR_VC,
            3,
            0,
            PU_BRIGHTNESS_CONTROL,
            STATUS_ATTR_VALUE,
            7,
            0
        ]
    );
    assert_eq!(got[1], vec![STATUS_ORIGINATOR_VS, 1, 0, 1]);
    assert_eq!(got[2], vec![STATUS_ORIGINATOR_VS, 1, 0, 0]);
    assert_eq!(got[3..5], [vec![2, 1, 0, 1], vec![2, 1, 0, 0]]);
    assert_eq!(got[5], vec![1, 2]);
    // The control really changed.
    let v = host.control(
        dev,
        SetupPacket::new(
            0xa1,
            GET_CUR,
            (PU_BRIGHTNESS_CONTROL as u16) << 8,
            3 << 8,
            2,
        ),
        &[],
        None,
    );
    assert_eq!(v, Ok(vec![7, 0]));
}

#[test]
fn bulk_transfer_kind() {
    let cfg = synthetic(SyntheticConfig {
        transfer: "bulk".into(),
        ..Default::default()
    });
    let host = host();
    let info = host.attach(build(&cfg).unwrap()).unwrap();
    let view = ConfigView::parse(&info.configs[0]).unwrap();
    let f = parse_function(&view, 0, info.speed).unwrap();
    assert!(matches!(
        f.streaming[0].transfer,
        Transfer::Bulk { max_packet: 512 }
    ));
}

fn pu_get(host: &Host, dev: DeviceId, cs: u8) -> Result<Vec<u8>, UsbError> {
    host.control(
        dev,
        SetupPacket::new(0xa1, GET_CUR, (cs as u16) << 8, 3 << 8, 2),
        &[],
        None,
    )
}

fn pu_set(host: &Host, dev: DeviceId, cs: u8, v: &[u8]) -> Result<Vec<u8>, UsbError> {
    let setup = SetupPacket::new(0x21, SET_CUR, (cs as u16) << 8, 3 << 8, v.len() as u16);
    host.control(dev, setup, v, None)
}

fn request_error(host: &Host, dev: DeviceId) -> u8 {
    let setup = SetupPacket::new(
        0xa1,
        GET_CUR,
        (VC_REQUEST_ERROR_CODE_CONTROL as u16) << 8,
        0,
        1,
    );
    host.control(dev, setup, &[], None).unwrap()[0]
}

/// UVC 1.5 2.4.4: asynchronous SET_CUR completes with a Control Change interrupt (value or
/// failure); the control is "not ready" meanwhile, the others are not.
#[test]
fn async_controls() {
    let mut cfg = synthetic(SyntheticConfig::default());
    cfg.controls = vec![
        ControlSpec {
            entity: 3,
            selector: PU_BRIGHTNESS_CONTROL,
            async_ms: Some(50),
            fail: None,
            ..Default::default()
        },
        ControlSpec {
            entity: 3,
            selector: PU_CONTRAST_CONTROL,
            async_ms: Some(10),
            fail: Some(ERR_WRONG_STATE),
            ..Default::default()
        },
    ];
    let host = host();
    let dev = host.attach(build(&cfg).unwrap()).unwrap().id;
    let info = |cs: u8| {
        let setup = SetupPacket::new(0xa1, GET_INFO, (cs as u16) << 8, 3 << 8, 1);
        host.control(dev, setup, &[], None).unwrap()[0]
    };
    assert_ne!(info(PU_BRIGHTNESS_CONTROL) & INFO_ASYNC, 0);
    let status = || host.transfer(Urb::interrupt_in(dev, 0x87, 16)).data;

    // Invalid values still STALL right away.
    assert!(pu_set(&host, dev, PU_BRIGHTNESS_CONTROL, &[0xff, 0x7f]).is_err());
    assert_eq!(request_error(&host, dev), ERR_OUT_OF_RANGE);

    assert_eq!(
        pu_set(&host, dev, PU_BRIGHTNESS_CONTROL, &[5, 0]),
        Ok(vec![])
    );
    assert!(pu_get(&host, dev, PU_BRIGHTNESS_CONTROL).is_err());
    assert_eq!(request_error(&host, dev), ERR_NOT_READY);
    assert!(pu_set(&host, dev, PU_BRIGHTNESS_CONTROL, &[6, 0]).is_err());
    assert_eq!(request_error(&host, dev), ERR_NOT_READY);
    assert_eq!(pu_get(&host, dev, PU_SATURATION_CONTROL), Ok(vec![64, 0]));
    assert_eq!(
        status(),
        [
            STATUS_ORIGINATOR_VC,
            3,
            0,
            PU_BRIGHTNESS_CONTROL,
            STATUS_ATTR_VALUE,
            5,
            0
        ]
    );
    assert_eq!(pu_get(&host, dev, PU_BRIGHTNESS_CONTROL), Ok(vec![5, 0]));

    // Failure: Control Failure Change with the error code, value unchanged.
    assert_eq!(
        pu_set(&host, dev, PU_CONTRAST_CONTROL, &[10, 0]),
        Ok(vec![])
    );
    assert_eq!(
        status(),
        [
            STATUS_ORIGINATOR_VC,
            3,
            0,
            PU_CONTRAST_CONTROL,
            STATUS_ATTR_FAILURE,
            ERR_WRONG_STATE
        ]
    );
    assert_eq!(pu_get(&host, dev, PU_CONTRAST_CONTROL), Ok(vec![32, 0]));

    // Synchronous autoupdate controls report every SET_CUR too (2.4.2.2).
    assert_eq!(pu_set(&host, dev, PU_HUE_CONTROL, &[3, 0]), Ok(vec![]));
    assert_eq!(
        status(),
        [
            STATUS_ORIGINATOR_VC,
            3,
            0,
            PU_HUE_CONTROL,
            STATUS_ATTR_VALUE,
            3,
            0
        ]
    );
}

/// UVC 1.5 2.4.3.3 timestamps with the clock quirks: a wrapping clock, a PTS one interval
/// back every 3rd frame, PTS only in a frame's first payload, a stuck SCR.
#[test]
fn clock_quirks() {
    let mut cfg = synthetic(SyntheticConfig {
        transfer: "bulk".into(),
        ..Default::default()
    });
    cfg.uvc.clock = emuvc_uvc::stream::ClockQuirks {
        start_ticks: 0xffff_0000,
        pts_backwards_every: Some(3),
        scr_stuck: true,
        pts_first_only: true,
        ..Default::default()
    };
    let host = host();
    let info = host.attach(build(&cfg).unwrap()).unwrap();
    let view = ConfigView::parse(&info.configs[0]).unwrap();
    let vs = &parse_function(&view, 0, info.speed).unwrap().streaming[0];
    let c = probe_commit(&host, info.id, vs.interface, 2, 1, 333_333, 48).unwrap();
    let interval_ticks = (c.frame_interval as u64 * 48_000_000 / 10_000_000) as u32;

    // (pts of each frame, stuck SCR clock values)
    let (mut pts, mut stc, mut fid, mut payloads) = (Vec::new(), Vec::new(), None, 0);
    while pts.len() < 8 {
        let p = host
            .transfer(Urb::bulk_in(info.id, vs.endpoint, 64 * 1024))
            .data;
        let (hlen, bfh) = (p[0] as usize, p[1]);
        payloads += 1;
        let first = fid != Some(bfh & HDR_FID);
        fid = Some(bfh & HDR_FID);
        assert_eq!(bfh & HDR_PTS != 0, first, "PTS only in the first payload");
        assert_eq!(hlen, if first { 12 } else { 8 });
        assert_ne!(bfh & HDR_SCR, 0);
        if first {
            pts.push(u32::from_le_bytes(p[2..6].try_into().unwrap()));
        }
        stc.push(u32::from_le_bytes(
            p[hlen - 6..hlen - 2].try_into().unwrap(),
        ));
    }
    assert!(payloads > pts.len(), "frames span several payloads");
    assert!(stc.iter().all(|&s| s == 0xffff_0000), "SCR stuck: {stc:x?}");
    assert!(
        pts[0] >= 0xffff_0000 || pts.iter().any(|&p| p < pts[0]),
        "clock wraps"
    );
    let deltas: Vec<u32> = pts.windows(2).map(|w| w[1].wrapping_sub(w[0])).collect();
    // Each PTS is rounded down from nanoseconds separately: +-1 tick.
    let ok = |d: u32| {
        [0, interval_ticks, 2 * interval_ticks]
            .iter()
            .any(|&e| d.abs_diff(e) <= 1)
    };
    assert!(
        deltas.iter().all(|&d| ok(d)),
        "deltas {deltas:?} vs {interval_ticks}"
    );
    assert!(deltas.contains(&0), "a PTS one interval back: {deltas:?}");
}

/// Payloads of a bulk YUYV stream (one per transfer) from a synthetic camera with `quirks`.
fn bulk_payloads(quirks: emuvc_uvc::stream::StreamQuirks, n: usize) -> Vec<Vec<u8>> {
    bulk_payloads_of(quirks, n, 2)
}

/// Payloads of a bulk stream of format `format` (1 MJPEG, 2 YUYV), first frame size.
fn bulk_payloads_of(quirks: emuvc_uvc::stream::StreamQuirks, n: usize, format: u8) -> Vec<Vec<u8>> {
    let mut cfg = synthetic(SyntheticConfig {
        transfer: "bulk".into(),
        ..Default::default()
    });
    cfg.uvc = quirks;
    let host = host();
    let info = host.attach(build(&cfg).unwrap()).unwrap();
    let view = ConfigView::parse(&info.configs[0]).unwrap();
    let vs = &parse_function(&view, 0, info.speed).unwrap().streaming[0];
    probe_commit(&host, info.id, vs.interface, format, 1, 333_333, 48).unwrap();
    (0..n)
        .map(|_| {
            host.transfer(Urb::bulk_in(info.id, vs.endpoint, 64 * 1024))
                .data
        })
        .collect()
}

/// Payload framing quirks: EOF in a header-only payload, ERR on one payload in the middle
/// of each frame, a bogus bHeaderLength of 1.
#[test]
fn payload_framing_quirks() {
    use emuvc_uvc::stream::StreamQuirks;
    let p = bulk_payloads(
        StreamQuirks {
            eof_payload: true,
            ..Default::default()
        },
        80,
    );
    let eofs: Vec<&Vec<u8>> = p.iter().filter(|p| p[1] & HDR_EOF != 0).collect();
    assert!(eofs.len() >= 2, "frames end");
    assert!(
        eofs.iter().all(|p| p.len() == p[0] as usize),
        "EOF payloads carry no data"
    );

    let p = bulk_payloads(
        StreamQuirks {
            err_payload: Some(1),
            ..Default::default()
        },
        80,
    );
    let (mut errs, mut frames) = (0, 0);
    for p in &p {
        errs += (p[1] & HDR_ERR != 0) as u32;
        if p[1] & HDR_EOF != 0 {
            assert_eq!(errs, 1, "one ERR payload in frame {frames}");
            (errs, frames) = (0, frames + 1);
        }
    }
    assert!(frames >= 2, "frames end");

    let p = bulk_payloads(
        StreamQuirks {
            header_len: Some(1),
            ..Default::default()
        },
        4,
    );
    assert!(
        p.iter().all(|p| p[0] == 1 && p.len() > 1),
        "bHeaderLength 1, then data"
    );
}

/// `frame_extra`: every frame carries junk bytes past its size, before EOF.
#[test]
fn frame_extra_bytes() {
    let p = bulk_payloads(
        emuvc_uvc::stream::StreamQuirks {
            frame_extra: 1000,
            ..Default::default()
        },
        80,
    );
    let (mut bytes, mut frames) = (0, 0);
    for p in &p {
        bytes += p.len() - p[0] as usize;
        if p[1] & HDR_EOF != 0 {
            if frames > 0 {
                assert_eq!(bytes, 640 * 480 * 2 + 1000, "frame {frames}");
            }
            (bytes, frames) = (0, frames + 1);
        }
    }
    assert!(frames >= 3, "frames end");
}

/// MJPEG frames without their SOI marker; every 2nd frame empty (a header-only payload).
#[test]
fn soi_and_empty_frames() {
    use emuvc_uvc::stream::StreamQuirks;
    let p = bulk_payloads_of(
        StreamQuirks {
            mjpeg_strip_soi: true,
            ..Default::default()
        },
        40,
        1,
    );
    let mut first = true;
    for p in &p {
        let data = &p[p[0] as usize..];
        if first && !data.is_empty() {
            assert_ne!(&data[..2], &[0xff, 0xd8], "no SOI at the start of a frame");
        }
        first = p[1] & HDR_EOF != 0;
    }

    let p = bulk_payloads(
        StreamQuirks {
            empty_every: Some(2),
            ..Default::default()
        },
        80,
    );
    let (mut bytes, mut empty, mut frames) = (0, 0, 0);
    for p in &p {
        bytes += p.len() - p[0] as usize;
        if p[1] & HDR_EOF != 0 {
            empty += (bytes == 0) as u32;
            (bytes, frames) = (0, frames + 1);
        }
    }
    assert!(
        frames >= 4 && empty >= 1,
        "{empty} empty frames of {frames}"
    );
}

/// `bulk_stop_clear_halt`: SET_INTERFACE 0 doesn't stop a bulk stream, a commit then STALLs,
/// and CLEAR_FEATURE(ENDPOINT_HALT) stops it.
#[test]
fn bulk_stop_clear_halt() {
    let mut cfg = synthetic(SyntheticConfig {
        transfer: "bulk".into(),
        ..Default::default()
    });
    cfg.uvc.bulk_stop_clear_halt = true;
    let host = host();
    let info = host.attach(build(&cfg).unwrap()).unwrap();
    let view = ConfigView::parse(&info.configs[0]).unwrap();
    let vs = &parse_function(&view, 0, info.speed).unwrap().streaming[0];
    probe_commit(&host, info.id, vs.interface, 2, 1, 333_333, 48).unwrap();
    let setup = |rt: u8, req: u8, val: u16, idx: u16| SetupPacket::new(rt, req, val, idx, 0);
    // SET_INTERFACE alt 0: still streaming, so the next commit is refused.
    host.control(
        info.id,
        setup(0x01, 0x0b, 0, vs.interface as u16),
        &[],
        None,
    )
    .unwrap();
    assert!(probe_commit(&host, info.id, vs.interface, 2, 1, 333_333, 48).is_err());
    // CLEAR_FEATURE(ENDPOINT_HALT) on the streaming endpoint stops it.
    host.control(info.id, setup(0x02, 0x01, 0, vs.endpoint as u16), &[], None)
        .unwrap();
    assert!(probe_commit(&host, info.id, vs.interface, 2, 1, 333_333, 48).is_ok());
}

/// Payload header metadata (after the SCR: the frame index, then filler) belongs to frames: all
/// the payloads of a frame carry the same, idle payloads between frames none, and neither do the
/// frames `no_metadata_every` picks.
#[test]
fn header_metadata() {
    let mut cfg = synthetic(SyntheticConfig::default());
    cfg.uvc.header_len = Some(20);
    cfg.uvc.no_metadata_every = Some(2);
    let host = host();
    let dev = host.attach(build(&cfg).unwrap()).unwrap().id;
    probe_commit(&host, dev, 1, 1, 3, 333333, 48).unwrap(); // MJPEG 160x120
    host.control(dev, SetupPacket::new(0x01, 0x0b, 5, 1, 0), &[], None)
        .unwrap(); // alt 5: 3x1024
    // The metadata's frame index (None: no metadata) of each payload with data, per frame (FID
    // run); the header lengths of the idle payloads.
    let (mut frames, mut idle, mut fid) = (Vec::<Vec<Option<u32>>>::new(), Vec::new(), None);
    while frames.len() < 7 {
        let c = host.transfer(Urb::iso_in(dev, 0x81, vec![3072; 32]));
        for p in c.iso.iter().map(|p| &p.data).filter(|d| !d.is_empty()) {
            let (hlen, info) = (p[0] as usize, p[1]);
            if p.len() == hlen {
                idle.push(hlen);
                continue;
            }
            if fid != Some(info & HDR_FID) {
                frames.push(Vec::new());
                fid = Some(info & HDR_FID);
            }
            assert!(hlen == 20 || hlen == 12, "bHeaderLength {hlen}");
            let meta = (hlen == 20).then(|| u32::from_le_bytes(p[12..16].try_into().unwrap()));
            frames.last_mut().unwrap().push(meta);
        }
    }
    assert!(
        !idle.is_empty() && idle.iter().all(|&h| h == 12),
        "idle headers {idle:?}"
    );
    let whole = &frames[..frames.len() - 1];
    for f in whole {
        assert!(f.iter().all(|&m| m == f[0]), "one frame's metadata: {f:?}");
        assert!(
            f[0].is_none_or(|i| i % 2 == 0),
            "odd frames have none: {f:?}"
        );
    }
    assert!(whole.iter().any(|f| f[0].is_none()) && whole.iter().any(|f| f[0] == Some(2)));
}

/// Bulk payloads read the way the host sees them: transfers of `urb` bytes, a payload ending
/// with a short transfer (a zero-length one included) or at dwMaxPayloadTransferSize, as
/// uvcvideo reassembles them. Returns the frames and the number of zero-length transfers.
fn bulk_frames_by_urb(
    cfg: &DeviceConfig,
    urb: usize,
    frames: usize,
) -> (Reassembler, usize, usize) {
    let host = host();
    let info = host.attach(build(cfg).unwrap()).unwrap();
    let view = ConfigView::parse(&info.configs[0]).unwrap();
    let vs = &parse_function(&view, 0, info.speed).unwrap().streaming[0];
    let c = probe_commit(&host, info.id, vs.interface, 2, 1, 333_333, 48).unwrap();
    let max = c.max_payload_transfer_size as usize;
    let size = {
        let f = vs.format(2).unwrap().frame(1).unwrap();
        f.width as usize * f.height as usize * 2
    };
    let (mut r, mut payload, mut zlps) = (Reassembler::default(), Vec::new(), 0);
    while r.frames.len() < frames {
        let d = host.transfer(Urb::bulk_in(info.id, vs.endpoint, urb)).data;
        zlps += d.is_empty() as usize;
        payload.extend_from_slice(&d);
        if d.len() < urb || payload.len() >= max {
            r.push(&std::mem::take(&mut payload));
        }
    }
    (r, zlps, size)
}

/// A host transfer smaller than a bulk payload gets part of it, and the next transfers the rest
/// (uvcvideo's 16 KiB URBs, 32 KiB payloads); a short payload that fills a transfer exactly ends
/// with a zero-length packet.
#[test]
fn bulk_payloads_span_transfers() {
    let mut cfg = synthetic(SyntheticConfig {
        transfer: "bulk".into(),
        ..Default::default()
    });
    let (r, _, size) = bulk_frames_by_urb(&cfg, 16 * 1024, 3);
    assert_eq!(r.bad_headers, 0);
    assert!(r.frames.iter().all(|(f, err)| f.len() == size && !err));
    // 16 KiB payloads under a 32 KiB dwMaxPayloadTransferSize: each fills a transfer, and only
    // the zero-length packet after it tells the host it ended.
    cfg.uvc.bulk_payload_size = Some(16 * 1024);
    let (r, zlps, size) = bulk_frames_by_urb(&cfg, 16 * 1024, 3);
    assert_eq!(r.bad_headers, 0);
    assert!(r.frames.iter().all(|(f, err)| f.len() == size && !err));
    assert!(
        zlps >= r.payloads - 3,
        "{zlps} zero-length packets, {} payloads",
        r.payloads
    );
}

/// The iSight packet format: a tagged header packet per frame, then data without headers.
#[test]
fn isight_packets() {
    const TAG: [u8; 12] = [
        0x11, 0x22, 0x33, 0x44, 0xde, 0xad, 0xbe, 0xef, 0xde, 0xad, 0xfa, 0xce,
    ];
    let p = bulk_payloads(
        emuvc_uvc::stream::StreamQuirks {
            isight: true,
            ..Default::default()
        },
        120,
    );
    let headers: Vec<usize> = (0..p.len())
        .filter(|&i| p[i].len() == 30 && p[i][2..14] == TAG)
        .collect();
    assert!(headers.len() >= 3, "a header packet per frame");
    assert_eq!(headers[0], 0, "the stream starts with a header");
    for w in headers.windows(2) {
        let data: usize = p[w[0] + 1..w[1]].iter().map(Vec::len).sum();
        assert_eq!(
            data,
            640 * 480 * 2,
            "a whole YUYV frame of data between headers"
        );
        assert_ne!(p[w[0]][1] & HDR_FID, p[w[1]][1] & HDR_FID, "FID toggles");
    }
}

/// Probe quirks: class requests stall until a SET_INTERFACE (XBox Live Vision); SET_CUR(PROBE)
/// stalls with dwMaxVideoFrameSize 0 (UVC_QUIRK_PROBE_EXTRAFIELDS).
#[test]
fn probe_quirks() {
    use emuvc_uvc::stream::{StreamQuirks, StreamingControl};
    let attach = |quirks: StreamQuirks| {
        let mut cfg = synthetic(SyntheticConfig::default());
        cfg.uvc = quirks;
        let host = host();
        let info = host.attach(build(&cfg).unwrap()).unwrap();
        let view = ConfigView::parse(&info.configs[0]).unwrap();
        let iface = parse_function(&view, 0, info.speed).unwrap().streaming[0].interface;
        (host, info.id, iface)
    };
    let probe = |frame_size: u32| {
        StreamingControl {
            hint: 1,
            format_index: 1,
            frame_index: 1,
            frame_interval: 333_333,
            max_video_frame_size: frame_size,
            ..Default::default()
        }
        .to_bytes(48)
    };

    let (host, dev, iface) = attach(StreamQuirks {
        need_set_interface: true,
        ..Default::default()
    });
    assert!(uvc_set(&host, dev, VS_PROBE_CONTROL, iface, &probe(0)).is_err());
    host.control(
        dev,
        SetupPacket::new(0x01, 0x0b, 0, iface as u16, 0), // SET_INTERFACE alt 0
        &[],
        None,
    )
    .unwrap();
    assert!(uvc_set(&host, dev, VS_PROBE_CONTROL, iface, &probe(0)).is_ok());

    let (host, dev, iface) = attach(StreamQuirks {
        strict_probe_fields: true,
        ..Default::default()
    });
    assert!(uvc_set(&host, dev, VS_PROBE_CONTROL, iface, &probe(0)).is_err());
    assert!(uvc_set(&host, dev, VS_PROBE_CONTROL, iface, &probe(614_400)).is_ok());

    // Replies with format/frame index 0; a commit with 0 keeps the probed format and frame.
    let (host, dev, iface) = attach(StreamQuirks {
        zero_index_replies: true,
        ..Default::default()
    });
    let get_cur = |cs: u8| {
        let r = host
            .control(
                dev,
                SetupPacket::new(0xa1, GET_CUR, (cs as u16) << 8, iface as u16, 48),
                &[],
                None,
            )
            .unwrap();
        StreamingControl::from_bytes(&r)
    };
    assert!(uvc_set(&host, dev, VS_PROBE_CONTROL, iface, &probe(0)).is_ok());
    let p = get_cur(VS_PROBE_CONTROL);
    assert_eq!((p.format_index, p.frame_index), (0, 0));
    let zero = StreamingControl {
        frame_interval: 333_333,
        ..p
    };
    assert!(uvc_set(&host, dev, VS_COMMIT_CONTROL, iface, &zero.to_bytes(48)).is_ok());
}

fn vc_req(
    host: &Host,
    dev: DeviceId,
    req: u8,
    entity: u8,
    cs: u8,
    data: &[u8],
    len: u16,
) -> Result<Vec<u8>, UsbError> {
    let rt = if req & 0x80 != 0 { 0xa1 } else { 0x21 };
    let setup = SetupPacket::new(rt, req, (cs as u16) << 8, (entity as u16) << 8, len);
    host.control(dev, setup, data, None)
}

/// `[[device.control]]` reshapes extension unit controls; the power mode control (4.2.1.1).
#[test]
fn xu_config_and_power_mode() {
    let mut cfg = synthetic(SyntheticConfig::default());
    cfg.controls = vec![
        ControlSpec {
            entity: 5,
            selector: 1,
            len: Some(33),
            info: Some(INFO_GET),
            res: Some(vec![]),
            def: Some(vec![7; 33]),
            ..Default::default()
        },
        ControlSpec {
            entity: 5,
            selector: 2,
            def: Some(vec![1, 2]),
            ..Default::default()
        },
    ];
    let host = host();
    let dev = host.attach(build(&cfg).unwrap()).unwrap().id;
    let get = |req, sel, len| vc_req(&host, dev, req, 5, sel, &[], len);
    assert_eq!(get(GET_LEN, 1, 2), Ok(vec![33, 0]));
    assert_eq!(get(GET_INFO, 1, 1), Ok(vec![INFO_GET]));
    assert_eq!(get(GET_CUR, 1, 33), Ok(vec![7; 33]), "starts at GET_DEF");
    assert_eq!(get(GET_MAX, 1, 33), Ok(vec![0xff; 33]), "resized");
    assert!(get(GET_RES, 1, 33).is_err());
    assert!(
        vc_req(&host, dev, SET_CUR, 5, 1, &[0; 33], 33).is_err(),
        "read-only"
    );
    assert_eq!(request_error(&host, dev), ERR_INVALID_REQUEST);
    assert_eq!(get(GET_CUR, 2, 2), Ok(vec![1, 2]));

    // Power mode: USB powered (D5), vendor mode supported (D4), full power.
    let pm = |data: &[u8]| vc_req(&host, dev, SET_CUR, 0, VC_VIDEO_POWER_MODE_CONTROL, data, 1);
    let cur = || vc_req(&host, dev, GET_CUR, 0, VC_VIDEO_POWER_MODE_CONTROL, &[], 1).unwrap()[0];
    assert_eq!(cur(), 0x30);
    assert_eq!(pm(&[1]), Ok(vec![]));
    assert_eq!(cur(), 0x31);
    // libuvc's UVC_VC_VIDEO_POWER_MODE_FULL (0x0b): a reserved mode.
    assert!(pm(&[0x0b]).is_err());
    assert_eq!(request_error(&host, dev), ERR_OUT_OF_RANGE);
    assert_eq!(pm(&[0]), Ok(vec![]));
    assert_eq!(cur(), 0x30);

    // Configured: battery powered, no vendor mode.
    cfg.controls = vec![ControlSpec {
        entity: 0,
        selector: VC_VIDEO_POWER_MODE_CONTROL,
        cur: Some(vec![0x40]),
        ..Default::default()
    }];
    let dev = host.attach(build(&cfg).unwrap()).unwrap().id;
    assert!(vc_req(&host, dev, SET_CUR, 0, VC_VIDEO_POWER_MODE_CONTROL, &[1], 1).is_err());
    // Range overrides on a standard control are refused.
    cfg.controls = vec![ControlSpec {
        entity: 3,
        selector: PU_BRIGHTNESS_CONTROL,
        len: Some(4),
        ..Default::default()
    }];
    assert!(build(&cfg).is_err());
}

#[test]
fn auto_modes_drift() {
    let mut cfg = synthetic(SyntheticConfig::default());
    // Every 100 ms: the status endpoint (polled every 16 ms) keeps up with 4 changes per step.
    cfg.events = vec![EventSpec {
        at_ms: 100,
        every_ms: Some(100),
        count: None,
        kind: EventKind::Auto,
    }];
    let host = host();
    let dev = host.attach(build(&cfg).unwrap()).unwrap().id;
    let ct = |req: u8, cs: u8, v: &[u8], len: u16| {
        let rt = if req == SET_CUR { 0x21 } else { 0xa1 };
        let setup = SetupPacket::new(rt, req, (cs as u16) << 8, 1 << 8, len);
        host.control(dev, setup, v, None).unwrap()
    };
    let exposure = || {
        let v = ct(GET_CUR, CT_EXPOSURE_TIME_ABSOLUTE_CONTROL, &[], 4);
        u32::from_le_bytes(v.try_into().unwrap())
    };
    let status = || {
        let mut u = Urb::interrupt_in(dev, 0x87, 16);
        u.timeout = Some(std::time::Duration::from_millis(500));
        host.transfer(u).data
    };
    let head = |attr: u8| {
        [
            STATUS_ORIGINATOR_VC,
            1,
            0,
            CT_EXPOSURE_TIME_ABSOLUTE_CONTROL,
            attr,
        ]
    };
    let change = |p: &[u8]| {
        (p.len() == 9 && p[..5] == head(STATUS_ATTR_VALUE))
            .then(|| u32::from_le_bytes(p[5..9].try_into().unwrap()))
    };
    // Aperture priority, the default AE mode: the exposure time moves by its GET_RES (1) at every
    // step, and reports it (autoupdate). Auto focus and auto white balance move theirs meanwhile.
    let start = exposure();
    let seen: Vec<u32> = (0..12).filter_map(|_| change(&status())).collect();
    assert!(seen.len() >= 3, "{seen:?}");
    assert!(
        seen[0] == start || seen[0] == start + 1,
        "{seen:?} from {start}"
    );
    assert!(seen.windows(2).all(|w| w[1] == w[0] + 1), "{seen:?}");
    // Manual: the changes queued before, up to the exposure time's info change (no longer
    // disabled), then none. The last one reported is the value.
    ct(SET_CUR, CT_AE_MODE_CONTROL, &[1], 1);
    let mut last = *seen.last().unwrap();
    let mut enabled = false;
    for _ in 0..20 {
        let p = status();
        if let Some(v) = change(&p) {
            last = v;
        } else if p.len() == 6 && p[..5] == head(STATUS_ATTR_INFO) {
            enabled = p[5] & INFO_DISABLED == 0;
            break;
        }
    }
    assert!(enabled);
    assert_eq!(exposure(), last);
    for _ in 0..12 {
        assert_eq!(change(&status()), None);
    }
    assert_eq!(exposure(), last);
}
