//! Descriptors of a Luxvisions Innotech integrated RGB + IR webcam
//! (`30c9:0052`), as seen on a laptop at high speed: two UVC 1.5 functions
//! (RGB on interfaces 0+1, IR on 2+3) and a DFU interface (4).
//!
//! The output is byte-identical to `tests/fixtures/30c9_0052.descriptors.bin`.

use emuvc_usb::desc::ConfigView;
use emuvc_usb::{CompositeDevice, Descriptors, Speed};
use emuvc_uvc::UvcFunction;
use emuvc_uvc::control::{Control, region_of_interest};
use emuvc_uvc::topology::{find_video_control_interfaces, parse_function};

use crate::config::DeviceConfig;
use crate::dfu::DfuRuntime;
use emuvc_usb::desc::{
    ConfigBuilder, DeviceDescriptor, EndpointDescriptor, IadDescriptor, InterfaceDescriptor,
};
use emuvc_uvc::consts::*;
use emuvc_uvc::desc::*;

const STR_PRODUCT_RGB: u8 = 2;
const STR_FUNCTION_RGB: u8 = 4;
const STR_FUNCTION_IR: u8 = 11;
const STR_DFU: u8 = 12;

const YUY2: &str = "{32595559-0000-0010-8000-00aa00389b71}";
const L8_IR: &str = "{00000032-0002-0010-8000-00aa00389b71}";

/// `(width, height, frames per second, 100 ns interval)` of the RGB frame
/// sizes, used for both MJPEG and YUY2. YUY2 runs slower at large sizes
/// (USB 2.0 bandwidth), MJPEG always at 30 fps.
const RGB_SIZES: [(u16, u16, u32, u32); 10] = [
    (1920, 1080, 5, 2_000_000),
    (1280, 720, 10, 1_000_000),
    (960, 540, 15, 666_666),
    (848, 480, 20, 500_000),
    (640, 480, 30, 333_333),
    (640, 360, 30, 333_333),
    (424, 240, 30, 333_333),
    (352, 288, 30, 333_333),
    (320, 240, 30, 333_333),
    (320, 180, 30, 333_333),
];

/// `wMaxPacketSize` (size plus high-bandwidth multiplier bits) of the RGB
/// isochronous alternate settings 1..=11.
const RGB_ISO_ALTS: [u16; 11] = [
    0x00c0, 0x0180, 0x0200, 0x0280, 0x0320, 0x03b0, 0x0a80, 0x0b20, 0x0be0, 0x13c0, 0x13fc,
];

/// Frame descriptor with a single discrete interval.
fn frame(w: u16, h: u16, bits_per_pixel: u32, fps: u32, interval: u32) -> VsFrame {
    let bit_rate = w as u32 * h as u32 * bits_per_pixel * fps;
    VsFrame {
        capabilities: 0x01, // still image supported
        width: w,
        height: h,
        min_bit_rate: bit_rate,
        max_bit_rate: bit_rate,
        max_frame_size: w as u32 * h as u32 * bits_per_pixel / 8,
        bytes_per_line: 0,
        default_interval: interval,
        intervals: FrameIntervals::Discrete(vec![interval]),
    }
}

fn format(kind: VsFormatKind, frames: Vec<VsFrame>) -> VsFormat {
    VsFormat {
        kind,
        default_frame: 1,
        aspect_x: 0,
        aspect_y: 0,
        interlace_flags: 0,
        copy_protect: 0,
        frames,
    }
}

const BT709_601: ColorMatching = ColorMatching {
    primaries: 1,
    transfer: 1,
    matrix: 4,
};

fn camera(controls: u32) -> VcEntity {
    VcEntity::Camera(CameraTerminal {
        id: 1,
        assoc_terminal: 0,
        i_terminal: 0,
        objective_focal_length_min: 0,
        objective_focal_length_max: 0,
        ocular_focal_length: 0,
        control_size: 3,
        controls,
    })
}

