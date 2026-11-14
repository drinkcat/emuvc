//! A configurable synthetic UVC camera, to exercise as much of the
//! specification (and of the host's code) as possible: every camera and
//! processing unit control, selector/encoding/extension units, all pixel
//! formats emuvc can generate, still image capture methods 1-3, several
//! streaming interfaces, isochronous or bulk, full/high/SuperSpeed, UVC
//! 1.0/1.1/1.5. Defaults enable nearly everything; test configurations turn
//! knobs:
//!
//! ```toml
//! [[device]]
//! model = "synthetic"
//! synthetic = { speed = "super", transfer = "bulk", uvc = "1.1", streams = 2, still = 3 }
//! ```

use serde::{Deserialize, Serialize};

use emuvc_usb::desc::{
    ConfigBuilder, ConfigView, DeviceDescriptor, EndpointDescriptor, IadDescriptor,
    InterfaceDescriptor, SsEndpointCompanion,
};
use emuvc_usb::{CompositeDevice, Descriptors, Speed};
use emuvc_uvc::UvcFunction;
use emuvc_uvc::consts::*;
use emuvc_uvc::desc::*;
use emuvc_uvc::topology::{find_video_control_interfaces, parse_function};
use emuvc_video::PixelFormat;

use crate::config::DeviceConfig;

pub const NAME: &str = "synthetic";
pub const DESCRIPTION: &str =
    "configurable camera exercising most of UVC ([device.synthetic] options)";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SyntheticConfig {
    pub vendor_id: u16,
    pub product_id: u16,
    /// "1.0", "1.1" or "1.5".
    pub uvc: String,
    pub speed: Speed,
    /// "iso" or "bulk".
    pub transfer: String,
    /// Format names: mjpeg, h264 (frame-based) or any uncompressed format
    /// (yuyv, uyvy, nv12, nv21, i420, p010, grey, grey16, bgr24, rgb24,
    /// by8, ba81, grbg8, gbrg8, rggb8, bggr8).
    pub formats: Vec<String>,
    pub sizes: Vec<(u16, u16)>,
    /// Frame rates of every frame descriptor, highest first.
    pub fps: Vec<u32>,
    /// Continuous frame intervals (from the highest to the lowest fps).
    pub continuous: bool,
    /// dwFrameIntervalStep override for continuous intervals (0 is a classic bug).
    pub interval_step: Option<u32>,
    /// VideoStreaming interfaces in the function.
    pub streams: u8,
    /// Still image capture method (0-3).
    pub still: u8,
    /// Hardware trigger (still image button) support.
    pub trigger: bool,
    pub status_endpoint: bool,
    pub selector_unit: bool,
    pub encoding_unit: bool,
    pub extension_units: u8,
    /// Add MPEG2-TS, DV and stream-based format descriptors (not streamable).
    pub extra_formats: bool,
    /// An isochronous alternate setting with wMaxPacketSize 0.
    pub zero_packet_alt: bool,
}

impl Default for SyntheticConfig {
    fn default() -> Self {
        SyntheticConfig {
            vendor_id: 0x1209, // pid.codes test VID/PID
            product_id: 0x0001,
            uvc: "1.5".into(),
            speed: Speed::High,
            transfer: "iso".into(),
            formats: ["mjpeg", "yuyv", "nv12", "grey", "h264"]
                .map(String::from)
                .to_vec(),
            sizes: vec![(640, 480), (320, 240), (160, 120)],
            fps: vec![30, 15, 5],
            continuous: false,
            interval_step: None,
            streams: 1,
            still: 2,
            trigger: true,
            status_endpoint: true,
            selector_unit: true,
            // libuvc rejects devices with encoding units (unknown VC subtype).
            encoding_unit: false,
            extension_units: 2,
            extra_formats: false,
            zero_packet_alt: false,
        }
    }
}

const CT_ID: u8 = 1;
const SU_ID: u8 = 2;
const PU_ID: u8 = 3;
const EU_ID: u8 = 4;
const XU_FIRST: u8 = 5;
const OT_FIRST: u8 = 16;
const STATUS_EP: u8 = 0x87;

enum Fmt {
    Mjpeg,
    H264,
    Raw(PixelFormat),
}

