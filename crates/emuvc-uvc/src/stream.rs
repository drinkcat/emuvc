//! VideoStreaming interface: probe/commit negotiation and payload generation.
//!
//! Frames are produced on the bus clock: frame `k` of a stream is captured
//! at `start + k * dwFrameInterval` (plus an optional startup delay) and is
//! then sent as UVC payloads — one per isochronous service interval, or one
//! per bulk transfer. If the host doesn't read fast enough, stale frames are
//! skipped like a real sensor would, so timing bugs on the host side show up
//! as dropped frames, not as an artificially perfect stream.

use log::{debug, info, warn};
use serde::{Deserialize, Serialize};

use emuvc_usb::device::UFRAME_NS;
use emuvc_usb::{BusTime, Ctx};
use emuvc_video::{FrameSource, FrameSpec, PixelFormat};

use crate::consts::*;
use crate::topology::{FormatKind, Frame, Transfer, VideoStreaming};

/// Video probe and commit control (UVC 1.5 table 4-75).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamingControl {
    pub hint: u16,
    pub format_index: u8,
    pub frame_index: u8,
    pub frame_interval: u32,
    pub key_frame_rate: u16,
    pub p_frame_rate: u16,
    pub comp_quality: u16,
    pub comp_window_size: u16,
    pub delay: u16,
    pub max_video_frame_size: u32,
    pub max_payload_transfer_size: u32,
    pub clock_frequency: u32,
    pub framing_info: u8,
    pub preferred_version: u8,
    pub min_version: u8,
    pub max_version: u8,
    pub usage: u8,
    pub bit_depth_luma: u8,
    pub settings: u8,
    pub max_number_of_ref_frames_plus1: u8,
    pub rate_control_modes: u16,
    pub layout_per_stream: u64,
}

impl StreamingControl {
    /// Structure length for a given bcdUVC: 26 (1.0), 34 (1.1), 48 (1.5).
    pub fn len_for(bcd_uvc: u16) -> usize {
        match bcd_uvc {
            ..0x0110 => 26,
            0x0110..0x0150 => 34,
            _ => 48,
        }
    }

    pub fn to_bytes(&self, len: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(48);
        v.extend_from_slice(&self.hint.to_le_bytes());
        v.extend_from_slice(&[self.format_index, self.frame_index]);
        v.extend_from_slice(&self.frame_interval.to_le_bytes());
        for w in [
            self.key_frame_rate,
            self.p_frame_rate,
            self.comp_quality,
            self.comp_window_size,
            self.delay,
        ] {
            v.extend_from_slice(&w.to_le_bytes());
        }
        v.extend_from_slice(&self.max_video_frame_size.to_le_bytes());
        v.extend_from_slice(&self.max_payload_transfer_size.to_le_bytes());
        v.extend_from_slice(&self.clock_frequency.to_le_bytes());
        v.extend_from_slice(&[
            self.framing_info,
            self.preferred_version,
            self.min_version,
            self.max_version,
        ]);
        v.extend_from_slice(&[
            self.usage,
            self.bit_depth_luma,
            self.settings,
            self.max_number_of_ref_frames_plus1,
        ]);
        v.extend_from_slice(&self.rate_control_modes.to_le_bytes());
        v.extend_from_slice(&self.layout_per_stream.to_le_bytes());
        v.resize(len, 0);
        v
    }

    /// Lenient decode: missing trailing fields read as zero.
    pub fn from_bytes(b: &[u8]) -> Self {
        let mut p = [0u8; 48];
        let n = b.len().min(48);
        p[..n].copy_from_slice(&b[..n]);
        let w = |i: usize| u16::from_le_bytes([p[i], p[i + 1]]);
        let d = |i: usize| u32::from_le_bytes([p[i], p[i + 1], p[i + 2], p[i + 3]]);
        StreamingControl {
            hint: w(0),
            format_index: p[2],
            frame_index: p[3],
            frame_interval: d(4),
            key_frame_rate: w(8),
            p_frame_rate: w(10),
            comp_quality: w(12),
            comp_window_size: w(14),
            delay: w(16),
            max_video_frame_size: d(18),
            max_payload_transfer_size: d(22),
            clock_frequency: d(26),
            framing_info: p[30],
            preferred_version: p[31],
            min_version: p[32],
            max_version: p[33],
            usage: p[34],
            bit_depth_luma: p[35],
            settings: p[36],
            max_number_of_ref_frames_plus1: p[37],
            rate_control_modes: w(38),
            layout_per_stream: u64::from_le_bytes(p[40..48].try_into().unwrap()),
        }
    }
}

/// What the device sends in an isochronous interval with no frame data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdlePayload {
    /// A header-only payload (common).
    #[default]
    Header,
    /// A zero-length packet.
    Zlp,
}

/// Field overrides applied to a probe/commit structure as the device
/// reports it for one request type (unset fields keep the negotiated value).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProbeOverride {
    pub hint: Option<u16>,
    pub format_index: Option<u8>,
    pub frame_index: Option<u8>,
    pub frame_interval: Option<u32>,
    pub comp_quality: Option<u16>,
    pub delay: Option<u16>,
    pub max_video_frame_size: Option<u32>,
    pub max_payload_transfer_size: Option<u32>,
    pub clock_frequency: Option<u32>,
    pub framing_info: Option<u8>,
    /// Reply with exactly these bytes instead.
    pub raw: Option<Vec<u8>>,
}