fn processing(controls: u32) -> VcEntity {
    VcEntity::Processing(ProcessingUnit {
        id: 2,
        source: 1,
        max_multiplier: 16384,
        control_size: 3,
        controls,
        i_processing: 0,
        video_standards: Some(0),
    })
}

fn xu(id: u8, guid_str: &str, num_controls: u8, source: u8, controls: [u8; 4]) -> VcEntity {
    VcEntity::Extension(ExtensionUnit {
        id,
        guid: guid(guid_str),
        num_controls,
        sources: vec![source],
        control_size: 4,
        controls: controls.to_vec(),
        i_extension: 0,
    })
}

fn output(source: u8) -> VcEntity {
    VcEntity::Output(OutputTerminal {
        id: 6,
        terminal_type: TT_STREAMING,
        assoc_terminal: 0,
        source,
        i_terminal: 0,
        extra: vec![],
    })
}

fn interface(
    number: u8,
    alt: u8,
    num_endpoints: u8,
    subclass: u8,
    i_interface: u8,
) -> InterfaceDescriptor {
    InterfaceDescriptor {
        number,
        alt,
        num_endpoints,
        class: CC_VIDEO,
        subclass,
        protocol: PC_PROTOCOL_15,
        i_interface,
    }
}

/// VideoControl interface with its entities and interrupt endpoint.
fn video_control(
    cfg: &mut ConfigBuilder,
    number: u8,
    i_interface: u8,
    vs: u8,
    entities: &[VcEntity],
    int_ep: u8,
) {
    cfg.interface(&interface(number, 0, 1, SC_VIDEOCONTROL, i_interface));
    let header = VcHeader {
        bcd_uvc: 0x0150,
        clock_frequency: 48_000_000,
        streaming_interfaces: vec![vs],
    };
    cfg.raw(&vc_block(&header, entities));
    cfg.endpoint(&EndpointDescriptor {
        address: int_ep,
        attributes: 3,
        max_packet_size: 16,
        interval: 8,
    });
    cfg.raw(&vc_interrupt_endpoint(1029));
}

/// Streaming interface: alt 0 (no endpoint) with the class-specific
/// descriptors, then one isochronous alt setting per `wMaxPacketSize`.
fn video_streaming(
    cfg: &mut ConfigBuilder,
    number: u8,
    ep: u8,
    header: &VsInputHeader,
    formats: &[VsFormatEntry],
    alts: &[u16],
) {
    cfg.interface(&interface(number, 0, 0, SC_VIDEOSTREAMING, 0));
    cfg.raw(&vs_block(header, formats));
    for (i, &mps) in alts.iter().enumerate() {
        cfg.interface(&interface(number, i as u8 + 1, 1, SC_VIDEOSTREAMING, 0));
        cfg.endpoint(&EndpointDescriptor {
            address: ep,
            attributes: 5,
            max_packet_size: mps,
            interval: 1,
        });
    }
}