fn format_by_name(name: &str) -> Result<Fmt, String> {
    use PixelFormat::*;
    Ok(match name {
        "mjpeg" => Fmt::Mjpeg,
        "h264" => Fmt::H264,
        "yuyv" => Fmt::Raw(Yuyv),
        "uyvy" => Fmt::Raw(Uyvy),
        "nv12" => Fmt::Raw(Nv12),
        "nv21" => Fmt::Raw(Nv21),
        "i420" => Fmt::Raw(I420),
        "p010" => Fmt::Raw(P010),
        "grey" | "gray" | "grey8" => Fmt::Raw(Grey8),
        "grey16" | "gray16" | "y16" => Fmt::Raw(Grey16),
        "bgr24" => Fmt::Raw(Bgr24),
        "rgb24" => Fmt::Raw(Rgb24),
        "by8" => Fmt::Raw(By8),
        "ba81" => Fmt::Raw(Ba81),
        "grbg8" => Fmt::Raw(Grbg8),
        "gbrg8" => Fmt::Raw(Gbrg8),
        "rggb8" => Fmt::Raw(Rggb8),
        "bggr8" => Fmt::Raw(Bggr8),
        _ => return Err(format!("synthetic: unknown format {name:?}")),
    })
}

/// bmControls with every bit whose selector exists in the UVC version.
fn control_mask(selector: fn(u32) -> Option<u8>, bits: u32, skip: &[u8]) -> u32 {
    (0..bits)
        .filter(|&b| selector(b).is_some_and(|s| !skip.contains(&s)))
        .fold(0, |m, b| m | 1 << b)
}

fn frames(cfg: &SyntheticConfig, fmt: &Fmt) -> Vec<VsFrame> {
    let intervals: Vec<u32> = {
        let mut v: Vec<u32> = cfg
            .fps
            .iter()
            .filter(|&&f| f > 0)
            .map(|&f| 10_000_000 / f)
            .collect();
        v.sort_unstable();
        v.dedup();
        if v.is_empty() { vec![333_333] } else { v }
    };
    let (shortest, longest) = (intervals[0], *intervals.last().unwrap());
    cfg.sizes
        .iter()
        .map(|&(w, h)| {
            let px = w as u32 * h as u32;
            let (size, bytes_per_line) = match fmt {
                Fmt::Raw(pf) => (pf.frame_size(w, h).unwrap_or(px as usize * 2) as u32, 0),
                Fmt::Mjpeg => (px * 2, 0),
                Fmt::H264 => (0, 0),
            };
            let rate = |interval: u32| {
                ((size.max(px) as u64 * 8 * 10_000_000 / interval as u64).min(u32::MAX as u64))
                    as u32
            };
            VsFrame {
                capabilities: (cfg.still > 0) as u8,
                width: w,
                height: h,
                min_bit_rate: rate(longest),
                max_bit_rate: rate(shortest),
                max_frame_size: size,
                bytes_per_line,
                default_interval: shortest,
                intervals: if cfg.continuous {
                    let step = cfg.interval_step.unwrap_or(1);
                    FrameIntervals::Continuous {
                        min: shortest,
                        max: longest,
                        step,
                    }
                } else {
                    FrameIntervals::Discrete(intervals.clone())
                },
            }
        })
        .collect()
}

fn ss_companion(max_burst: u8, attributes: u8, bytes_per_interval: u16) -> SsEndpointCompanion {
    SsEndpointCompanion {
        max_burst,
        attributes,
        bytes_per_interval,
    }
}