impl ProbeOverride {
    fn apply(&self, c: &mut StreamingControl) {
        macro_rules! set {
            ($($f:ident),*) => { $( if let Some(v) = self.$f { c.$f = v; } )* };
        }
        set!(
            hint,
            format_index,
            frame_index,
            frame_interval,
            comp_quality,
            delay,
            max_video_frame_size,
            max_payload_transfer_size,
            clock_frequency,
            framing_info
        );
    }
}

/// Class-level misbehaviour knobs for a streaming interface. All default to
/// a well-behaved device.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StreamQuirks {
    pub idle: IdlePayload,
    /// Payload header length (default 12 = PTS + SCR; 2 = none, 6 = PTS).
    /// Longer headers carry metadata bytes after the SCR.
    pub header_len: Option<u8>,
    /// Set the PTS and SCR flags whatever the header length (bogus headers).
    pub lie_pts_scr: bool,
    /// Never set EOF (hosts must rely on the FID toggle).
    pub no_eof: bool,
    /// Never toggle FID.
    pub no_fid_toggle: bool,
    /// Set the ERR bit on every Nth frame's payloads.
    pub err_every: Option<u32>,
    /// Silently skip every Nth frame.
    pub drop_every: Option<u32>,
    /// Send only the first half of every Nth frame (then EOF).
    pub truncate_every: Option<u32>,
    /// Restrict err/drop/truncate_every to the first N frames of each stream
    /// (e.g. cameras sending garbage while the sensor starts).
    pub only_first_frames: Option<u64>,
    /// Report this dwMaxPayloadTransferSize in every probe/commit reply (e.g. 0).
    pub max_payload_transfer_size: Option<u32>,
    /// Per-request overrides of the probe/commit replies.
    pub probe_cur: Option<ProbeOverride>,
    pub probe_min: Option<ProbeOverride>,
    pub probe_max: Option<ProbeOverride>,
    pub probe_def: Option<ProbeOverride>,
    pub commit_cur: Option<ProbeOverride>,
    /// Bulk: device payload size (default: dwMaxPayloadTransferSize).
    pub bulk_payload_size: Option<u32>,
    /// Probe/commit length regardless of bcdUVC (e.g. 26 or 34 on a 1.5 device).
    pub probe_len: Option<u16>,
    /// STALL probe/commit requests whose wLength isn't the probe length.
    pub strict_probe_len: bool,
    /// Negotiate the default format/frame whatever the host asks for.
    pub ignore_probe_format: bool,
    /// Delay before the first frame after streaming starts.
    pub startup_ms: u32,
    /// Device clock (PTS/SCR) misbehaviour.
    pub clock: ClockQuirks,
}

/// Payload header timestamps (UVC 1.5 2.4.3.3): PTS is the device clock at
/// capture, SCR the device clock and the 1 kHz USB SOF counter when the
/// payload is sent. These knobs break the assumptions hosts make when they
/// map the device clock to their own (e.g. uvcvideo's hardware timestamps):
///
/// ```toml
/// uvc = { clock = { start_ticks = 0xfd000000, pts_jitter = 20000 } }  # wraps after ~1 s at 48 MHz
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClockQuirks {
    /// Device clock value at bus time 0 (the 32-bit clock wraps from 0xffffffff to 0).
    pub start_ticks: u32,
    /// The clock runs this many parts per million faster (negative: slower) than
    /// dwClockFrequency says.
    pub drift_ppm: i32,
    /// PTS off by a pseudo-random amount within +-N ticks (same for every payload of a frame).
    pub pts_jitter: u32,
    /// Every Nth frame's PTS is one frame interval in the past.
    pub pts_backwards_every: Option<u32>,
    /// The SCR's clock field never advances (its SOF counter does).
    pub scr_stuck: bool,
    /// PTS only in the first payload of each frame: the header is 4 bytes shorter after that.
    pub pts_first_only: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamStats {
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub stills_sent: u64,
    pub payloads: u64,
}

/// Still probe/commit control (UVC 1.5 table 4-77).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StillControl {
    pub format_index: u8,
    pub frame_index: u8,
    pub compression_index: u8,
    pub max_video_frame_size: u32,
    pub max_payload_transfer_size: u32,
}

impl StillControl {
    pub const LEN: usize = 11;

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = vec![self.format_index, self.frame_index, self.compression_index];
        v.extend_from_slice(&self.max_video_frame_size.to_le_bytes());
        v.extend_from_slice(&self.max_payload_transfer_size.to_le_bytes());
        v
    }

    pub fn from_bytes(b: &[u8]) -> Self {
        let mut p = [0u8; 11];
        p[..b.len().min(11)].copy_from_slice(&b[..b.len().min(11)]);
        StillControl {
            format_index: p[0],
            frame_index: p[1],
            compression_index: p[2],
            max_video_frame_size: u32::from_le_bytes(p[3..7].try_into().unwrap()),
            max_payload_transfer_size: u32::from_le_bytes(p[7..11].try_into().unwrap()),
        }
    }
}

struct FrameTx {
    index: u64,
    data: Vec<u8>,
    offset: usize,
    pts: u32,
    error: bool,
    still: bool,
}