/// RGB function: interfaces 0 (control) and 1 (streaming, MJPEG + YUY2).
fn rgb_function(cfg: &mut ConfigBuilder) {
    cfg.iad(&IadDescriptor {
        first_interface: 0,
        interface_count: 2,
        function_class: CC_VIDEO,
        function_subclass: SC_VIDEO_INTERFACE_COLLECTION,
        function_protocol: 0,
        i_function: STR_FUNCTION_RGB,
    });
    let entities = [
        // AE mode, AE priority, exposure (abs), privacy, region of interest.
        camera(0x0024_000e),
        // Brightness, contrast, hue, saturation, sharpness, gamma, WB temp,
        // backlight comp, power line frequency, WB temp auto.
        processing(0x0000_157f),
        xu(
            3,
            "{26b8105a-0713-4870-979d-da79444bb68e}",
            2,
            2,
            [0x04, 0, 0, 0],
        ),
        xu(
            4,
            "{63610682-5070-49ab-b8cc-b3855e8d221d}",
            26,
            3,
            [0xff, 0xff, 0x71, 0x47],
        ),
        xu(
            5,
            "{0f3f95dc-2632-4c4e-92c9-a04782f43bc8}",
            3,
            4,
            [0x06, 0x01, 0, 0],
        ),
        output(5),
    ];
    video_control(cfg, 0, STR_FUNCTION_RGB, 1, &entities, 0x87);

    let mjpeg = RGB_SIZES
        .iter()
        .map(|&(w, h, _, _)| frame(w, h, 16, 30, 333_333))
        .collect();
    let yuy2 = RGB_SIZES
        .iter()
        .map(|&(w, h, fps, iv)| frame(w, h, 16, fps, iv))
        .collect();
    let formats = [
        VsFormatEntry::new(format(VsFormatKind::Mjpeg { flags: 1 }, mjpeg)).with_color(BT709_601),
        VsFormatEntry::new(format(
            VsFormatKind::Uncompressed {
                guid: guid(YUY2),
                bits_per_pixel: 16,
            },
            yuy2,
        ))
        .with_color(BT709_601),
    ];
    let header = VsInputHeader {
        endpoint: 0x81,
        info: 0,
        terminal_link: 6,
        still_capture_method: 1,
        trigger_support: 0,
        trigger_usage: 0,
        control_size: 1,
        format_controls: vec![0x04, 0x00],
    };
    video_streaming(cfg, 1, 0x81, &header, &formats, &RGB_ISO_ALTS);
}

/// IR function: interfaces 2 (control) and 3 (streaming, 8-bit IR).
fn ir_function(cfg: &mut ConfigBuilder) {
    cfg.iad(&IadDescriptor {
        first_interface: 2,
        interface_count: 2,
        function_class: CC_VIDEO,
        function_subclass: SC_VIDEO_INTERFACE_COLLECTION,
        function_protocol: 0,
        i_function: STR_FUNCTION_IR,
    });
    let entities = [
        // Privacy and region of interest only.
        camera(0x0024_0000),
        processing(0),
        xu(
            7,
            "{0f3f95dc-2632-4c4e-92c9-a04782f43bc8}",
            6,
            2,
            [0xe6, 0x01, 0, 0],
        ),
        xu(
            4,
            "{63610682-5070-49ab-b8cc-b3855e8d221d}",
            25,
            7,
            [0xff, 0xff, 0x71, 0x07],
        ),
        xu(
            5,
            "{52182091-70d0-4ff5-acbc-0d04cc5f5a64}",
            2,
            4,
            [0x0a, 0, 0, 0],
        ),
        output(5),
    ];
    video_control(cfg, 2, STR_FUNCTION_IR, 3, &entities, 0x84);

    // Two 640x360 modes: 15 fps and 30 fps (the latter with twice the bit rate).
    // The buffer size is that of 16 bits per pixel, as reported by the device.
    let mut frames = vec![
        frame(640, 360, 8, 15, 666_666),
        frame(640, 360, 8, 30, 333_333),
    ];
    for f in &mut frames {
        f.max_frame_size = 460_800;
    }
    let formats = [VsFormatEntry::new(format(
        VsFormatKind::Uncompressed {
            guid: guid(L8_IR),
            bits_per_pixel: 8,
        },
        frames,
    ))
    .with_color(BT709_601)];
    let header = VsInputHeader {
        endpoint: 0x82,
        info: 0,
        terminal_link: 6,
        still_capture_method: 1,
        trigger_support: 0,
        trigger_usage: 0,
        control_size: 1,
        format_controls: vec![0x00],
    };
    video_streaming(cfg, 3, 0x82, &header, &formats, &[0x0a80]);
}

