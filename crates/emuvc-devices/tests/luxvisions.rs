//! End-to-end tests of the Luxvisions model on a virtual-clock host.

use std::sync::mpsc;

use emuvc_devices::{DeviceConfig, build};
use emuvc_usb::desc::ConfigView;
use emuvc_usb::fault::{Action, FaultRule, Match};
use emuvc_usb::*;
use emuvc_uvc::consts::*;
use emuvc_uvc::host::{Reassembler, probe_commit};
use emuvc_uvc::topology::{Transfer, parse_function};
use emuvc_video::{PixelFormat, stamp};

fn host() -> Host {
    Host::new(HostConfig {
        clock: ClockMode::Virtual,
        ..Default::default()
    })
}

fn attach(host: &Host, cfg: &DeviceConfig) -> std::sync::Arc<DeviceInfo> {
    host.attach(build(cfg).unwrap()).unwrap()
}

fn uvc_get(
    host: &Host,
    dev: DeviceId,
    req: u8,
    entity: u8,
    cs: u8,
    iface: u8,
    len: u16,
) -> Result<Vec<u8>, UsbError> {
    host.control(
        dev,
        SetupPacket::new(
            0xa1,
            req,
            (cs as u16) << 8,
            (entity as u16) << 8 | iface as u16,
            len,
        ),
        &[],
        None,
    )
}

fn uvc_set(
    host: &Host,
    dev: DeviceId,
    entity: u8,
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
            (entity as u16) << 8 | iface as u16,
            data.len() as u16,
        ),
        data,
        None,
    )
}

#[test]
fn enumeration() {
    let host = host();
    let info = attach(&host, &DeviceConfig::model("luxvisions-30c9-0052"));
    assert_eq!(info.product.as_deref(), Some("Integrated RGB Camera"));
    assert_eq!(info.serial.as_deref(), Some("01.00.00"));
    assert_eq!(
        info.configs[0],
        include_bytes!("../../../tests/fixtures/30c9_0052.descriptors.bin")[18..].to_vec()
    );
    assert_eq!(
        info.bos.as_deref(),
        Some(&include_bytes!("../../../tests/fixtures/30c9_0052.bos.bin")[..])
    );
    assert!(info.endpoint(0x87).is_some(), "VC status endpoint");
    assert!(
        info.endpoint(0x81).is_none(),
        "streaming endpoint only in alt > 0"
    );
}

