use emuvc_video::format::{GUID_L8_IR, PixelFormat as F};
use emuvc_video::stamp::{band_height, decode_frame};
use emuvc_video::{FrameSource, FrameSpec, PatternKind, TestPattern};
use std::time::Instant;

const RAW: [F; 4] = [F::Yuyv, F::Uyvy, F::Nv12, F::Grey8];
const VALUES: [u32; 4] = [0, 1, 0xdeadbeef, u32::MAX];

#[test]
fn raw_round_trip() {
    for (w, h) in [(320u16, 240u16), (640, 480), (1920, 1080)] {
        for f in RAW {
            let spec = FrameSpec {
                format: f,
                width: w,
                height: h,
            };
            let mut p = TestPattern::default();
            for v in VALUES {
                let data = p.render(&spec, v as u64, 0);
                assert_eq!(data.len(), f.frame_size(w, h).unwrap());
                assert_eq!(
                    decode_frame(f, &data, w as usize, h as usize),
                    Some(v),
                    "{f:?} {w}x{h}"
                );
            }
        }
    }
}

#[test]
fn corrupted_band_fails() {
    let spec = FrameSpec {
        format: F::Grey8,
        width: 320,
        height: 240,
    };
    let mut data = TestPattern::default().render(&spec, 12345, 0);
    let band = band_height(240);
    for y in 0..band {
        for x in 0..320 {
            data[y * 320 + x] = if (x / 3 + y) % 2 == 0 { 16 } else { 235 };
        }
    }
    assert_eq!(decode_frame(F::Grey8, &data, 320, 240), None);
    // Flipping a single payload cell breaks the CRC.
    let mut data = TestPattern::default().render(&spec, 12345, 0);
    let (x0, x1) = (10 * 320 / 42, 11 * 320 / 42);
    for y in 0..band {
        for x in x0..x1 {
            data[y * 320 + x] = 255 - data[y * 320 + x];
        }
    }
    assert_eq!(decode_frame(F::Grey8, &data, 320, 240), None);
}

#[cfg(feature = "decode")]
#[test]
fn mjpeg_round_trip() {
    for (w, h) in [(320u16, 240u16), (1280, 720)] {
        for q in [50, 85] {
            for strip in [false, true] {
                let mut p = TestPattern::default()
                    .with_jpeg_quality(q)
                    .with_strip_dht(strip);
                let spec = FrameSpec {
                    format: F::Mjpeg,
                    width: w,
                    height: h,
                };
                for v in VALUES {
                    let d = p.render(&spec, v as u64, 0);
                    assert_eq!(&d[..2], &[0xff, 0xd8]);
                    assert_eq!(&d[d.len() - 2..], &[0xff, 0xd9]);
                    assert_eq!(emuvc_video::jpeg::has_dht(&d), !strip);
                    assert_eq!(
                        decode_frame(F::Mjpeg, &d, w as usize, h as usize),
                        Some(v),
                        "q{q} {w}x{h} strip={strip}"
                    );
                }
            }
        }
    }
}

#[test]
fn guids_and_sizes() {
    for f in RAW {
        assert_eq!(F::from_guid(&f.guid().unwrap()), Some(f));
        assert!(f.bits_per_pixel().is_some());
    }
    assert_eq!(&F::Yuyv.guid().unwrap()[..4], b"YUY2");
    assert_eq!(F::from_guid(&GUID_L8_IR), Some(F::Grey8));
    assert_eq!(F::Mjpeg.guid(), None);
    assert_eq!(F::Nv12.frame_size(640, 480), Some(460800));
    assert_eq!(F::Mjpeg.frame_size(640, 480), None);
}

#[test]
fn frames_differ_and_odd_sizes_ok() {
    for kind in [
        PatternKind::ColorBars,
        PatternKind::Gradient,
        PatternKind::Checkerboard,
        PatternKind::Solid { r: 9, g: 8, b: 7 },
    ] {
        for f in [F::Yuyv, F::Uyvy, F::Nv12, F::Grey8, F::Mjpeg] {
            for (w, h) in [(320u16, 240u16), (321, 243)] {
                let spec = FrameSpec {
                    format: f,
                    width: w,
                    height: h,
                };
                let mut p = TestPattern::new(kind);
                let a = p.render(&spec, 0, 0);
                let b = p.render(&spec, 1, 0);
                assert_ne!(a, b, "{kind:?} {f:?}");
            }
        }
    }
}

#[test]
#[ignore]
fn timings() {
    for f in [F::Yuyv, F::Mjpeg] {
        let spec = FrameSpec {
            format: f,
            width: 1920,
            height: 1080,
        };
        let mut p = TestPattern::default();
        p.render(&spec, 0, 0);
        let n = 20;
        let t = Instant::now();
        let mut len = 0;
        for i in 1..=n {
            len = p.render(&spec, i, 0).len();
        }
        println!(
            "{f:?} 1080p: {:?}/frame ({len} bytes)",
            t.elapsed() / n as u32
        );
    }
}