struct Stream {
    start_ns: u64,
    interval_ns: u64,
    spec: FrameSpec,
    /// Bytes per payload: iso capacity of the alt setting, or bulk payload size.
    payload_size: usize,
    next_frame: u64,
    cur: Option<FrameTx>,
    fid: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Report {
    ProbeCur,
    ProbeMin,
    ProbeMax,
    ProbeDef,
    CommitCur,
}

/// Still image trigger values (VS_STILL_IMAGE_TRIGGER_CONTROL).
const TRIGGER_NORMAL: u8 = 0;
const TRIGGER_STILL: u8 = 1;
const TRIGGER_STILL_BULK: u8 = 2;
const TRIGGER_ABORT: u8 = 3;

/// One VideoStreaming interface.
pub struct VsInterface {
    pub desc: VideoStreaming,
    bcd_uvc: u16,
    clock_frequency: u32,
    pub quirks: StreamQuirks,
    probe: StreamingControl,
    commit: Option<StreamingControl>,
    still_probe: StillControl,
    still_commit: Option<StillControl>,
    /// Pending still trigger (method 2: TRIGGER_STILL, method 3: TRIGGER_STILL_BULK).
    trigger: u8,
    /// Method 3: still image being sent on the still bulk endpoint.
    still_tx: Option<FrameTx>,
    alt: u8,
    pub error_code: u8,
    stream: Option<Stream>,
    source: Box<dyn FrameSource>,
    pub stats: StreamStats,
    name: String,
}

/// Frame generator format for a UVC format, if emuvc can produce it.
pub fn pixel_format(kind: &FormatKind) -> Option<PixelFormat> {
    match kind {
        FormatKind::Uncompressed { guid, .. } => PixelFormat::from_guid(guid),
        FormatKind::Mjpeg => Some(PixelFormat::Mjpeg),
        FormatKind::FrameBased { guid, .. } if &guid[..4] == b"MJPG" => Some(PixelFormat::Mjpeg),
        FormatKind::FrameBased { guid, .. } => PixelFormat::from_guid(guid),
        _ => None,
    }
}

impl VsInterface {
    pub fn new(
        desc: VideoStreaming,
        bcd_uvc: u16,
        clock_frequency: u32,
        source: Box<dyn FrameSource>,
        quirks: StreamQuirks,
    ) -> Self {
        let name = format!("VS{}", desc.interface);
        let mut vs = VsInterface {
            desc,
            bcd_uvc,
            clock_frequency,
            quirks,
            probe: StreamingControl::default(),
            commit: None,
            still_probe: StillControl::default(),
            still_commit: None,
            trigger: TRIGGER_NORMAL,
            still_tx: None,
            alt: 0,
            error_code: 0,
            stream: None,
            source,
            stats: StreamStats::default(),
            name,
        };
        vs.probe = vs.default_probe();
        vs.still_probe = vs
            .negotiate_still(
                &StillControl {
                    format_index: 1,
                    frame_index: 1,
                    ..Default::default()
                },
                false,
            )
            .unwrap_or_default();
        vs
    }

    pub fn interface(&self) -> u8 {
        self.desc.interface
    }

    pub fn is_streaming(&self) -> bool {
        self.stream.is_some()
    }

    /// Bulk endpoint of still capture method 3, if any.
    pub fn still_endpoint(&self) -> Option<u8> {
        self.desc
            .formats
            .iter()
            .filter_map(|f| f.still.as_ref())
            .map(|s| s.endpoint)
            .find(|&e| e != 0)
    }

    fn probe_len(&self) -> usize {
        self.quirks
            .probe_len
            .map_or_else(|| StreamingControl::len_for(self.bcd_uvc), |l| l as usize)
    }

    fn header_len(&self) -> usize {
        self.quirks.header_len.map_or(12, |n| n.max(2) as usize)
    }

    fn default_probe(&self) -> StreamingControl {
        let req = StreamingControl {
            format_index: 1,
            ..Default::default()
        };
        self.negotiate(&req, false).unwrap_or_default()
    }

    /// Size of one frame of a format.
    fn frame_size(kind: &FormatKind, width: u16, height: u16, max_frame_size: u32) -> u32 {
        let px = width as u32 * height as u32;
        match kind {
            FormatKind::Uncompressed { bits_per_pixel, .. } => {
                (px * *bits_per_pixel as u32).div_ceil(8)
            }
            // Variable-size frame-based formats report dwBytesPerLine = 0.
            FormatKind::FrameBased { bits_per_pixel, .. } if max_frame_size == 0 => {
                (px * (*bits_per_pixel).max(16) as u32).div_ceil(8)
            }
            _ if max_frame_size == 0 => px * 2,
            _ => max_frame_size,
        }
    }

