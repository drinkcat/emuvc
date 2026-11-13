//! Parses a UVC function (VideoControl interface plus its VideoStreaming
//! interfaces) out of a configuration descriptor into a typed model.
//!
//! Lenient on real-world junk: unknown descriptor subtypes are skipped, while
//! descriptors too short for their subtype are reported with context.

use crate::consts::*;
use emuvc_usb::Speed;
use emuvc_usb::TransferType;
use emuvc_usb::desc::{ConfigView, InterfaceView, split};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TopologyError {
    /// The interface does not exist in the configuration.
    NoSuchInterface(u8),
    /// The interface has no alternate setting 0 or is not a UVC interface.
    NotUvc(u8),
    /// No class-specific header descriptor found.
    MissingHeader { interface: u8 },
    /// A descriptor is too short or inconsistent for its subtype.
    Malformed {
        interface: u8,
        subtype: u8,
        what: &'static str,
    },
}

impl std::fmt::Display for TopologyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TopologyError::NoSuchInterface(i) => write!(f, "interface {i} not found"),
            TopologyError::NotUvc(i) => write!(f, "interface {i} is not a UVC interface"),
            TopologyError::MissingHeader { interface } => {
                write!(f, "interface {interface}: missing class-specific header")
            }
            TopologyError::Malformed {
                interface,
                subtype,
                what,
            } => {
                write!(
                    f,
                    "interface {interface}: malformed descriptor subtype {subtype:#x}: {what}"
                )
            }
        }
    }
}