pub fn descriptors(cfg: &SyntheticConfig) -> Result<Descriptors, String> {
    let bcd_uvc = match cfg.uvc.as_str() {
        "1.0" => 0x0100,
        "1.1" => 0x0110,
        "1.5" => 0x0150,
        v => return Err(format!("synthetic: unsupported UVC version {v:?}")),
    };
    let bulk = match cfg.transfer.as_str() {
        "iso" => false,
        "bulk" => true,
        t => {
            return Err(format!(
                "synthetic: transfer must be iso or bulk, not {t:?}"
            ));
        }
    };
    let streams = cfg.streams.clamp(1, 4);
    let ss = matches!(cfg.speed, Speed::Super | Speed::SuperPlus);
    let protocol = (bcd_uvc >= 0x0150) as u8;
    let formats: Vec<Fmt> = cfg
        .formats
        .iter()
        .map(|f| format_by_name(f))
        .collect::<Result<_, _>>()?;
    if formats.is_empty() {
        return Err("synthetic: no formats".into());
    }

    let device = DeviceDescriptor {
        bcd_usb: match cfg.speed {
            Speed::Super | Speed::SuperPlus => 0x0320,
            Speed::High => 0x0201,
            _ => 0x0110,
        },
        class: 0xef,
        subclass: 2,
        protocol: 1,
        max_packet_size0: if ss { 9 } else { 64 },
        vendor_id: cfg.vendor_id,
        product_id: cfg.product_id,
        bcd_device: 0x0100,
        i_manufacturer: 1,
        i_product: 2,
        i_serial: 3,
        num_configurations: 1,
    };

    // --- VideoControl ---
    let ct_skip: &[u8] = &[CT_WINDOW_CONTROL];
    let (ct_bits, pu_bits, pu_size) = match bcd_uvc {
        0x0100 => (18, 16, 2),
        0x0110 => (19, 16, 2),
        _ => (22, 19, 3),
    };
    let mut entities = vec![VcEntity::Camera(CameraTerminal {
        id: CT_ID,
        assoc_terminal: 0,
        i_terminal: 0,
        objective_focal_length_min: 0,
        objective_focal_length_max: 100,
        ocular_focal_length: 50,
        control_size: 3,
        controls: control_mask(ct_selector, ct_bits, ct_skip),
    })];
    let mut last = CT_ID;
    if cfg.selector_unit {
        entities.push(VcEntity::Selector(SelectorUnit {
            id: SU_ID,
            sources: vec![CT_ID],
            i_selector: 0,
        }));
        last = SU_ID;
    }
    entities.push(VcEntity::Processing(ProcessingUnit {
        id: PU_ID,
        source: last,
        max_multiplier: 1000,
        control_size: pu_size,
        controls: control_mask(pu_selector, pu_bits, &[]),
        i_processing: 0,
        video_standards: (bcd_uvc >= 0x0110).then_some(0),
    }));
    last = PU_ID;
    if cfg.encoding_unit && bcd_uvc >= 0x0150 {
        entities.push(VcEntity::Encoding(EncodingUnit {
            id: EU_ID,
            source: last,
            i_encoding: 0,
            control_size: 3,
            controls: 0,
            runtime_controls: 0,
        }));
        last = EU_ID;
    }
    for i in 0..cfg.extension_units.min(8) {
        let id = XU_FIRST + i;
        let mut g = *b"emuvc-synth-xu-0";
        g[15] = b'0' + i;
        entities.push(VcEntity::Extension(ExtensionUnit {
            id,
            guid: g,
            num_controls: 4,
            sources: vec![last],
            control_size: 1,
            controls: vec![0x0f],
            i_extension: 0,
        }));
        last = id;
    }
    for s in 0..streams {
        entities.push(VcEntity::Output(OutputTerminal {
            id: OT_FIRST + s,
            terminal_type: TT_STREAMING,
            assoc_terminal: 0,
            source: last,
            i_terminal: 0,
            extra: vec![],
        }));
    }
    let header = VcHeader {
        bcd_uvc,
        clock_frequency: 48_000_000,
        streaming_interfaces: (1..=streams).collect(),
    };

    let mut cfgb = ConfigBuilder::new(1).max_power(if ss { 112 } else { 250 });
    cfgb.iad(&IadDescriptor {
        first_interface: 0,
        interface_count: 1 + streams,
        function_class: CC_VIDEO,
        function_subclass: SC_VIDEO_INTERFACE_COLLECTION,
        function_protocol: 0,
        i_function: 2,
    });
    let iface = |number: u8, alt: u8, eps: u8, subclass: u8| InterfaceDescriptor {
        number,
        alt,
        num_endpoints: eps,
        class: CC_VIDEO,
        subclass,
        protocol,
        i_interface: if subclass == SC_VIDEOCONTROL { 2 } else { 0 },
    };
    cfgb.interface(&iface(0, 0, cfg.status_endpoint as u8, SC_VIDEOCONTROL));
    cfgb.raw(&vc_block(&header, &entities));
    if cfg.status_endpoint {
        cfgb.endpoint(&EndpointDescriptor {
            address: STATUS_EP,
            attributes: 3,
            max_packet_size: 16,
            interval: if ss || cfg.speed == Speed::High {
                8
            } else {
                32
            },
        });
        if ss {
            cfgb.ss_companion(&ss_companion(0, 0, 16));
        }
        cfgb.raw(&vc_interrupt_endpoint(16));
    }

    // --- VideoStreaming ---
    let bulk_mps: u16 = match cfg.speed {
        Speed::Super | Speed::SuperPlus => 1024,
        Speed::High => 512,
        _ => 64,
    };
    for s in 0..streams {
        let number = 1 + s;
        let ep = 0x81 + s;
        let still_ep = (cfg.still == 3).then_some(0x88 + s);
        let entries: Vec<VsFormatEntry> = formats
            .iter()
            .map(|f| {
                let kind = match f {
                    Fmt::Mjpeg => VsFormatKind::Mjpeg { flags: 1 },
                    Fmt::H264 => VsFormatKind::FrameBased {
                        guid: PixelFormat::H264.guid().unwrap(),
                        bits_per_pixel: 16,
                        variable_size: true,
                    },
                    Fmt::Raw(pf) => VsFormatKind::Uncompressed {
                        guid: pf.guid().unwrap(),
                        bits_per_pixel: pf.bits_per_pixel().unwrap_or(16),
                    },
                };
                let mut e = VsFormatEntry::new(VsFormat {
                    kind,
                    default_frame: 1,
                    aspect_x: 0,
                    aspect_y: 0,
                    interlace_flags: 0,
                    copy_protect: 0,
                    frames: frames(cfg, f),
                })
                .with_color(ColorMatching {
                    primaries: 1,
                    transfer: 1,
                    matrix: 4,
                });
                if cfg.still >= 2 {
                    e = e.with_still(VsStillFrame {
                        endpoint: still_ep.unwrap_or(0),
                        sizes: cfg.sizes.clone(),
                        compression: vec![],
                    });
                }
                e
            })
            .collect();
        let mut vs = vs_block(
            &VsInputHeader {
                endpoint: ep,
                info: 0,
                terminal_link: OT_FIRST + s,
                still_capture_method: cfg.still.min(3),
                trigger_support: cfg.trigger as u8,
                trigger_usage: 0,
                control_size: 1,
                format_controls: formats
                    .iter()
                    .map(|f| if matches!(f, Fmt::Mjpeg) { 0x04 } else { 0 })
                    .collect(),
            },
            &entries,
        );
        if cfg.extra_formats && s == 0 {
            // Formats hosts parse but nobody streams here, appended as raw
            // descriptors after the regular ones (indexes continue).
            let next = entries.len() as u8 + 1;
            vs.extend_from_slice(&[7, CS_INTERFACE, VS_FORMAT_MPEG2TS, next, 0, 188, 188]);
            vs.extend_from_slice(&[
                9,
                CS_INTERFACE,
                VS_FORMAT_DV,
                next + 1,
                0x40,
                0x0d,
                0x03,
                0,
                0x02,
            ]);
            vs.extend_from_slice(&[24, CS_INTERFACE, VS_FORMAT_STREAM_BASED, next + 2]);
            vs.extend_from_slice(&PixelFormat::H264.guid().unwrap());
            vs.extend_from_slice(&0u32.to_le_bytes());
            fix_input_header(&mut vs, next + 2);
        }
        let still_endpoint = |b: &mut ConfigBuilder| {
            if let Some(a) = still_ep {
                b.endpoint(&EndpointDescriptor {
                    address: a,
                    attributes: 2,
                    max_packet_size: bulk_mps,
                    interval: 0,
                });
                if ss {
                    b.ss_companion(&ss_companion(0, 0, 0));
                }
            }
        };
        let n_still = still_ep.is_some() as u8;
        if bulk {
            cfgb.interface(&iface(number, 0, 1 + n_still, SC_VIDEOSTREAMING));
            cfgb.raw(&vs);
            cfgb.endpoint(&EndpointDescriptor {
                address: ep,
                attributes: 2,
                max_packet_size: bulk_mps,
                interval: 0,
            });
            if ss {
                cfgb.ss_companion(&ss_companion(15, 0, 0));
            }
            still_endpoint(&mut cfgb);
        } else {
            cfgb.interface(&iface(number, 0, n_still, SC_VIDEOSTREAMING));
            cfgb.raw(&vs);
            still_endpoint(&mut cfgb);
            // (wMaxPacketSize, SS max burst, SS mult) per alternate setting.
            let mut alts: Vec<(u16, u8, u8)> = match cfg.speed {
                Speed::Super | Speed::SuperPlus => vec![
                    (1024, 0, 0),
                    (1024, 3, 0),
                    (1024, 15, 0),
                    (1024, 15, 1),
                    (1024, 15, 2),
                ],
                Speed::High => vec![
                    (128, 0, 0),
                    (512, 0, 0),
                    (1024, 0, 0),
                    (0x0c00, 0, 0),
                    (0x1400, 0, 0),
                ],
                _ => vec![(256, 0, 0), (512, 0, 0), (1023, 0, 0)],
            };
            if cfg.zero_packet_alt {
                alts.insert(0, (0, 0, 0));
            }
            for (i, &(mps, burst, mult)) in alts.iter().enumerate() {
                cfgb.interface(&iface(number, i as u8 + 1, 1 + n_still, SC_VIDEOSTREAMING));
                cfgb.endpoint(&EndpointDescriptor {
                    address: ep,
                    attributes: 5,
                    max_packet_size: mps,
                    interval: 1,
                });
                if ss {
                    let bpi = (mps as u32 * (burst as u32 + 1) * (mult as u32 + 1))
                        .min(u16::MAX as u32) as u16;
                    cfgb.ss_companion(&ss_companion(burst, mult, bpi));
                }
                still_endpoint(&mut cfgb);
            }
        }
    }

    let mut bos = None;
    if device.bcd_usb >= 0x0201 {
        let mut caps = vec![7, 0x10, 0x02, 0x02, 0, 0, 0]; // USB 2.0 extension: LPM
        if ss {
            caps.extend_from_slice(&[10, 0x10, 0x03, 0, 0x0e, 0, 1, 10, 0xff, 0x07]); // SuperSpeed
        }
        let mut b = vec![5, 0x0f, 0, 0, if ss { 2 } else { 1 }];
        b.extend(caps);
        let total = b.len() as u16;
        b[2..4].copy_from_slice(&total.to_le_bytes());
        bos = Some(b);
    }

    Ok(Descriptors {
        device: device.to_bytes(),
        configs: vec![cfgb.build()],
        bos,
        langids: vec![0x0409],
        strings: [(1, "emuvc"), (2, "Synthetic UVC camera"), (3, "SYN0001")]
            .into_iter()
            .map(|(i, s)| (i, s.to_string()))
            .collect(),
        ..Default::default()
    })
}