/// DFU (application mode) interface 4.
fn dfu_interface(cfg: &mut ConfigBuilder) {
    cfg.interface(&InterfaceDescriptor {
        number: 4,
        alt: 0,
        num_endpoints: 0,
        class: 0xfe,
        subclass: 1,
        protocol: 1,
        i_interface: STR_DFU,
    });
    // DFU functional descriptor: bmAttributes 0x0f, wDetachTimeOut 2000 ms,
    // wTransferSize 4096, bcdDFUVersion 1.10.
    cfg.raw(&[9, 0x21, 0x0f, 0xd0, 0x07, 0x00, 0x10, 0x10, 0x01]);
}

/// Full descriptor set of the device (high speed).
/// BOS from the real device: a Microsoft OS 2.0 platform capability
/// (vendor code 1, 1248-byte descriptor set) and a firmware status capability.
const BOS: [u8; 41] = [
    0x05, 0x0f, 0x29, 0x00, 0x02, //
    0x1c, 0x10, 0x05, 0x00, 0xdf, 0x60, 0xdd, 0xd8, 0x89, 0x45, 0xc7, 0x4c, 0x9c, 0xd2, 0x65, 0x9d,
    0x9e, 0x64, 0x8a, 0x9f, 0x00, 0x00, 0x03, 0x06, 0xe0, 0x04, 0x01, 0x00, //
    0x08, 0x10, 0x11, 0x01, 0x03, 0x00, 0x00, 0x00,
];

pub fn descriptors() -> Descriptors {
    let device = DeviceDescriptor {
        bcd_usb: 0x0201,
        class: 0xef, // miscellaneous, IAD
        subclass: 2,
        protocol: 1,
        max_packet_size0: 64,
        vendor_id: 0x30c9,
        product_id: 0x0052,
        bcd_device: 0x1109,
        i_manufacturer: 1,
        i_product: STR_PRODUCT_RGB,
        i_serial: 3,
        num_configurations: 1,
    };
    let mut cfg = ConfigBuilder::new(1).max_power(250);
    rgb_function(&mut cfg);
    ir_function(&mut cfg);
    dfu_interface(&mut cfg);

    Descriptors {
        device: device.to_bytes(),
        configs: vec![cfg.build()],
        bos: Some(BOS.to_vec()),
        langids: vec![0x0409],
        strings: [
            (1, "8SSC21D67438V1SR0000000A"),
            (2, "Integrated RGB Camera"),
            (3, "01.00.00"),
            (4, "Integrated Camera"),
            (11, "Integrated IR Camera"),
            (12, "APP Mode"),
        ]
        .into_iter()
        .map(|(i, s)| (i, s.to_string()))
        .collect(),
        ..Default::default()
    }
}

pub const NAME: &str = "luxvisions-30c9-0052";
pub const DESCRIPTION: &str =
    "Luxvisions Integrated RGB+IR laptop camera (UVC 1.5, MJPEG/YUY2 + L8 IR, isochronous)";

/// The emulated camera: two UVC functions (RGB on interfaces 0-1, IR on
/// 2-3) and a DFU runtime interface, with control ranges and extension unit
/// sizes measured on the real device.
pub fn build(cfg: &DeviceConfig) -> Result<CompositeDevice, String> {
    let mut desc = descriptors();
    cfg.apply_strings(&mut desc);
    let view = ConfigView::parse(&desc.configs[0]).map_err(|e| e.to_string())?;
    let mut dev = CompositeDevice::new(NAME, Speed::High, desc);
    for vc in find_video_control_interfaces(&view) {
        let f = parse_function(&view, vc, Speed::High).map_err(|e| e.to_string())?;
        let mut uvc =
            UvcFunction::new(f, Speed::High, cfg.uvc.clone(), &mut |_| cfg.frame_source());
        if vc == 0 {
            tune_rgb(&mut uvc)
        } else {
            tune_ir(&mut uvc)
        }
        dev.add_function(&uvc.interfaces(), Box::new(uvc));
    }
    dev.add_function(&[4], Box::new(DfuRuntime));
    Ok(dev)
}