#[test]
fn controls() {
    let host = host();
    let dev = attach(&host, &DeviceConfig::model("luxvisions-30c9-0052")).id;
    // Brightness: PU 2, interface 0.
    assert_eq!(
        uvc_get(&host, dev, GET_CUR, 2, PU_BRIGHTNESS_CONTROL, 0, 2),
        Ok(vec![128, 0])
    );
    assert_eq!(
        uvc_get(&host, dev, GET_MAX, 2, PU_BRIGHTNESS_CONTROL, 0, 2),
        Ok(vec![255, 0])
    );
    assert_eq!(
        uvc_set(
            &host,
            dev,
            2,
            PU_BRIGHTNESS_CONTROL,
            0,
            &300i16.to_le_bytes()
        ),
        Err(UsbError::Stall)
    );
    assert_eq!(
        uvc_get(&host, dev, GET_CUR, 0, VC_REQUEST_ERROR_CODE_CONTROL, 0, 1),
        Ok(vec![ERR_OUT_OF_RANGE])
    );
    uvc_set(
        &host,
        dev,
        2,
        PU_BRIGHTNESS_CONTROL,
        0,
        &200i16.to_le_bytes(),
    )
    .unwrap();
    assert_eq!(
        uvc_get(&host, dev, GET_CUR, 2, PU_BRIGHTNESS_CONTROL, 0, 2),
        Ok(vec![200, 0])
    );
    assert_eq!(
        uvc_get(&host, dev, GET_CUR, 0, VC_REQUEST_ERROR_CODE_CONTROL, 0, 1),
        Ok(vec![ERR_NO_ERROR])
    );
    // Unknown unit / control.
    assert_eq!(
        uvc_get(&host, dev, GET_CUR, 9, 1, 0, 1),
        Err(UsbError::Stall)
    );
    assert_eq!(
        uvc_get(&host, dev, GET_CUR, 0, VC_REQUEST_ERROR_CODE_CONTROL, 0, 1),
        Ok(vec![ERR_INVALID_UNIT])
    );
    // Extension unit sizes measured on the real device.
    assert_eq!(
        uvc_get(&host, dev, GET_LEN, 4, 10, 0, 2),
        Ok(vec![0x00, 0x04])
    );

    // White balance auto disables the temperature; turning it off notifies
    // the host through the status endpoint.
    let info = uvc_get(
        &host,
        dev,
        GET_INFO,
        2,
        PU_WHITE_BALANCE_TEMPERATURE_CONTROL,
        0,
        1,
    )
    .unwrap();
    assert_ne!(info[0] & INFO_DISABLED, 0);
    assert_eq!(
        uvc_set(
            &host,
            dev,
            2,
            PU_WHITE_BALANCE_TEMPERATURE_CONTROL,
            0,
            &5000u16.to_le_bytes()
        ),
        Err(UsbError::Stall)
    );
    uvc_set(
        &host,
        dev,
        2,
        PU_WHITE_BALANCE_TEMPERATURE_AUTO_CONTROL,
        0,
        &[0],
    )
    .unwrap();
    let c = host.transfer(Urb::interrupt_in(dev, 0x87, 16));
    assert_eq!(
        c.data,
        vec![
            STATUS_ORIGINATOR_VC,
            2,
            0,
            PU_WHITE_BALANCE_TEMPERATURE_CONTROL,
            STATUS_ATTR_INFO,
            0x0b
        ]
    );
    uvc_set(
        &host,
        dev,
        2,
        PU_WHITE_BALANCE_TEMPERATURE_CONTROL,
        0,
        &5000u16.to_le_bytes(),
    )
    .unwrap();
    // AE mode: only manual (1) and aperture priority (8).
    assert_eq!(
        uvc_get(&host, dev, GET_RES, 1, CT_AE_MODE_CONTROL, 0, 1),
        Ok(vec![0x09])
    );
    assert_eq!(
        uvc_set(&host, dev, 1, CT_AE_MODE_CONTROL, 0, &[0x02]),
        Err(UsbError::Stall)
    );
}

struct Captured {
    frames: Vec<(Vec<u8>, bool)>,
    commit: emuvc_uvc::stream::StreamingControl,
}

/// Negotiates format/frame on interface 1, picks the alt setting like
/// libuvc and streams until `n` frames are reassembled.
fn stream(host: &Host, info: &DeviceInfo, format: u8, frame: u8, n: usize) -> Captured {
    let dev = info.id;
    let commit = probe_commit(host, dev, 1, format, frame, 333333, 48).unwrap();
    let view = ConfigView::parse(&info.configs[0]).unwrap();
    let uvc = parse_function(&view, 0, Speed::High).unwrap();
    let Transfer::Isochronous { alts } = &uvc.streaming[0].transfer else {
        panic!()
    };
    let alt = alts
        .iter()
        .find(|a| a.bytes_per_interval >= commit.max_payload_transfer_size as usize)
        .unwrap();
    host.control(
        dev,
        SetupPacket::new(0x01, 0x0b, alt.alt as u16, 1, 0),
        &[],
        None,
    )
    .unwrap();
    let (tx, rx) = mpsc::channel();
    let submit = |tx: mpsc::Sender<Completion>| {
        host.submit(
            Urb::iso_in(dev, 0x81, vec![alt.bytes_per_interval; 32]),
            Box::new(move |c| drop(tx.send(c))),
        )
        .unwrap();
    };
    for _ in 0..8 {
        submit(tx.clone());
    }
    let mut r = Reassembler::default();
    while r.frames.len() < n {
        let c = rx.recv().unwrap();
        for p in &c.iso {
            if p.status.is_ok() {
                r.push(&p.data);
            }
        }
        submit(tx.clone());
    }
    host.control(dev, SetupPacket::new(0x01, 0x0b, 0, 1, 0), &[], None)
        .unwrap();
    Captured {
        frames: r.frames,
        commit,
    }
}