impl std::error::Error for TopologyError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UvcFunctionDesc {
    pub vc: VideoControl,
    pub streaming: Vec<VideoStreaming>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEndpoint {
    pub address: u8,
    pub max_packet: usize,
    pub interval_uframes: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoControl {
    pub interface: u8,
    pub bcd_uvc: u16,
    pub clock_frequency: u32,
    pub streaming_interfaces: Vec<u8>,
    pub entities: Vec<Entity>,
    pub status_endpoint: Option<StatusEndpoint>,
}

impl VideoControl {
    pub fn entity(&self, id: u8) -> Option<&Entity> {
        self.entities.iter().find(|e| e.id == id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entity {
    pub id: u8,
    pub kind: EntityKind,
    pub sources: Vec<u8>,
    /// Raw `bmControls` bytes (empty for entities without one).
    pub bm_controls: Vec<u8>,
    /// Supported control selectors decoded from `bm_controls`.
    pub controls: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntityKind {
    Camera,
    InputTerminal {
        terminal_type: u16,
    },
    OutputTerminal {
        terminal_type: u16,
    },
    Selector,
    Processing {
        max_multiplier: u16,
        video_standards: Option<u8>,
    },
    Extension {
        guid: [u8; 16],
        num_controls: u8,
    },
    Encoding,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoStreaming {
    pub interface: u8,
    pub endpoint: u8,
    pub terminal_link: u8,
    pub still_capture_method: u8,
    pub trigger_support: u8,
    pub trigger_usage: u8,
    pub formats: Vec<Format>,
    pub transfer: Transfer,
}

impl VideoStreaming {
    pub fn format(&self, index: u8) -> Option<&Format> {
        self.formats.iter().find(|f| f.index == index)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoAlt {
    pub alt: u8,
    pub bytes_per_interval: usize,
    pub interval_uframes: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transfer {
    Isochronous { alts: Vec<IsoAlt> },
    Bulk { max_packet: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Format {
    pub index: u8,
    pub kind: FormatKind,
    pub default_frame: u8,
    pub frames: Vec<Frame>,
    /// `(primaries, transfer characteristics, matrix coefficients)`.
    pub color: Option<(u8, u8, u8)>,
    /// Still image frame descriptor (still capture methods 2 and 3).
    pub still: Option<StillFrame>,
}

/// VS still image frame descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StillFrame {
    /// Bulk endpoint for method 3, 0 for method 2.
    pub endpoint: u8,
    /// Image sizes; bFrameIndex of a still probe is 1-based into this list.
    pub sizes: Vec<(u16, u16)>,
    pub compression: Vec<u8>,
}

impl Format {
    pub fn frame(&self, index: u8) -> Option<&Frame> {
        self.frames.iter().find(|f| f.index == index)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatKind {
    Uncompressed {
        guid: [u8; 16],
        bits_per_pixel: u8,
    },
    Mjpeg,
    FrameBased {
        guid: [u8; 16],
        bits_per_pixel: u8,
        variable_size: bool,
    },
    Other {
        subtype: u8,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub index: u8,
    pub capabilities: u8,
    pub width: u16,
    pub height: u16,
    pub min_bit_rate: u32,
    pub max_bit_rate: u32,
    /// `dwMaxVideoFrameBufferSize`; for frame-based formats (which lack the
    /// field) `bytes_per_line * height`.
    pub max_frame_size: u32,
    /// `dwBytesPerLine` (frame-based formats only, else 0).
    pub bytes_per_line: u32,
    pub default_interval: u32,
    pub intervals: Intervals,
}

impl Frame {
    /// The supported frame interval closest to `want` (100 ns units).
    pub fn nearest_interval(&self, want: u32) -> u32 {
        match &self.intervals {
            Intervals::Discrete(l) => l
                .iter()
                .copied()
                .min_by_key(|i| i.abs_diff(want))
                .unwrap_or(want),
            &Intervals::Continuous { min, max, step } => {
                let v = want.clamp(min, max.max(min));
                if step == 0 {
                    return v;
                }
                let snapped =
                    min as u64 + ((v - min) as u64 + step as u64 / 2) / step as u64 * step as u64;
                (snapped.min(max as u64)) as u32
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intervals {
    Discrete(Vec<u32>),
    Continuous { min: u32, max: u32, step: u32 },
}

/// Interface numbers of all VideoControl interfaces in the configuration.
pub fn find_video_control_interfaces(cfg: &ConfigView) -> Vec<u8> {
    cfg.interfaces
        .iter()
        .filter(|i| {
            i.alts
                .iter()
                .any(|a| a.desc.class == CC_VIDEO && a.desc.subclass == SC_VIDEOCONTROL)
        })
        .map(|i| i.number)
        .collect()
}

fn le16(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn le32(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// Selectors for each set bit of `bm`, using `map(bit)`.
fn decode_controls(bm: &[u8], map: impl Fn(u32) -> Option<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    for (bi, byte) in bm.iter().enumerate() {
        for bit in 0..8 {
            if byte & (1 << bit) != 0 {
                out.extend(map((bi * 8 + bit) as u32));
            }
        }
    }
    out
}

fn xu_selector(bit: u32) -> Option<u8> {
    u8::try_from(bit + 1).ok()
}

/// Iterates the class-specific interface descriptors in `extra` as `(subtype, bytes)`.
fn cs_descriptors(extra: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    split(extra)
        .map_while(Result::ok)
        .filter_map(|(_, d)| (d[1] == CS_INTERFACE && d.len() >= 3).then_some((d[2], d)))
}

fn need(d: &[u8], len: usize, interface: u8, what: &'static str) -> Result<(), TopologyError> {
    if d.len() < len {
        Err(TopologyError::Malformed {
            interface,
            subtype: d[2],
            what,
        })
    } else {
        Ok(())
    }
}

fn parse_entity(d: &[u8], itf: u8) -> Result<Option<Entity>, TopologyError> {
    let sub = d[2];
    let e = match sub {
        VC_INPUT_TERMINAL => {
            need(d, 8, itf, "input terminal shorter than 8 bytes")?;
            let terminal_type = le16(d, 4);
            if terminal_type == ITT_CAMERA && d.len() >= 15 {
                let n = d[14] as usize;
                need(
                    d,
                    15 + n,
                    itf,
                    "camera terminal bmControls overruns descriptor",
                )?;
                let bm = d[15..15 + n].to_vec();
                Entity {
                    id: d[3],
                    kind: EntityKind::Camera,
                    sources: vec![],
                    controls: decode_controls(&bm, ct_selector),
                    bm_controls: bm,
                }
            } else {
                Entity {
                    id: d[3],
                    kind: EntityKind::InputTerminal { terminal_type },
                    sources: vec![],
                    bm_controls: vec![],
                    controls: vec![],
                }
            }
        }
        VC_OUTPUT_TERMINAL => {
            need(d, 9, itf, "output terminal shorter than 9 bytes")?;
            Entity {
                id: d[3],
                kind: EntityKind::OutputTerminal {
                    terminal_type: le16(d, 4),
                },
                sources: vec![d[7]],
                bm_controls: vec![],
                controls: vec![],
            }
        }
        VC_SELECTOR_UNIT => {
            need(d, 5, itf, "selector unit too short")?;
            let p = d[4] as usize;
            need(
                d,
                6 + p,
                itf,
                "selector unit baSourceID overruns descriptor",
            )?;
            Entity {
                id: d[3],
                kind: EntityKind::Selector,
                sources: d[5..5 + p].to_vec(),
                bm_controls: vec![],
                controls: vec![SU_INPUT_SELECT_CONTROL],
            }
        }
        VC_PROCESSING_UNIT => {
            need(d, 8, itf, "processing unit too short")?;
            let n = d[7] as usize;
            need(
                d,
                9 + n,
                itf,
                "processing unit bmControls overruns descriptor",
            )?;
            let bm = d[8..8 + n].to_vec();
            Entity {
                id: d[3],
                kind: EntityKind::Processing {
                    max_multiplier: le16(d, 5),
                    video_standards: d.get(9 + n).copied(),
                },
                sources: vec![d[4]],
                controls: decode_controls(&bm, pu_selector),
                bm_controls: bm,
            }
        }
        VC_EXTENSION_UNIT => {
            need(d, 23, itf, "extension unit too short")?;
            let p = d[21] as usize;
            need(
                d,
                23 + p,
                itf,
                "extension unit baSourceID overruns descriptor",
            )?;
            let n = d[22 + p] as usize;
            need(
                d,
                23 + p + n,
                itf,
                "extension unit bmControls overruns descriptor",
            )?;
            let bm = d[23 + p..23 + p + n].to_vec();
            let mut guid = [0u8; 16];
            guid.copy_from_slice(&d[4..20]);
            Entity {
                id: d[3],
                kind: EntityKind::Extension {
                    guid,
                    num_controls: d[20],
                },
                sources: d[22..22 + p].to_vec(),
                controls: decode_controls(&bm, xu_selector),
                bm_controls: bm,
            }
        }
        VC_ENCODING_UNIT => {
            need(d, 7, itf, "encoding unit too short")?;
            let n = d[6] as usize;
            need(
                d,
                7 + n,
                itf,
                "encoding unit bmControls overruns descriptor",
            )?;
            let bm = d[7..7 + n].to_vec();
            Entity {
                id: d[3],
                kind: EntityKind::Encoding,
                sources: vec![d[4]],
                controls: decode_controls(&bm, eu_selector),
                bm_controls: bm,
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(e))
}

fn parse_vc(itf: &InterfaceView, speed: Speed) -> Result<VideoControl, TopologyError> {
    let alt0 = itf
        .alts
        .iter()
        .find(|a| a.desc.alt == 0)
        .ok_or(TopologyError::NotUvc(itf.number))?;
    let mut vc = None;
    let mut entities = Vec::new();
    for (sub, d) in cs_descriptors(&alt0.extra) {
        if sub == VC_HEADER {
            need(d, 12, itf.number, "VC header shorter than 12 bytes")?;
            let n = d[11] as usize;
            need(
                d,
                12 + n,
                itf.number,
                "VC header baInterfaceNr overruns descriptor",
            )?;
            vc = Some(VideoControl {
                interface: itf.number,
                bcd_uvc: le16(d, 3),
                clock_frequency: le32(d, 7),
                streaming_interfaces: d[12..12 + n].to_vec(),
                entities: vec![],
                status_endpoint: None,
            });
        } else if let Some(e) = parse_entity(d, itf.number)? {
            entities.push(e);
        }
    }
    let mut vc = vc.ok_or(TopologyError::MissingHeader {
        interface: itf.number,
    })?;
    vc.entities = entities;
    vc.status_endpoint = alt0
        .endpoints
        .iter()
        .find(|e| e.desc.transfer_type() == TransferType::Interrupt)
        .map(|e| StatusEndpoint {
            address: e.desc.address,
            max_packet: (e.desc.max_packet_size & 0x7ff) as usize,
            interval_uframes: e.desc.interval_uframes(speed),
        });
    Ok(vc)
}

fn parse_intervals(
    d: &[u8],
    type_off: usize,
    list_off: usize,
    itf: u8,
) -> Result<Intervals, TopologyError> {
    let t = d[type_off] as usize;
    if t == 0 {
        need(
            d,
            list_off + 12,
            itf,
            "continuous frame intervals overrun descriptor",
        )?;
        Ok(Intervals::Continuous {
            min: le32(d, list_off),
            max: le32(d, list_off + 4),
            step: le32(d, list_off + 8),
        })
    } else {
        need(
            d,
            list_off + 4 * t,
            itf,
            "discrete frame intervals overrun descriptor",
        )?;
        Ok(Intervals::Discrete(
            (0..t).map(|i| le32(d, list_off + 4 * i)).collect(),
        ))
    }
}

fn parse_frame(d: &[u8], itf: u8) -> Result<Frame, TopologyError> {
    let frame_based = d[2] == VS_FRAME_FRAME_BASED;
    need(d, 26, itf, "frame descriptor shorter than 26 bytes")?;
    let (height, bytes_per_line) = (le16(d, 7), if frame_based { le32(d, 22) } else { 0 });
    Ok(Frame {
        index: d[3],
        capabilities: d[4],
        width: le16(d, 5),
        height,
        min_bit_rate: le32(d, 9),
        max_bit_rate: le32(d, 13),
        max_frame_size: if frame_based {
            bytes_per_line.wrapping_mul(height as u32)
        } else {
            le32(d, 17)
        },
        bytes_per_line,
        default_interval: le32(d, if frame_based { 17 } else { 21 }),
        intervals: if frame_based {
            parse_intervals(d, 21, 26, itf)?
        } else {
            parse_intervals(d, 25, 26, itf)?
        },
    })
}

fn parse_vs(itf: &InterfaceView, speed: Speed) -> Result<VideoStreaming, TopologyError> {
    let n = itf.number;
    let alt0 = itf
        .alts
        .iter()
        .find(|a| a.desc.alt == 0)
        .ok_or(TopologyError::NotUvc(n))?;
    let mut hdr = None;
    let mut formats: Vec<Format> = Vec::new();
    // Some cameras (ELP) put the class-specific descriptors after the endpoint.
    let mut cs = alt0.extra.clone();
    alt0.endpoints
        .iter()
        .for_each(|e| cs.extend_from_slice(&e.extra));
    for (sub, d) in cs_descriptors(&cs) {
        match sub {
            VS_INPUT_HEADER => {
                need(d, 13, n, "VS input header shorter than 13 bytes")?;
                hdr = Some((d[6], d[8], d[9], d[10], d[11]));
            }
            VS_OUTPUT_HEADER => {
                need(d, 8, n, "VS output header shorter than 8 bytes")?;
                hdr = Some((d[6], d[7], 0, 0, 0));
            }
            VS_FORMAT_UNCOMPRESSED | VS_FORMAT_MJPEG | VS_FORMAT_FRAME_BASED => {
                let (kind, def_off) = match sub {
                    VS_FORMAT_MJPEG => {
                        need(d, 11, n, "MJPEG format shorter than 11 bytes")?;
                        (FormatKind::Mjpeg, 6)
                    }
                    VS_FORMAT_UNCOMPRESSED => {
                        need(d, 27, n, "uncompressed format shorter than 27 bytes")?;
                        let mut guid = [0u8; 16];
                        guid.copy_from_slice(&d[5..21]);
                        (
                            FormatKind::Uncompressed {
                                guid,
                                bits_per_pixel: d[21],
                            },
                            22,
                        )
                    }
                    _ => {
                        need(d, 28, n, "frame-based format shorter than 28 bytes")?;
                        let mut guid = [0u8; 16];
                        guid.copy_from_slice(&d[5..21]);
                        (
                            FormatKind::FrameBased {
                                guid,
                                bits_per_pixel: d[21],
                                variable_size: d[27] != 0,
                            },
                            22,
                        )
                    }
                };
                formats.push(Format {
                    index: d[3],
                    kind,
                    default_frame: d[def_off],
                    frames: vec![],
                    color: None,
                    still: None,
                });
            }
            VS_FORMAT_MPEG2TS
            | VS_FORMAT_DV
            | VS_FORMAT_STREAM_BASED
            | VS_FORMAT_H264
            | VS_FORMAT_H264_SIMULCAST
            | VS_FORMAT_VP8
            | VS_FORMAT_VP8_SIMULCAST => {
                need(d, 4, n, "format descriptor too short")?;
                formats.push(Format {
                    index: d[3],
                    kind: FormatKind::Other { subtype: sub },
                    default_frame: 0,
                    frames: vec![],
                    color: None,
                    still: None,
                });
            }
            VS_FRAME_UNCOMPRESSED | VS_FRAME_MJPEG | VS_FRAME_FRAME_BASED => {
                let frame = parse_frame(d, n)?;
                if let Some(f) = formats.last_mut() {
                    f.frames.push(frame);
                }
            }
            VS_STILL_IMAGE_FRAME => {
                need(d, 5, n, "still image frame descriptor shorter than 5 bytes")?;
                let ns = d[4] as usize;
                need(
                    d,
                    5 + 4 * ns + 1,
                    n,
                    "still image frame descriptor truncated",
                )?;
                let sizes = (0..ns)
                    .map(|i| {
                        (
                            u16::from_le_bytes([d[5 + 4 * i], d[6 + 4 * i]]),
                            u16::from_le_bytes([d[7 + 4 * i], d[8 + 4 * i]]),
                        )
                    })
                    .collect();
                let nc = d[5 + 4 * ns] as usize;
                let compression = d.get(6 + 4 * ns..6 + 4 * ns + nc).unwrap_or(&[]).to_vec();
                if let Some(f) = formats.last_mut() {
                    f.still = Some(StillFrame {
                        endpoint: d[3],
                        sizes,
                        compression,
                    });
                }
            }
            VS_COLORFORMAT => {
                need(d, 6, n, "color matching descriptor shorter than 6 bytes")?;
                if let Some(f) = formats.last_mut() {
                    f.color = Some((d[3], d[4], d[5]));
                }
            }
            _ => {}
        }
    }
    let (endpoint, terminal_link, still_capture_method, trigger_support, trigger_usage) =
        hdr.ok_or(TopologyError::MissingHeader { interface: n })?;

    // Transfer type: isochronous alt settings (with an endpoint at the
    // advertised address) or a bulk endpoint.
    let mut alts = Vec::new();
    let mut bulk = None;
    for a in &itf.alts {
        for ep in a.endpoints.iter().filter(|e| e.desc.address == endpoint) {
            match ep.desc.transfer_type() {
                TransferType::Isochronous if a.desc.alt != 0 => alts.push(IsoAlt {
                    alt: a.desc.alt,
                    bytes_per_interval: ep.bytes_per_interval(speed),
                    interval_uframes: ep.desc.interval_uframes(speed),
                }),
                TransferType::Bulk if bulk.is_none() => {
                    bulk = Some((ep.desc.max_packet_size & 0x7ff) as usize)
                }
                _ => {}
            }
        }
    }
    let transfer = match (alts.is_empty(), bulk) {
        (true, Some(max_packet)) => Transfer::Bulk { max_packet },
        _ => Transfer::Isochronous { alts },
    };
    Ok(VideoStreaming {
        interface: n,
        endpoint,
        terminal_link,
        still_capture_method,
        trigger_support,
        trigger_usage,
        formats,
        transfer,
    })
}

/// Parses the UVC function whose VideoControl interface is `vc_interface`.
pub fn parse_function(
    cfg: &ConfigView,
    vc_interface: u8,
    speed: Speed,
) -> Result<UvcFunctionDesc, TopologyError> {
    let itf = cfg
        .interface(vc_interface)
        .ok_or(TopologyError::NoSuchInterface(vc_interface))?;
    if !itf
        .alts
        .iter()
        .any(|a| a.desc.class == CC_VIDEO && a.desc.subclass == SC_VIDEOCONTROL)
    {
        return Err(TopologyError::NotUvc(vc_interface));
    }
    let vc = parse_vc(itf, speed)?;
    let mut streaming = Vec::new();
    for &n in &vc.streaming_interfaces {
        let vs = cfg.interface(n).ok_or(TopologyError::NoSuchInterface(n))?;
        streaming.push(parse_vs(vs, speed)?);
    }
    Ok(UvcFunctionDesc { vc, streaming })
}