    /// Picks the payload size for a format/frame/interval: for isochronous
    /// streams the smallest alternate setting that carries the required
    /// bandwidth (the largest if none does); for bulk 32 KiB payloads.
    fn payload_size(&self, kind: &FormatKind, frame: &Frame, interval: u32) -> u32 {
        match &self.desc.transfer {
            Transfer::Isochronous { alts } => {
                let fps = 10_000_000f64 / interval.max(1) as f64;
                let bytes_per_s = match kind {
                    FormatKind::Uncompressed { .. } => {
                        Self::frame_size(kind, frame.width, frame.height, frame.max_frame_size)
                            as f64
                            * fps
                    }
                    _ => frame.max_bit_rate as f64 / 8.0,
                };
                // Payload capacity per second of an alt setting, after headers.
                let hlen = self.header_len();
                let capacity = |a: &crate::topology::IsoAlt| {
                    a.bytes_per_interval.saturating_sub(hlen) as f64 * 8000.0
                        / a.interval_uframes.max(1) as f64
                };
                let mut usable: Vec<&crate::topology::IsoAlt> = alts
                    .iter()
                    .filter(|a| a.bytes_per_interval > hlen)
                    .collect();
                usable.sort_by(|a, b| capacity(a).total_cmp(&capacity(b)));
                usable
                    .iter()
                    .find(|a| capacity(a) >= bytes_per_s)
                    .or(usable.last())
                    .map_or(0, |a| a.bytes_per_interval) as u32
            }
            Transfer::Bulk { .. } => 32 * 1024,
        }
    }

    /// Fills in a probe/commit structure. With `strict`, out-of-range
    /// indexes are an error (COMMIT); otherwise they snap to defaults (PROBE).
    fn negotiate(&self, req: &StreamingControl, strict: bool) -> Result<StreamingControl, u8> {
        let want_fmt = if self.quirks.ignore_probe_format {
            1
        } else {
            req.format_index
        };
        let format = match self.desc.format(want_fmt) {
            Some(f) => f,
            None if strict => return Err(ERR_OUT_OF_RANGE),
            None => self.desc.formats.first().ok_or(ERR_OUT_OF_RANGE)?,
        };
        let want_frame = if self.quirks.ignore_probe_format {
            format.default_frame
        } else {
            req.frame_index
        };
        let frame = match format.frame(want_frame) {
            Some(f) => f,
            None if strict => return Err(ERR_OUT_OF_RANGE),
            None => format
                .frame(format.default_frame)
                .or(format.frames.first())
                .ok_or(ERR_OUT_OF_RANGE)?,
        };
        let interval = if req.frame_interval == 0 {
            frame.default_interval
        } else {
            frame.nearest_interval(req.frame_interval)
        };
        let mut out = StreamingControl {
            hint: req.hint,
            format_index: format.index,
            frame_index: frame.index,
            frame_interval: interval,
            key_frame_rate: req.key_frame_rate,
            p_frame_rate: req.p_frame_rate,
            comp_quality: req.comp_quality,
            comp_window_size: req.comp_window_size,
            delay: 0,
            max_video_frame_size: Self::frame_size(
                &format.kind,
                frame.width,
                frame.height,
                frame.max_frame_size,
            ),
            max_payload_transfer_size: self.payload_size(&format.kind, frame, interval),
            ..Default::default()
        };
        if self.bcd_uvc >= 0x0110 {
            out.clock_frequency = self.clock_frequency;
            out.framing_info = FRAMING_FID | FRAMING_EOF;
        }
        if let FormatKind::Uncompressed { .. } = format.kind {
            out.bit_depth_luma = 8;
        }
        Ok(out)
    }

    /// The structure as the device reports it (quirks applied).
    fn report(&self, c: &StreamingControl, which: Report) -> Vec<u8> {
        let mut c = c.clone();
        if let Some(m) = self.quirks.max_payload_transfer_size {
            c.max_payload_transfer_size = m;
        }
        let o = match which {
            Report::ProbeCur => &self.quirks.probe_cur,
            Report::ProbeMin => &self.quirks.probe_min,
            Report::ProbeMax => &self.quirks.probe_max,
            Report::ProbeDef => &self.quirks.probe_def,
            Report::CommitCur => &self.quirks.commit_cur,
        };
        if let Some(o) = o {
            if let Some(raw) = &o.raw {
                return raw.clone();
            }
            o.apply(&mut c);
        }
        c.to_bytes(self.probe_len())
    }

    /// Negotiates a still probe/commit structure.
    fn negotiate_still(&self, req: &StillControl, strict: bool) -> Result<StillControl, u8> {
        let with_still = |i: u8| self.desc.format(i).filter(|f| f.still.is_some());
        let format = match with_still(req.format_index) {
            Some(f) => f,
            None if strict => return Err(ERR_OUT_OF_RANGE),
            None => self
                .desc
                .formats
                .iter()
                .find(|f| f.still.is_some())
                .ok_or(ERR_INVALID_CONTROL)?,
        };
        let still = format.still.as_ref().unwrap();
        let n = still.sizes.len() as u8;
        if strict && (req.frame_index == 0 || req.frame_index > n) {
            return Err(ERR_OUT_OF_RANGE);
        }
        let frame_index = req.frame_index.clamp(1, n.max(1));
        let (w, h) = still
            .sizes
            .get(frame_index as usize - 1)
            .copied()
            .unwrap_or((0, 0));
        let nc = still.compression.len() as u8;
        let compression_index = if nc == 0 {
            0
        } else {
            req.compression_index.clamp(1, nc)
        };
        let max_payload = match (&self.desc.transfer, still.endpoint) {
            (_, 0) => {
                self.commit
                    .as_ref()
                    .unwrap_or(&self.probe)
                    .max_payload_transfer_size
            }
            _ => 32 * 1024,
        };
        Ok(StillControl {
            format_index: format.index,
            frame_index,
            compression_index,
            max_video_frame_size: Self::frame_size(&format.kind, w, h, 0),
            max_payload_transfer_size: max_payload,
        })
    }