const CT: u8 = 1;
const PU: u8 = 2;

/// Read-only privacy switch, reported through status interrupts.
fn privacy() -> Control {
    Control::switch(0, 1)
        .only(false, false, false, false)
        .with_info(INFO_GET | INFO_AUTOUPDATE)
}

/// Extension unit controls: (unit, selector, GET_LEN, GET_INFO) as reported by the device.
fn xu_controls(uvc: &mut UvcFunction, table: &[(u8, u8, u16, u8)]) {
    for &(unit, sel, len, info) in table {
        uvc.set_control(unit, sel, Control::raw(len).with_info(info));
    }
}

fn tune_rgb(uvc: &mut UvcFunction) {
    uvc.set_control(
        PU,
        PU_BRIGHTNESS_CONTROL,
        Control::int(2, true, 0, 255, 1, 128),
    );
    uvc.set_control(
        PU,
        PU_CONTRAST_CONTROL,
        Control::int(2, false, 0, 100, 1, 32),
    );
    uvc.set_control(PU, PU_HUE_CONTROL, Control::int(2, true, -180, 180, 1, 0));
    uvc.set_control(
        PU,
        PU_SATURATION_CONTROL,
        Control::int(2, false, 0, 100, 1, 64),
    );
    uvc.set_control(PU, PU_SHARPNESS_CONTROL, Control::int(2, false, 0, 7, 1, 3));
    uvc.set_control(
        PU,
        PU_GAMMA_CONTROL,
        Control::int(2, false, 90, 150, 1, 120),
    );
    uvc.set_control(
        PU,
        PU_WHITE_BALANCE_TEMPERATURE_CONTROL,
        Control::int(2, false, 2800, 6500, 1, 4600)
            .with_info(INFO_GET | INFO_SET | INFO_AUTOUPDATE),
    );
    uvc.set_control(
        PU,
        PU_BACKLIGHT_COMPENSATION_CONTROL,
        Control::int(2, false, 0, 2, 1, 1),
    );
    uvc.set_control(
        CT,
        CT_EXPOSURE_TIME_ABSOLUTE_CONTROL,
        Control::int(4, false, 2, 1250, 1, 156).with_info(INFO_GET | INFO_SET | INFO_AUTOUPDATE),
    );
    uvc.set_control(CT, CT_PRIVACY_CONTROL, privacy());
    xu_controls(
        uvc,
        &[
            (3, 3, 2, 0x0f),
            (4, 1, 1, 0x01),
            (4, 2, 2, 0x03),
            (4, 3, 1, 0x03),
            (4, 4, 2, 0x03),
            (4, 5, 2, 0x03),
            (4, 6, 1, 0x03),
            (4, 7, 1, 0x03),
            (4, 8, 1, 0x03),
            (4, 9, 1, 0x03),
            (4, 10, 1024, 0x03),
            (4, 11, 1, 0x01),
            (4, 12, 1, 0x03),
            (4, 13, 4, 0x03),
            (4, 14, 4, 0x03),
            (4, 15, 1024, 0x03),
            (4, 16, 1, 0x03),
            (4, 17, 64, 0x03),
            (4, 21, 1, 0x03),
            (4, 22, 64, 0x03),
            (4, 23, 3, 0x03),
            (4, 25, 1, 0x03),
            (4, 26, 4, 0x03),
            (5, 2, 15, 0x13),
            (5, 3, 11, 0x13),
            (5, 9, 4, 0x03),
        ],
    );
}

