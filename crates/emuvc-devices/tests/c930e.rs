//! Logitech C930e model: descriptors from the lsusb fixture, two configurations
//! with their own UVC functions.

use emuvc_devices::logitech_046d_0843::{DUMP, NAME, NAME_UVC15};
use emuvc_devices::{DeviceConfig, build, lsusb};
use emuvc_usb::desc::ConfigView;
use emuvc_usb::*;
use emuvc_uvc::consts::*;
use emuvc_uvc::topology::{EntityKind, find_video_control_interfaces, parse_function};

fn host() -> Host {
    Host::new(HostConfig {
        clock: ClockMode::Virtual,
        ..Default::default()
    })
}

/// bcdUVC of the configuration's VideoControl interface, and whether it has an encoding unit.
fn uvc_of(config: &[u8]) -> (u16, bool) {
    let view = ConfigView::parse(config).unwrap();
    let vc = find_video_control_interfaces(&view)[0];
    let f = parse_function(&view, vc, Speed::High).unwrap();
    let eu = f.vc.entities.iter().any(|e| e.kind == EntityKind::Encoding);
    (f.vc.bcd_uvc, eu)
}

fn get_info(host: &Host, dev: DeviceId, entity: u8, cs: u8) -> Result<Vec<u8>, UsbError> {
    host.control(
        dev,
        SetupPacket::new(0xa1, GET_INFO, (cs as u16) << 8, (entity as u16) << 8, 1),
        &[],
        None,
    )
}

fn set_configuration(host: &Host, dev: DeviceId, value: u8) {
    host.control(
        dev,
        SetupPacket::new(0x00, 0x09, value as u16, 0, 0),
        &[],
        None,
    )
    .unwrap();
}

#[test]
fn fixture_is_the_lsusb_text_plus_vc_interrupt_endpoints() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/lsusb/046d_0843.txt"
    );
    let devices = lsusb::parse_file(path.as_ref()).unwrap();
    let raw = devices[0].raw_dump();
    // lsusb didn't print the class-specific VC interrupt endpoint descriptors.
    let ep = [0x07, 0x05, 0x86, 0x03, 0x40, 0x00, 0x08];
    let mut expected = Vec::new();
    let mut rest = &raw[..];
    while let Some(i) = rest.windows(ep.len()).position(|w| w == ep) {
        expected.extend_from_slice(&rest[..i + ep.len()]);
        expected.extend_from_slice(&[0x05, 0x25, 0x03, 0x40, 0x00]);
        rest = &rest[i + ep.len()..];
    }
    expected.extend_from_slice(rest);
    assert_eq!(expected, DUMP);
    // And now every configuration is exactly as long as its wTotalLength says.
    let (_, configs) = emuvc_devices::generic::split_dump(DUMP).unwrap();
    assert_eq!(configs.iter().map(Vec::len).sum::<usize>(), DUMP.len() - 18);
}

#[test]
fn configurations() {
    let host = host();
    let real = host
        .attach(build(&DeviceConfig::model(NAME)).unwrap())
        .unwrap();
    assert_eq!(real.product.as_deref(), Some("Logitech Webcam C930e"));
    assert_eq!(uvc_of(&real.configs[0]), (0x0100, false));
    assert_eq!(uvc_of(&real.configs[1]), (0x0150, true));
    let uvc15 = host
        .attach(build(&DeviceConfig::model(NAME_UVC15)).unwrap())
        .unwrap();
    assert_eq!(uvc15.device_descriptor[17], 1, "bNumConfigurations");
    assert_eq!(uvc15.configs.len(), 1);
    assert_eq!(uvc_of(&uvc15.configs[0]), (0x0150, true));
    assert_eq!(uvc15.configs[0][5], 1, "bConfigurationValue");
}

#[test]
fn each_configuration_has_its_own_functions() {
    let host = host();
    let dev = host
        .attach(build(&DeviceConfig::model(NAME)).unwrap())
        .unwrap()
        .id;
    let probe_info = |iface: u8| {
        host.control(
            dev,
            SetupPacket::new(
                0xa1,
                GET_INFO,
                (VS_PROBE_CONTROL as u16) << 8,
                iface as u16,
                1,
            ),
            &[],
            None,
        )
    };
    // Configuration 1 (UVC 1.0): interface 2 is the audio control interface.
    assert_eq!(probe_info(2), Err(UsbError::Stall));
    assert_eq!(probe_info(1), Ok(vec![0x03]));
    // Configuration 2 (UVC 1.5): interface 2 is the H.264 VideoStreaming interface.
    set_configuration(&host, dev, 2);
    assert_eq!(probe_info(2), Ok(vec![0x03]));
    assert!(get_info(&host, dev, 3, PU_BRIGHTNESS_CONTROL).is_ok());
    set_configuration(&host, dev, 1);
    assert_eq!(probe_info(2), Err(UsbError::Stall));
}