    /// Class request to this interface. Returns data or a request error code.
    pub fn control(
        &mut self,
        cx: &mut Ctx,
        request: u8,
        selector: u8,
        data: &[u8],
        length: u16,
    ) -> Result<Vec<u8>, u8> {
        let probe_like = matches!(selector, VS_PROBE_CONTROL | VS_COMMIT_CONTROL)
            && !matches!(request, GET_LEN | GET_INFO);
        if probe_like && self.quirks.strict_probe_len && length as usize != self.probe_len() {
            debug!(
                "{}: probe/commit with wLength {length}, expected {}",
                self.name,
                self.probe_len()
            );
            return Err(ERR_INVALID_REQUEST);
        }
        match (selector, request) {
            (VS_PROBE_CONTROL, SET_CUR) => {
                let req = StreamingControl::from_bytes(data);
                self.probe = self.negotiate(&req, false)?;
                debug!("{}: probe {:?} -> {:?}", self.name, req, self.probe);
                Ok(Vec::new())
            }
            (VS_PROBE_CONTROL, GET_CUR) => Ok(self.report(&self.probe, Report::ProbeCur)),
            (VS_PROBE_CONTROL, GET_DEF) => Ok(self.report(&self.default_probe(), Report::ProbeDef)),
            (VS_PROBE_CONTROL, GET_MIN | GET_MAX) => {
                let mut c = self.probe.clone();
                if let Some(frame) = self
                    .desc
                    .format(c.format_index)
                    .and_then(|f| f.frame(c.frame_index))
                {
                    let (min, max) = match &frame.intervals {
                        crate::topology::Intervals::Discrete(l) => (
                            l.iter().copied().min().unwrap_or(c.frame_interval),
                            l.iter().copied().max().unwrap_or(c.frame_interval),
                        ),
                        crate::topology::Intervals::Continuous { min, max, .. } => (*min, *max),
                    };
                    let is_max = request == GET_MAX;
                    c.frame_interval = if is_max { max } else { min };
                    c.comp_quality = if is_max { 10000 } else { 0 };
                    c.key_frame_rate = if is_max { 0xffff } else { 0 };
                    c.p_frame_rate = if is_max { 0xffff } else { 0 };
                }
                Ok(self.report(
                    &c,
                    if request == GET_MAX {
                        Report::ProbeMax
                    } else {
                        Report::ProbeMin
                    },
                ))
            }
            (VS_PROBE_CONTROL | VS_COMMIT_CONTROL, GET_LEN) => {
                Ok((self.probe_len() as u16).to_le_bytes().to_vec())
            }
            (VS_PROBE_CONTROL | VS_COMMIT_CONTROL, GET_INFO) => Ok(vec![INFO_GET | INFO_SET]),
            (VS_COMMIT_CONTROL, SET_CUR) => {
                let req = StreamingControl::from_bytes(data);
                let c = self.negotiate(&req, true)?;
                info!(
                    "{}: commit format {} frame {} interval {}",
                    self.name, c.format_index, c.frame_index, c.frame_interval
                );
                self.commit = Some(c);
                if matches!(self.desc.transfer, Transfer::Bulk { .. }) {
                    self.start(cx);
                }
                Ok(Vec::new())
            }
            (VS_COMMIT_CONTROL, GET_CUR) => Ok(self.report(
                self.commit.as_ref().unwrap_or(&self.probe),
                Report::CommitCur,
            )),
            (VS_STILL_PROBE_CONTROL, SET_CUR) => {
                self.still_probe = self.negotiate_still(&StillControl::from_bytes(data), false)?;
                Ok(Vec::new())
            }
            (VS_STILL_PROBE_CONTROL, GET_CUR) => Ok(self.still_probe.to_bytes()),
            (VS_STILL_PROBE_CONTROL, GET_MIN | GET_DEF) => {
                let req = StillControl {
                    format_index: self.still_probe.format_index,
                    frame_index: 1,
                    compression_index: 1,
                    ..Default::default()
                };
                Ok(self.negotiate_still(&req, false)?.to_bytes())
            }
            (VS_STILL_PROBE_CONTROL, GET_MAX) => {
                let req = StillControl {
                    format_index: self.still_probe.format_index,
                    frame_index: u8::MAX,
                    compression_index: u8::MAX,
                    ..Default::default()
                };
                Ok(self.negotiate_still(&req, false)?.to_bytes())
            }
            (VS_STILL_COMMIT_CONTROL, SET_CUR) => {
                let c = self.negotiate_still(&StillControl::from_bytes(data), true)?;
                info!(
                    "{}: still commit format {} frame {}",
                    self.name, c.format_index, c.frame_index
                );
                self.still_commit = Some(c);
                Ok(Vec::new())
            }
            (VS_STILL_COMMIT_CONTROL, GET_CUR) => {
                Ok(self.still_commit.unwrap_or(self.still_probe).to_bytes())
            }
            (VS_STILL_PROBE_CONTROL | VS_STILL_COMMIT_CONTROL, GET_LEN) => {
                Ok((StillControl::LEN as u16).to_le_bytes().to_vec())
            }
            (VS_STILL_PROBE_CONTROL | VS_STILL_COMMIT_CONTROL, GET_INFO) => {
                Ok(vec![INFO_GET | INFO_SET])
            }
            (VS_STILL_IMAGE_TRIGGER_CONTROL, SET_CUR) => self
                .trigger_still(cx, data.first().copied().unwrap_or(0))
                .map(|_| Vec::new()),
            (VS_STILL_IMAGE_TRIGGER_CONTROL, GET_CUR) => Ok(vec![self.trigger]),
            (VS_STILL_IMAGE_TRIGGER_CONTROL, GET_INFO) => Ok(vec![INFO_GET | INFO_SET]),
            (VS_STREAM_ERROR_CODE_CONTROL, GET_CUR) => Ok(vec![self.error_code]),
            (VS_STREAM_ERROR_CODE_CONTROL, GET_INFO) => Ok(vec![INFO_GET]),
            (VS_PROBE_CONTROL..=VS_STREAM_ERROR_CODE_CONTROL, _) => Err(ERR_INVALID_REQUEST),
            _ => Err(ERR_INVALID_CONTROL),
        }
    }

