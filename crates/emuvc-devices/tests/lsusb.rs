use std::path::PathBuf;

use emuvc_devices::lsusb::{parse, parse_file};
use emuvc_devices::{DeviceConfig, build};
use emuvc_usb::Speed;
use emuvc_usb::desc::ConfigView;
use emuvc_uvc::topology::find_video_control_interfaces;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn truth() -> Vec<u8> {
    std::fs::read(fixtures().join("30c9_0052.descriptors.bin")).unwrap()
}

#[test]
fn round_trip_emulated_webcam() {
    let bin = truth();
    let devs = parse_file(&fixtures().join("lsusb/30c9_0052-emulated.txt")).unwrap();
    assert_eq!(devs.len(), 1);
    let d = &devs[0];
    assert_eq!(d.speed, Some(Speed::High));
    assert_eq!(d.descriptors.device, &bin[..18]);
    assert_eq!(d.descriptors.configs, vec![bin[18..].to_vec()]);
    assert_eq!(
        d.descriptors.bos.as_deref(),
        Some(
            std::fs::read(fixtures().join("30c9_0052.bos.bin"))
                .unwrap()
                .as_slice()
        )
    );
}

#[test]
fn round_trip_real_webcam() {
    let bin = truth();
    let devs = parse_file(&fixtures().join("lsusb/30c9_0052-real.txt")).unwrap();
    let d = &devs[0];
    assert_eq!(d.descriptors.device, &bin[..18]);
    assert_eq!(d.descriptors.configs, vec![bin[18..].to_vec()]);
    assert_eq!(d.descriptors.bos, None);
    assert_eq!(
        d.descriptors.strings.get(&2).map(String::as_str),
        Some("Integrated RGB Camera")
    );
}

fn check_camera(path: PathBuf) {
    let devs = parse_file(&path).unwrap_or_else(|e| panic!("{e}"));
    let d = &devs[0];
    let view = ConfigView::parse(&d.descriptors.configs[0]).unwrap();
    assert!(
        !find_video_control_interfaces(&view).is_empty(),
        "{}: no VideoControl interface",
        path.display()
    );
    let cfg = DeviceConfig {
        descriptors: Some(path.clone()),
        ..DeviceConfig::model("lsusb")
    };
    build(&cfg).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

#[test]
fn libuvc_cameras_parse_and_build() {
    let dir = fixtures().join("lsusb/libuvc-cameras");
    let mut n = 0;
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_none_or(|x| x != "txt") {
            continue;
        }
        if p.file_name().unwrap().to_string_lossy().contains("_ctrls") {
            // V4L2 control listings, not lsusb output.
            assert!(parse_file(&p).is_err());
            continue;
        }
        check_camera(p);
        n += 1;
    }
    assert_eq!(n, 5);
}

#[test]
fn issue_dumps_parse_and_build() {
    let p = fixtures().join("lsusb/issues/67-05a3_8830.txt");
    check_camera(p.clone());
    let d = &parse_file(&p).unwrap()[0];
    // wTotalLength is as printed (897), whatever the bytes add up to.
    assert_eq!(
        u16::from_le_bytes([d.descriptors.configs[0][2], d.descriptors.configs[0][3]]),
        897
    );
}

#[test]
fn multiple_devices() {
    let a = std::fs::read_to_string(fixtures().join("lsusb/30c9_0052-emulated.txt")).unwrap();
    let b = std::fs::read_to_string(fixtures().join("lsusb/30c9_0052-real.txt")).unwrap();
    let devs = parse(&format!("{a}\n{b}")).unwrap();
    assert_eq!(devs.len(), 2);
    assert_eq!((devs[1].bus, devs[1].address), (3, 3));
}
