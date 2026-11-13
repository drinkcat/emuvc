//! Builders for UVC 1.5 class-specific descriptors (VideoControl and
//! VideoStreaming), with the exact byte layouts of the class specification.

use crate::consts::*;

/// Parses a GUID like `{32595559-0000-0010-8000-00aa00389b71}` into its
/// descriptor byte layout (first three fields little endian). Panics on
/// malformed input; meant for literals.
pub fn guid(s: &str) -> [u8; 16] {
    let hex: Vec<u8> = s.bytes().filter(|b| b.is_ascii_hexdigit()).collect();
    assert_eq!(hex.len(), 32, "bad GUID {s:?}");
    let mut b = [0u8; 16];
    for (i, o) in b.iter_mut().enumerate() {
        *o = u8::from_str_radix(std::str::from_utf8(&hex[2 * i..2 * i + 2]).unwrap(), 16).unwrap();
    }
    b[0..4].reverse();
    b[4..6].reverse();
    b[6..8].reverse();
    b
}

fn push16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn push32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

/// First `size` bytes of `controls`, little endian (zero padded beyond 4 bytes).
fn bm_bytes(controls: u32, size: u8) -> Vec<u8> {
    let le = controls.to_le_bytes();
    (0..size as usize)
        .map(|i| le.get(i).copied().unwrap_or(0))
        .collect()
}

fn cs_interface(len: usize, subtype: u8) -> Vec<u8> {
    let mut v = Vec::with_capacity(len);
    v.extend_from_slice(&[len as u8, CS_INTERFACE, subtype]);
    v
}

// ---------------------------------------------------------------------------
// VideoControl
// ---------------------------------------------------------------------------

/// Class-specific VC interface header (table 3-3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VcHeader {
    pub bcd_uvc: u16,
    pub clock_frequency: u32,
    /// `baInterfaceNr`: the VideoStreaming interfaces of the function.
    pub streaming_interfaces: Vec<u8>,
}

impl VcHeader {
    /// `total_length` is `wTotalLength` (header plus all entities).
    pub fn to_bytes(&self, total_length: u16) -> Vec<u8> {
        let mut v = cs_interface(12 + self.streaming_interfaces.len(), VC_HEADER);
        push16(&mut v, self.bcd_uvc);
        push16(&mut v, total_length);
        push32(&mut v, self.clock_frequency);
        v.push(self.streaming_interfaces.len() as u8);
        v.extend_from_slice(&self.streaming_interfaces);
        v
    }
}

/// Camera Terminal (input terminal of type `ITT_CAMERA`, table 3-6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraTerminal {
    pub id: u8,
    pub assoc_terminal: u8,
    pub i_terminal: u8,
    pub objective_focal_length_min: u16,
    pub objective_focal_length_max: u16,
    pub ocular_focal_length: u16,
    pub control_size: u8,
    pub controls: u32,
}

impl CameraTerminal {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = cs_interface(15 + self.control_size as usize, VC_INPUT_TERMINAL);
        v.push(self.id);
        push16(&mut v, ITT_CAMERA);
        v.push(self.assoc_terminal);
        v.push(self.i_terminal);
        push16(&mut v, self.objective_focal_length_min);
        push16(&mut v, self.objective_focal_length_max);
        push16(&mut v, self.ocular_focal_length);
        v.push(self.control_size);
        v.extend(bm_bytes(self.controls, self.control_size));
        v
    }
}

/// Any other input terminal (table 3-4); `extra` holds type-specific trailing bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputTerminal {
    pub id: u8,
    pub terminal_type: u16,
    pub assoc_terminal: u8,
    pub i_terminal: u8,
    pub extra: Vec<u8>,
}

impl InputTerminal {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = cs_interface(8 + self.extra.len(), VC_INPUT_TERMINAL);
        v.push(self.id);
        push16(&mut v, self.terminal_type);
        v.push(self.assoc_terminal);
        v.push(self.i_terminal);
        v.extend_from_slice(&self.extra);
        v
    }
}