    fn trigger_still(&mut self, cx: &mut Ctx, value: u8) -> Result<(), u8> {
        match (value, self.desc.still_capture_method) {
            (TRIGGER_NORMAL | TRIGGER_ABORT, _) => {
                self.trigger = TRIGGER_NORMAL;
                self.still_tx = None;
                Ok(())
            }
            (TRIGGER_STILL, 2 | 3) if self.stream.is_some() => {
                info!("{}: still image triggered (in stream)", self.name);
                self.trigger = TRIGGER_STILL;
                Ok(())
            }
            (TRIGGER_STILL_BULK, 3) => {
                info!("{}: still image triggered (bulk pipe)", self.name);
                self.trigger = TRIGGER_STILL_BULK;
                self.still_tx = self.render_still(cx.now_ns());
                Ok(())
            }
            (TRIGGER_STILL | TRIGGER_STILL_BULK, _) => Err(ERR_WRONG_STATE),
            _ => Err(ERR_OUT_OF_RANGE),
        }
    }

    /// Renders the committed still image.
    fn render_still(&mut self, now_ns: u64) -> Option<FrameTx> {
        let c = self.still_commit.unwrap_or(self.still_probe);
        let format = self.desc.format(c.format_index)?;
        let (w, h) = *format
            .still
            .as_ref()?
            .sizes
            .get(c.frame_index.max(1) as usize - 1)?;
        let spec = FrameSpec {
            format: pixel_format(&format.kind)?,
            width: w,
            height: h,
        };
        let data = self.source.render(&spec, 0, now_ns);
        let pts = self.ticks(now_ns);
        Some(FrameTx {
            index: 0,
            data,
            offset: 0,
            pts,
            error: false,
            still: true,
        })
    }

    pub fn set_alt(&mut self, cx: &mut Ctx, alt: u8) {
        self.alt = alt;
        match (&self.desc.transfer, alt) {
            (_, 0) => self.stop(),
            (Transfer::Isochronous { .. }, _) => self.start(cx),
            (Transfer::Bulk { .. }, _) => {}
        }
    }

    pub fn reset(&mut self) {
        self.stop();
        self.alt = 0;
        self.commit = None;
        self.still_commit = None;
        self.trigger = TRIGGER_NORMAL;
        self.still_tx = None;
        self.probe = self.default_probe();
        self.error_code = 0;
    }

    pub fn clear_halt(&mut self, ep: u8) {
        // UVC 1.5 §2.4.3.2.4: on bulk, CLEAR_FEATURE(ENDPOINT_HALT) stops the stream.
        if ep == self.desc.endpoint && matches!(self.desc.transfer, Transfer::Bulk { .. }) {
            self.stop();
        }
    }

    fn start(&mut self, cx: &mut Ctx) {
        let c = match &self.commit {
            Some(c) => c.clone(),
            None => {
                warn!(
                    "{}: streaming without a committed format, using the probe",
                    self.name
                );
                self.probe.clone()
            }
        };
        let Some(format) = self.desc.format(c.format_index) else {
            return;
        };
        let Some(frame) = format.frame(c.frame_index) else {
            return;
        };
        let Some(pf) = pixel_format(&format.kind) else {
            warn!(
                "{}: format {:?} has no frame generator",
                self.name, format.kind
            );
            return;
        };
        let payload_size = match &self.desc.transfer {
            Transfer::Isochronous { alts } => alts
                .iter()
                .find(|a| a.alt == self.alt)
                .map_or(0, |a| a.bytes_per_interval),
            Transfer::Bulk { .. } => {
                self.quirks
                    .bulk_payload_size
                    .unwrap_or(c.max_payload_transfer_size) as usize
            }
        };
        info!(
            "{}: start streaming {}x{} {:?} @ {:.2} fps, {} bytes/payload",
            self.name,
            frame.width,
            frame.height,
            pf,
            1e7 / c.frame_interval.max(1) as f64,
            payload_size
        );
        self.stats = StreamStats::default();
        self.stream = Some(Stream {
            start_ns: cx.now_ns() + self.quirks.startup_ms as u64 * 1_000_000,
            interval_ns: c.frame_interval.max(1) as u64 * 100,
            spec: FrameSpec {
                format: pf,
                width: frame.width,
                height: frame.height,
            },
            payload_size,
            next_frame: 0,
            cur: None,
            fid: false,
        });
    }