fn stamps(frames: &[(Vec<u8>, bool)], fmt: PixelFormat, w: usize, h: usize) -> Vec<Option<u32>> {
    frames
        .iter()
        .map(|(f, _)| stamp::decode_frame(fmt, f, w, h))
        .collect()
}

#[test]
fn stream_yuyv() {
    let host = host();
    let info = attach(&host, &DeviceConfig::model("luxvisions-30c9-0052"));
    // Format 2 = YUY2, frame 5 = 640x480 @ 30.
    let cap = stream(&host, &info, 2, 5, 20);
    assert_eq!(cap.commit.max_video_frame_size, 640 * 480 * 2);
    for (f, err) in &cap.frames {
        assert_eq!(f.len(), 640 * 480 * 2);
        assert!(!err);
    }
    let s: Vec<u32> = stamps(&cap.frames, PixelFormat::Yuyv, 640, 480)
        .into_iter()
        .map(Option::unwrap)
        .collect();
    assert!(
        s.windows(2).all(|w| w[1] == w[0] + 1),
        "frames in order, none dropped: {s:?}"
    );
}

#[test]
fn stream_mjpeg() {
    let host = host();
    let info = attach(
        &host,
        &DeviceConfig {
            strip_dht: true,
            ..DeviceConfig::model("luxvisions-30c9-0052")
        },
    );
    let cap = stream(&host, &info, 1, 9, 10); // 320x240 MJPEG
    for (f, _) in &cap.frames {
        assert_eq!(&f[..2], &[0xff, 0xd8]);
        assert_eq!(&f[f.len() - 2..], &[0xff, 0xd9]);
        assert!(!emuvc_video::jpeg::has_dht(f));
    }
    let s: Vec<u32> = stamps(&cap.frames, PixelFormat::Mjpeg, 320, 240)
        .into_iter()
        .map(Option::unwrap)
        .collect();
    assert!(s.windows(2).all(|w| w[1] == w[0] + 1), "{s:?}");
}

#[test]
fn iso_errors_corrupt_frames() {
    let host = host();
    let mut cfg = DeviceConfig::model("luxvisions-30c9-0052");
    cfg.faults.push(FaultRule {
        on: Match::IsoIn { ep: Some(0x81) },
        action: Action::Error(UsbError::Protocol),
        skip: 0,
        count: None,
        every: Some(500),
        probability: None,
        after_ms: None,
        until_ms: None,
    });
    let info = attach(&host, &cfg);
    let cap = stream(&host, &info, 2, 5, 20);
    let bad = stamps(&cap.frames, PixelFormat::Yuyv, 640, 480)
        .iter()
        .zip(&cap.frames)
        .filter(|(s, (f, _))| s.is_none() || f.len() != 640 * 480 * 2)
        .count();
    assert!(bad > 0, "lost packets must show up as damaged frames");
}

#[test]
fn quirks_change_probe() {
    let host = host();
    let mut cfg = DeviceConfig::model("luxvisions-30c9-0052");
    cfg.uvc.max_payload_transfer_size = Some(0);
    cfg.uvc.probe_len = Some(26);
    let dev = attach(&host, &cfg).id;
    let p = uvc_get(&host, dev, GET_CUR, 0, VS_PROBE_CONTROL, 1, 48).unwrap();
    assert_eq!(p.len(), 26);
    assert_eq!(&p[22..26], &[0, 0, 0, 0]);
}