/// Output terminal (table 3-5); `extra` holds type-specific trailing bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputTerminal {
    pub id: u8,
    pub terminal_type: u16,
    pub assoc_terminal: u8,
    pub source: u8,
    pub i_terminal: u8,
    pub extra: Vec<u8>,
}

impl OutputTerminal {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = cs_interface(9 + self.extra.len(), VC_OUTPUT_TERMINAL);
        v.push(self.id);
        push16(&mut v, self.terminal_type);
        v.push(self.assoc_terminal);
        v.push(self.source);
        v.push(self.i_terminal);
        v.extend_from_slice(&self.extra);
        v
    }
}

/// Selector Unit (table 3-7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectorUnit {
    pub id: u8,
    pub sources: Vec<u8>,
    pub i_selector: u8,
}

impl SelectorUnit {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = cs_interface(6 + self.sources.len(), VC_SELECTOR_UNIT);
        v.push(self.id);
        v.push(self.sources.len() as u8);
        v.extend_from_slice(&self.sources);
        v.push(self.i_selector);
        v
    }
}

/// Processing Unit (table 3-8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessingUnit {
    pub id: u8,
    pub source: u8,
    pub max_multiplier: u16,
    pub control_size: u8,
    pub controls: u32,
    pub i_processing: u8,
    /// `bmVideoStandards`; `None` selects the UVC 1.0 layout without the field.
    pub video_standards: Option<u8>,
}

impl ProcessingUnit {
    pub fn to_bytes(&self) -> Vec<u8> {
        let len = 9 + self.control_size as usize + self.video_standards.is_some() as usize;
        let mut v = cs_interface(len, VC_PROCESSING_UNIT);
        v.push(self.id);
        v.push(self.source);
        push16(&mut v, self.max_multiplier);
        v.push(self.control_size);
        v.extend(bm_bytes(self.controls, self.control_size));
        v.push(self.i_processing);
        v.extend(self.video_standards);
        v
    }
}

/// Extension Unit (table 3-10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionUnit {
    pub id: u8,
    pub guid: [u8; 16],
    pub num_controls: u8,
    pub sources: Vec<u8>,
    pub control_size: u8,
    /// `bmControls`, `control_size` bytes (padded or truncated as needed).
    pub controls: Vec<u8>,
    pub i_extension: u8,
}

impl ExtensionUnit {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = cs_interface(
            24 + self.sources.len() + self.control_size as usize,
            VC_EXTENSION_UNIT,
        );
        v.push(self.id);
        v.extend_from_slice(&self.guid);
        v.push(self.num_controls);
        v.push(self.sources.len() as u8);
        v.extend_from_slice(&self.sources);
        v.push(self.control_size);
        let mut c = self.controls.clone();
        c.resize(self.control_size as usize, 0);
        v.extend(c);
        v.push(self.i_extension);
        v
    }
}

/// Encoding Unit (table 3-9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodingUnit {
    pub id: u8,
    pub source: u8,
    pub i_encoding: u8,
    pub control_size: u8,
    pub controls: u32,
    pub runtime_controls: u32,
}

impl EncodingUnit {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = cs_interface(7 + 2 * self.control_size as usize, VC_ENCODING_UNIT);
        v.push(self.id);
        v.push(self.source);
        v.push(self.i_encoding);
        v.push(self.control_size);
        v.extend(bm_bytes(self.controls, self.control_size));
        v.extend(bm_bytes(self.runtime_controls, self.control_size));
        v
    }
}

/// Any VC entity descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VcEntity {
    Camera(CameraTerminal),
    Input(InputTerminal),
    Output(OutputTerminal),
    Selector(SelectorUnit),
    Processing(ProcessingUnit),
    Extension(ExtensionUnit),
    Encoding(EncodingUnit),
    /// Verbatim descriptor bytes.
    Raw(Vec<u8>),
}

impl VcEntity {
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            VcEntity::Camera(d) => d.to_bytes(),
            VcEntity::Input(d) => d.to_bytes(),
            VcEntity::Output(d) => d.to_bytes(),
            VcEntity::Selector(d) => d.to_bytes(),
            VcEntity::Processing(d) => d.to_bytes(),
            VcEntity::Extension(d) => d.to_bytes(),
            VcEntity::Encoding(d) => d.to_bytes(),
            VcEntity::Raw(b) => b.clone(),
        }
    }
}