    fn stop(&mut self) {
        if self.stream.take().is_some() {
            info!("{}: stop streaming ({:?})", self.name, self.stats);
        }
        if self.trigger == TRIGGER_STILL {
            self.trigger = TRIGGER_NORMAL;
        }
    }

    /// Bus time at which the next frame becomes available (bulk pacing).
    pub fn next_wakeup(&self) -> Option<BusTime> {
        let st = self.stream.as_ref()?;
        if st.cur.is_some() {
            return None;
        }
        let t = st.start_ns + st.next_frame * st.interval_ns;
        Some(t.div_ceil(UFRAME_NS))
    }

    /// Device clock at bus time `ns` (wraps at 32 bits).
    fn ticks(&self, ns: u64) -> u32 {
        let c = &self.quirks.clock;
        let rate = self.clock_frequency as i128 * (1_000_000 + c.drift_ppm as i128);
        let t = ns as i128 * rate / 1_000_000_000_000_000;
        (t as u32).wrapping_add(c.start_ticks)
    }

    /// PTS of frame `index` captured at `ns`, with the clock quirks.
    fn pts(&self, index: u64, ns: u64, interval_ns: u64) -> u32 {
        let c = &self.quirks.clock;
        let mut ns = ns;
        if c.pts_backwards_every
            .is_some_and(|n| n > 0 && index % n as u64 == n as u64 - 1)
        {
            ns = ns.saturating_sub(interval_ns);
        }
        let mut pts = self.ticks(ns);
        if c.pts_jitter > 0 {
            // Deterministic per frame: Knuth's multiplicative hash of the index.
            let span = 2 * c.pts_jitter as u64 + 1;
            let off = (index.wrapping_mul(2_654_435_761) >> 7) % span;
            pts = pts.wrapping_add((off as i64 - c.pts_jitter as i64) as u32);
        }
        pts
    }

    /// Starts sending the most recent captured frame, if a new one is ready.
    fn pick_frame(&mut self, now_ns: u64) {
        let Some(st) = self.stream.as_mut() else {
            return;
        };
        if st.cur.is_some() || now_ns < st.start_ns {
            return;
        }
        let latest = (now_ns - st.start_ns) / st.interval_ns;
        if latest < st.next_frame {
            return;
        }
        let skipped = latest - st.next_frame;
        st.next_frame = latest + 1;
        self.stats.frames_dropped += skipped;
        let scoped = self.quirks.only_first_frames.is_none_or(|n| latest < n);
        let every = |n: Option<u32>| {
            scoped && n.is_some_and(|n| n > 0 && latest % n as u64 == n as u64 - 1)
        };
        if every(self.quirks.drop_every) {
            self.stats.frames_dropped += 1;
            return;
        }
        let truncate = every(self.quirks.truncate_every);
        let error = every(self.quirks.err_every);
        let captured_ns = st.start_ns + latest * st.interval_ns;
        let (spec, interval_ns) = (st.spec, st.interval_ns);
        // Method 2 still capture: this frame is the still image.
        let mut tx = if self.trigger == TRIGGER_STILL {
            self.trigger = TRIGGER_NORMAL;
            self.render_still(captured_ns)
        } else {
            None
        };
        if tx.is_none() {
            let data = self.source.render(&spec, latest, captured_ns);
            self.source
                .prefetch(&spec, latest + 1, captured_ns + interval_ns);
            tx = Some(FrameTx {
                index: latest,
                data,
                offset: 0,
                pts: self.pts(latest, captured_ns, interval_ns),
                error,
                still: false,
            });
        }
        let mut tx = tx.unwrap();
        if truncate {
            tx.data.truncate(tx.data.len() / 2);
        }
        let st = self.stream.as_mut().unwrap();
        if !self.quirks.no_fid_toggle {
            st.fid = !st.fid;
        }
        st.cur = Some(tx);
    }

    /// Writes a payload header for `f` (or an idle header) into `out`; `first`: the
    /// payload starts the frame.
    fn header(
        &self,
        out: &mut Vec<u8>,
        fid: bool,
        f: Option<&FrameTx>,
        first: bool,
        eof: bool,
        now: BusTime,
    ) {
        let mut hlen = self.header_len();
        let pts = hlen >= 6 && (first || !self.quirks.clock.pts_first_only);
        if hlen >= 6 && !pts {
            hlen -= 4;
        }
        let mut info = HDR_EOH | if fid { HDR_FID } else { 0 };
        if pts || self.quirks.lie_pts_scr {
            info |= HDR_PTS;
        }
        let scr = hlen >= if pts { 12 } else { 8 };
        if scr || self.quirks.lie_pts_scr {
            info |= HDR_SCR;
        }
        if eof && !self.quirks.no_eof {
            info |= HDR_EOF;
        }
        if f.is_some_and(|f| f.error) {
            info |= HDR_ERR;
        }
        if f.is_some_and(|f| f.still) {
            info |= HDR_STI;
        }
        let start = out.len();
        out.push(hlen as u8);
        out.push(info);
        if pts {
            out.extend_from_slice(&f.map_or(0, |f| f.pts).to_le_bytes());
        }
        if scr {
            let stc_ns = if self.quirks.clock.scr_stuck {
                0
            } else {
                now * UFRAME_NS
            };
            out.extend_from_slice(&self.ticks(stc_ns).to_le_bytes());
            out.extend_from_slice(&(((now / 8) & 0x7ff) as u16).to_le_bytes());
        }
        // Metadata after the SCR: the frame index, then a filler pattern.
        let meta = f.map_or(0, |f| f.index as u32).to_le_bytes();
        let fields = out.len() - start;
        while out.len() - start < hlen {
            out.push(*meta.get(out.len() - start - fields).unwrap_or(&0xa5));
        }
    }

