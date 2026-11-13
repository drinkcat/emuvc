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
use emuvc_uvc::{EventKind, EventSpec};
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