/// Class-specific VC interface descriptors: header followed by `entities`,
/// with `wTotalLength` computed.
pub fn vc_block(header: &VcHeader, entities: &[VcEntity]) -> Vec<u8> {
    let body: Vec<u8> = entities.iter().flat_map(|e| e.to_bytes()).collect();
    let total = (12 + header.streaming_interfaces.len() + body.len()) as u16;
    let mut v = header.to_bytes(total);
    v.extend(body);
    v
}

/// The 5-byte class-specific interrupt endpoint descriptor (table 3-12).
pub fn vc_interrupt_endpoint(max_transfer_size: u16) -> Vec<u8> {
    let mut v = vec![5, CS_ENDPOINT, EP_INTERRUPT];
    push16(&mut v, max_transfer_size);
    v
}

// ---------------------------------------------------------------------------
// VideoStreaming
// ---------------------------------------------------------------------------

/// Class-specific VS input header (table 3-14).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VsInputHeader {
    pub endpoint: u8,
    pub info: u8,
    pub terminal_link: u8,
    pub still_capture_method: u8,
    pub trigger_support: u8,
    pub trigger_usage: u8,
    /// Size of each `bmaControls` entry.
    pub control_size: u8,
    /// `bmaControls` for all formats, concatenated (`control_size` bytes per
    /// format; padded with zeros or truncated to match the format count).
    pub format_controls: Vec<u8>,
}

impl VsInputHeader {
    fn to_bytes(&self, num_formats: usize, total_length: u16) -> Vec<u8> {
        let n = self.control_size as usize;
        let mut v = cs_interface(13 + num_formats * n, VS_INPUT_HEADER);
        v.push(num_formats as u8);
        push16(&mut v, total_length);
        v.extend_from_slice(&[
            self.endpoint,
            self.info,
            self.terminal_link,
            self.still_capture_method,
            self.trigger_support,
            self.trigger_usage,
            self.control_size,
        ]);
        let mut c = self.format_controls.clone();
        c.resize(num_formats * n, 0);
        v.extend(c);
        v
    }
}

/// Frame interval list of a frame descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameIntervals {
    Discrete(Vec<u32>),
    Continuous { min: u32, max: u32, step: u32 },
}

impl FrameIntervals {
    fn push_to(&self, v: &mut Vec<u8>) {
        match self {
            FrameIntervals::Discrete(l) => {
                for i in l {
                    push32(v, *i);
                }
            }
            FrameIntervals::Continuous { min, max, step } => {
                push32(v, *min);
                push32(v, *max);
                push32(v, *step);
            }
        }
    }

    fn type_byte(&self) -> u8 {
        match self {
            FrameIntervals::Discrete(l) => l.len() as u8,
            FrameIntervals::Continuous { .. } => 0,
        }
    }

    fn byte_len(&self) -> usize {
        match self {
            FrameIntervals::Discrete(l) => 4 * l.len(),
            FrameIntervals::Continuous { .. } => 12,
        }
    }
}

/// Frame descriptor (uncompressed, MJPEG and frame-based layouts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VsFrame {
    pub capabilities: u8,
    pub width: u16,
    pub height: u16,
    pub min_bit_rate: u32,
    pub max_bit_rate: u32,
    /// `dwMaxVideoFrameBufferSize` (uncompressed and MJPEG only).
    pub max_frame_size: u32,
    /// `dwBytesPerLine` (frame-based only).
    pub bytes_per_line: u32,
    pub default_interval: u32,
    pub intervals: FrameIntervals,
}