fn tune_ir(uvc: &mut UvcFunction) {
    uvc.set_control(CT, CT_PRIVACY_CONTROL, privacy());
    // The IR function reports the RGB sensor's ROI rectangle.
    uvc.set_control(
        CT,
        CT_REGION_OF_INTEREST_CONTROL,
        region_of_interest(1920, 1080),
    );
    xu_controls(
        uvc,
        &[
            (7, 2, 15, 0x13),
            (7, 3, 11, 0x13),
            (7, 6, 9, 0x03),
            (7, 7, 53, 0x01),
            (7, 8, 53, 0x01),
            (7, 9, 4, 0x03),
            (4, 1, 1, 0x01),
            (4, 2, 2, 0x03),
            (4, 3, 1, 0x03),
            (4, 4, 2, 0x03),
            (4, 5, 2, 0x03),
            (4, 6, 1, 0x03),
            (4, 7, 1, 0x03),
            (4, 8, 1, 0x03),
            (4, 9, 1, 0x03),
            (4, 10, 1024, 0x03),
            (4, 11, 1, 0x01),
            (4, 12, 1, 0x03),
            (4, 13, 4, 0x03),
            (4, 14, 4, 0x03),
            (4, 15, 1024, 0x03),
            (4, 16, 1, 0x03),
            (4, 17, 64, 0x03),
            (4, 21, 1, 0x03),
            (4, 22, 64, 0x03),
            (4, 23, 3, 0x03),
            (4, 25, 1, 0x03),
            (5, 2, 1, 0x03),
            (5, 4, 2, 0x03),
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use emuvc_usb::Speed;
    use emuvc_usb::desc::ConfigView;
    use emuvc_uvc::topology::*;

    const FIXTURE: &[u8] = include_bytes!("../../../tests/fixtures/30c9_0052.descriptors.bin");

    #[test]
    fn matches_fixture() {
        let d = descriptors();
        assert_eq!(d.device, FIXTURE[..18]);
        assert_eq!(d.configs.len(), 1);
        assert_eq!(d.configs[0].len(), 0x0543);
        assert_eq!(d.configs[0], FIXTURE[18..]);
    }

    fn func(vc: u8) -> UvcFunctionDesc {
        let cfg = ConfigView::parse(&descriptors().configs[0]).unwrap();
        assert_eq!(find_video_control_interfaces(&cfg), vec![0, 2]);
        parse_function(&cfg, vc, Speed::High).unwrap()
    }

    #[test]
    fn rgb_function_model() {
        let f = func(0);
        assert_eq!(f.vc.bcd_uvc, 0x0150);
        assert_eq!(f.vc.clock_frequency, 48_000_000);
        assert_eq!(f.vc.streaming_interfaces, vec![1]);
        let st = f.vc.status_endpoint.clone().unwrap();
        assert_eq!(
            (st.address, st.max_packet, st.interval_uframes),
            (0x87, 16, 128)
        );
        let ids: Vec<u8> = f.vc.entities.iter().map(|e| e.id).collect();
        assert_eq!(ids, [1, 2, 3, 4, 5, 6]);
        let ct = f.vc.entity(1).unwrap();
        assert_eq!(ct.kind, EntityKind::Camera);
        assert_eq!(ct.controls, vec![0x02, 0x03, 0x04, 0x11, 0x14]);
        let pu = f.vc.entity(2).unwrap();
        assert_eq!(
            pu.controls,
            vec![0x02, 0x03, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x01, 0x05, 0x0b]
        );
        assert_eq!(pu.sources, vec![1]);
        assert_eq!(f.vc.entity(3).unwrap().controls, vec![3]);
        assert_eq!(f.vc.entity(4).unwrap().sources, vec![3]);
        // bNumControls says 26 but only 24 bits are set in bmControls (device quirk).
        assert_eq!(f.vc.entity(4).unwrap().controls.len(), 24);
        assert_eq!(
            f.vc.entity(6).unwrap().kind,
            EntityKind::OutputTerminal {
                terminal_type: TT_STREAMING
            }
        );

        let vs = &f.streaming[0];
        assert_eq!(
            (
                vs.interface,
                vs.endpoint,
                vs.terminal_link,
                vs.still_capture_method
            ),
            (1, 0x81, 6, 1)
        );
        assert_eq!(vs.formats.len(), 2);
        let mjpeg = vs.format(1).unwrap();
        assert_eq!(mjpeg.kind, FormatKind::Mjpeg);
        assert_eq!(mjpeg.color, Some((1, 1, 4)));
        assert_eq!(mjpeg.frames.len(), 10);
        let f1 = mjpeg.frame(1).unwrap();
        assert_eq!(
            (f1.width, f1.height, f1.max_frame_size),
            (1920, 1080, 4_147_200)
        );
        assert_eq!(f1.intervals, Intervals::Discrete(vec![333_333]));
        assert_eq!(f1.max_bit_rate, 995_328_000);
        let last = mjpeg.frame(10).unwrap();
        assert_eq!((last.width, last.height), (320, 180));
        let yuy2 = vs.format(2).unwrap();
        assert!(
            matches!(yuy2.kind, FormatKind::Uncompressed { bits_per_pixel: 16, guid } if guid == emuvc_uvc::desc::guid("{32595559-0000-0010-8000-00aa00389b71}"))
        );
        assert_eq!(yuy2.frames.len(), 10);
        assert_eq!(yuy2.frame(1).unwrap().default_interval, 2_000_000);
        assert_eq!(yuy2.frame(1).unwrap().nearest_interval(333_333), 2_000_000);
        assert_eq!(yuy2.frame(3).unwrap().max_bit_rate, 124_416_000);

        let Transfer::Isochronous { alts } = &vs.transfer else {
            panic!("not isochronous")
        };
        assert_eq!(alts.len(), 11);
        assert_eq!(
            alts[0],
            IsoAlt {
                alt: 1,
                bytes_per_interval: 192,
                interval_uframes: 1
            }
        );
        assert_eq!(
            alts[10],
            IsoAlt {
                alt: 11,
                bytes_per_interval: 3 * 1020,
                interval_uframes: 1
            }
        );
    }

    #[test]
    fn ir_function_model() {
        let f = func(2);
        assert_eq!(f.vc.streaming_interfaces, vec![3]);
        let st = f.vc.status_endpoint.clone().unwrap();
        assert_eq!(st.address, 0x84);
        let ids: Vec<u8> = f.vc.entities.iter().map(|e| e.id).collect();
        assert_eq!(ids, [1, 2, 7, 4, 5, 6]);
        assert_eq!(f.vc.entity(1).unwrap().controls, vec![0x11, 0x14]);
        assert!(f.vc.entity(2).unwrap().controls.is_empty());
        assert_eq!(f.vc.entity(7).unwrap().sources, vec![2]);
        assert_eq!(f.vc.entity(4).unwrap().sources, vec![7]);
        assert_eq!(f.vc.entity(5).unwrap().controls, vec![2, 4]);

        let vs = &f.streaming[0];
        assert_eq!((vs.interface, vs.endpoint), (3, 0x82));
        assert_eq!(vs.formats.len(), 1);
        let fmt = &vs.formats[0];
        assert!(matches!(
            fmt.kind,
            FormatKind::Uncompressed {
                bits_per_pixel: 8,
                ..
            }
        ));
        assert_eq!(fmt.frames.len(), 2);
        assert_eq!((fmt.frames[0].width, fmt.frames[0].height), (640, 360));
        assert_eq!(fmt.frames[1].default_interval, 333_333);
        let Transfer::Isochronous { alts } = &vs.transfer else {
            panic!("not isochronous")
        };
        assert_eq!(
            alts,
            &[IsoAlt {
                alt: 1,
                bytes_per_interval: 2 * 640,
                interval_uframes: 1
            }]
        );
    }

    #[test]
    fn strings() {
        let d = descriptors();
        assert_eq!(d.strings[&2], "Integrated RGB Camera");
        assert_eq!(d.strings.len(), 6);
    }
}