/// Rewrites bNumFormats and wTotalLength of a VS input header at the start of `vs`.
fn fix_input_header(vs: &mut [u8], num_formats: u8) {
    let total = vs.len() as u16;
    vs[3] = num_formats;
    vs[4..6].copy_from_slice(&total.to_le_bytes());
}

pub fn build(cfg: &DeviceConfig) -> Result<CompositeDevice, String> {
    let s = &cfg.synthetic;
    let mut desc = descriptors(s)?;
    cfg.apply_strings(&mut desc);
    let view = ConfigView::parse(&desc.configs[0]).map_err(|e| e.to_string())?;
    let mut dev = CompositeDevice::new(
        format!("synthetic-{:04x}:{:04x}", s.vendor_id, s.product_id),
        s.speed,
        desc,
    );
    for (i, vc) in find_video_control_interfaces(&view).into_iter().enumerate() {
        let f = parse_function(&view, vc, s.speed).map_err(|e| e.to_string())?;
        let mut uvc = UvcFunction::new(f, s.speed, cfg.uvc.clone(), &mut |_| cfg.frame_source());
        // Extension unit controls of various sizes.
        for x in 0..s.extension_units.min(8) {
            for (sel, len) in [(1u8, 1u16), (2, 2), (3, 4), (4, 64)] {
                uvc.set_control(XU_FIRST + x, sel, emuvc_uvc::control::Control::raw(len));
            }
        }
        // Power mode control: USB powered, vendor dependent mode supported.
        uvc.set_power_mode(0x30);
        if i == 0 {
            uvc.set_events(cfg.events.clone());
            uvc.configure_controls(&cfg.controls)?;
        }
        dev.add_function(&uvc.interfaces(), Box::new(uvc));
    }
    Ok(dev)
}