    /// Takes up to `room` bytes of a frame; returns (bytes, end of frame).
    fn chunk(f: &mut FrameTx, room: usize) -> (&[u8], bool) {
        let n = (f.data.len() - f.offset).min(room);
        let s = f.offset;
        f.offset += n;
        (&f.data[s..s + n], f.offset == f.data.len())
    }

    /// Builds the next payload of at most `cap` bytes. `None`: nothing to send.
    fn payload(&mut self, now: BusTime, cap: usize) -> Option<Vec<u8>> {
        self.pick_frame(now * UFRAME_NS);
        let hlen = self.header_len();
        if cap < hlen {
            // The host's packet can't even hold a header.
            return None;
        }
        let mut st = self.stream.take()?;
        let mut out = Vec::with_capacity(cap.min(st.payload_size.max(hlen)));
        let mut body = Vec::new();
        let first = st.cur.as_ref().is_some_and(|f| f.offset == 0);
        let eof = match st.cur.as_mut() {
            Some(f) => {
                let (data, eof) = Self::chunk(f, cap - hlen);
                body.extend_from_slice(data);
                eof
            }
            None => false,
        };
        self.header(&mut out, st.fid, st.cur.as_ref(), first, eof, now);
        out.extend_from_slice(&body);
        if eof {
            let f = st.cur.take().unwrap();
            if f.still {
                self.stats.stills_sent += 1;
            } else {
                self.stats.frames_sent += 1;
            }
        }
        self.stats.payloads += 1;
        self.stream = Some(st);
        Some(out)
    }

    /// One isochronous service interval.
    pub fn iso_in(&mut self, cx: &mut Ctx, max_len: usize) -> Vec<u8> {
        let Some(st) = &self.stream else {
            return Vec::new();
        };
        let cap = max_len.min(st.payload_size);
        self.pick_frame(cx.now_ns());
        let idle = self.stream.as_ref().is_some_and(|s| s.cur.is_none());
        if idle && self.quirks.idle == IdlePayload::Zlp {
            return Vec::new();
        }
        self.payload(cx.now(), cap).unwrap_or_default()
    }

    /// One bulk transfer: a payload, or `None` (NAK) between frames.
    pub fn bulk_in(&mut self, cx: &mut Ctx, max_len: usize) -> Option<Vec<u8>> {
        let st = self.stream.as_ref()?;
        let cap = max_len.min(st.payload_size.max(self.header_len() + 1));
        self.pick_frame(cx.now_ns());
        self.stream.as_ref()?.cur.as_ref()?;
        self.payload(cx.now(), cap)
    }

    /// Method 3: one transfer on the still image bulk endpoint.
    pub fn still_in(&mut self, cx: &mut Ctx, max_len: usize) -> Option<Vec<u8>> {
        let hlen = self.header_len();
        let mut f = self.still_tx.take()?;
        let first = f.offset == 0;
        let (data, eof) = Self::chunk(&mut f, max_len.saturating_sub(hlen));
        let body = data.to_vec();
        let mut out = Vec::with_capacity(hlen + body.len());
        self.header(&mut out, false, Some(&f), first, eof, cx.now());
        out.extend_from_slice(&body);
        if eof {
            self.trigger = TRIGGER_NORMAL;
            self.stats.stills_sent += 1;
        } else {
            self.still_tx = Some(f);
        }
        Some(out)
    }
}

/// Speed-independent helper used by tests and tools: bytes per second a
/// committed stream needs.
pub fn required_bandwidth(c: &StreamingControl) -> f64 {
    c.max_video_frame_size as f64 * 1e7 / c.frame_interval.max(1) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_roundtrip() {
        let c = StreamingControl {
            hint: 1,
            format_index: 2,
            frame_index: 3,
            frame_interval: 333333,
            max_video_frame_size: 614400,
            max_payload_transfer_size: 3060,
            clock_frequency: 48_000_000,
            framing_info: 3,
            layout_per_stream: 0x1122334455667788,
            ..Default::default()
        };
        for len in [26, 34, 48] {
            let b = c.to_bytes(len);
            assert_eq!(b.len(), len);
            let d = StreamingControl::from_bytes(&b);
            assert_eq!(d.frame_interval, 333333);
            assert_eq!(d.max_payload_transfer_size, 3060);
        }
        assert_eq!(StreamingControl::from_bytes(&c.to_bytes(48)), c);
        assert_eq!(StreamingControl::len_for(0x0100), 26);
        assert_eq!(StreamingControl::len_for(0x0110), 34);
        assert_eq!(StreamingControl::len_for(0x0150), 48);
    }
}