impl VsFrame {
    fn to_bytes(&self, kind: &VsFormatKind, index: u8) -> Vec<u8> {
        let frame_based = matches!(kind, VsFormatKind::FrameBased { .. });
        let subtype = match kind {
            VsFormatKind::Uncompressed { .. } => VS_FRAME_UNCOMPRESSED,
            VsFormatKind::Mjpeg { .. } => VS_FRAME_MJPEG,
            VsFormatKind::FrameBased { .. } => VS_FRAME_FRAME_BASED,
        };
        let len = 26 + self.intervals.byte_len();
        let mut v = cs_interface(len, subtype);
        v.extend_from_slice(&[index, self.capabilities]);
        push16(&mut v, self.width);
        push16(&mut v, self.height);
        push32(&mut v, self.min_bit_rate);
        push32(&mut v, self.max_bit_rate);
        if frame_based {
            push32(&mut v, self.default_interval);
            v.push(self.intervals.type_byte());
            push32(&mut v, self.bytes_per_line);
        } else {
            push32(&mut v, self.max_frame_size);
            push32(&mut v, self.default_interval);
            v.push(self.intervals.type_byte());
        }
        self.intervals.push_to(&mut v);
        v
    }
}

/// Format-specific part of a format descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VsFormatKind {
    Uncompressed {
        guid: [u8; 16],
        bits_per_pixel: u8,
    },
    /// `bmFlags` (bit 0: fixed size samples).
    Mjpeg {
        flags: u8,
    },
    FrameBased {
        guid: [u8; 16],
        bits_per_pixel: u8,
        variable_size: bool,
    },
}

/// A format descriptor with its frame descriptors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VsFormat {
    pub kind: VsFormatKind,
    pub default_frame: u8,
    pub aspect_x: u8,
    pub aspect_y: u8,
    pub interlace_flags: u8,
    pub copy_protect: u8,
    pub frames: Vec<VsFrame>,
}

impl VsFormat {
    fn to_bytes(&self, index: u8) -> Vec<u8> {
        let nframes = self.frames.len() as u8;
        let tail = [
            self.default_frame,
            self.aspect_x,
            self.aspect_y,
            self.interlace_flags,
            self.copy_protect,
        ];
        let mut v;
        match &self.kind {
            VsFormatKind::Uncompressed {
                guid,
                bits_per_pixel,
            } => {
                v = cs_interface(27, VS_FORMAT_UNCOMPRESSED);
                v.extend_from_slice(&[index, nframes]);
                v.extend_from_slice(guid);
                v.push(*bits_per_pixel);
                v.extend_from_slice(&tail);
            }
            VsFormatKind::Mjpeg { flags } => {
                v = cs_interface(11, VS_FORMAT_MJPEG);
                v.extend_from_slice(&[index, nframes, *flags]);
                v.extend_from_slice(&tail);
            }
            VsFormatKind::FrameBased {
                guid,
                bits_per_pixel,
                variable_size,
            } => {
                v = cs_interface(28, VS_FORMAT_FRAME_BASED);
                v.extend_from_slice(&[index, nframes]);
                v.extend_from_slice(guid);
                v.push(*bits_per_pixel);
                v.extend_from_slice(&tail);
                v.push(*variable_size as u8);
            }
        }
        v
    }
}

/// Color matching descriptor (table 3-19).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColorMatching {
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
}

impl ColorMatching {
    pub fn to_bytes(&self) -> Vec<u8> {
        vec![
            6,
            CS_INTERFACE,
            VS_COLORFORMAT,
            self.primaries,
            self.transfer,
            self.matrix,
        ]
    }
}

/// Still image frame descriptor (table 3-18).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VsStillFrame {
    pub endpoint: u8,
    /// `(wWidth, wHeight)` patterns.
    pub sizes: Vec<(u16, u16)>,
    pub compression: Vec<u8>,
}

impl VsStillFrame {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = cs_interface(
            6 + 4 * self.sizes.len() + self.compression.len(),
            VS_STILL_IMAGE_FRAME,
        );
        v.push(self.endpoint);
        v.push(self.sizes.len() as u8);
        for (w, h) in &self.sizes {
            push16(&mut v, *w);
            push16(&mut v, *h);
        }
        v.push(self.compression.len() as u8);
        v.extend_from_slice(&self.compression);
        v
    }
}

/// A format with its optional still image and color matching descriptors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VsFormatEntry {
    pub format: VsFormat,
    pub still: Option<VsStillFrame>,
    pub color: Option<ColorMatching>,
}

impl VsFormatEntry {
    pub fn new(format: VsFormat) -> Self {
        VsFormatEntry {
            format,
            still: None,
            color: None,
        }
    }

    pub fn with_color(mut self, color: ColorMatching) -> Self {
        self.color = Some(color);
        self
    }

    pub fn with_still(mut self, still: VsStillFrame) -> Self {
        self.still = Some(still);
        self
    }
}

/// Class-specific VS interface descriptors: input header, then each format
/// followed by its frames, still image frame and color matching descriptor.
/// `bFormatIndex`/`bFrameIndex` are assigned from 1 in order; `bNumFormats`,
/// `bNumFrameDescriptors` and `wTotalLength` are computed.
pub fn vs_block(header: &VsInputHeader, formats: &[VsFormatEntry]) -> Vec<u8> {
    let mut body = Vec::new();
    for (fi, entry) in formats.iter().enumerate() {
        let f = &entry.format;
        body.extend(f.to_bytes(fi as u8 + 1));
        for (i, fr) in f.frames.iter().enumerate() {
            body.extend(fr.to_bytes(&f.kind, i as u8 + 1));
        }
        if let Some(s) = &entry.still {
            body.extend(s.to_bytes());
        }
        if let Some(c) = &entry.color {
            body.extend(c.to_bytes());
        }
    }
    let hdr_len = 13 + formats.len() * header.control_size as usize;
    let mut v = header.to_bytes(formats.len(), (hdr_len + body.len()) as u16);
    v.extend(body);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_and_pu_layout() {
        let ct = CameraTerminal {
            id: 1,
            assoc_terminal: 0,
            i_terminal: 0,
            objective_focal_length_min: 0,
            objective_focal_length_max: 0,
            ocular_focal_length: 0,
            control_size: 3,
            controls: 0x24000e,
        }
        .to_bytes();
        assert_eq!(
            ct,
            [
                0x12, 0x24, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0x0e, 0, 0x24
            ]
        );
        let pu = ProcessingUnit {
            id: 2,
            source: 1,
            max_multiplier: 0x4000,
            control_size: 3,
            controls: 0x157f,
            i_processing: 0,
            video_standards: None,
        };
        assert_eq!(pu.to_bytes().len(), 12);
        assert_eq!(
            ProcessingUnit {
                video_standards: Some(0),
                ..pu
            }
            .to_bytes()
            .len(),
            13
        );
    }

    #[test]
    fn vs_block_indices_and_lengths() {
        let frame = VsFrame {
            capabilities: 0,
            width: 640,
            height: 480,
            min_bit_rate: 1,
            max_bit_rate: 2,
            max_frame_size: 3,
            bytes_per_line: 4,
            default_interval: 333333,
            intervals: FrameIntervals::Continuous {
                min: 1,
                max: 2,
                step: 3,
            },
        };
        let fmt = VsFormat {
            kind: VsFormatKind::FrameBased {
                guid: [0; 16],
                bits_per_pixel: 12,
                variable_size: true,
            },
            default_frame: 1,
            aspect_x: 0,
            aspect_y: 0,
            interlace_flags: 0,
            copy_protect: 0,
            frames: vec![frame.clone(), frame],
        };
        let hdr = VsInputHeader {
            endpoint: 0x81,
            info: 0,
            terminal_link: 2,
            still_capture_method: 0,
            trigger_support: 0,
            trigger_usage: 0,
            control_size: 2,
            format_controls: vec![],
        };
        let b = vs_block(&hdr, &[VsFormatEntry::new(fmt)]);
        assert_eq!(b.len(), 15 + 28 + 2 * 38);
        assert_eq!(u16::from_le_bytes([b[4], b[5]]) as usize, b.len());
        assert_eq!(b[15 + 3], 1); // bFormatIndex
        assert_eq!(b[15 + 28 + 3], 1); // bFrameIndex
        assert_eq!(b[15 + 28 + 38 + 3], 2);
        assert_eq!(b[15 + 28 + 22..15 + 28 + 26], [4, 0, 0, 0]); // dwBytesPerLine
    }
}
