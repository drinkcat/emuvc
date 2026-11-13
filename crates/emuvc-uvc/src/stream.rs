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

/// Class-level misbehaviour knobs for a streaming interface. All default to
/// a well-behaved device.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StreamQuirks {
    pub idle: IdlePayload,
    /// Payload header length: 2 (no PTS/SCR), 6 (PTS) or 12 (PTS+SCR, default).
    pub header_len: Option<u8>,
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
    /// Report this dwMaxPayloadTransferSize in probe/commit (e.g. 0).
    pub max_payload_transfer_size: Option<u32>,
    /// Bulk: device payload size (default: dwMaxPayloadTransferSize).
    pub bulk_payload_size: Option<u32>,
    /// Probe/commit length regardless of bcdUVC (e.g. 26 or 34 on a 1.5 device).
    pub probe_len: Option<u16>,
    /// Negotiate the default format/frame whatever the host asks for.
    pub ignore_probe_format: bool,
    /// Delay before the first frame after streaming starts.
    pub startup_ms: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamStats {
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub payloads: u64,
}

struct FrameTx {
    index: u64,
    data: Vec<u8>,
    offset: usize,
    pts: u32,
    error: bool,
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

/// One VideoStreaming interface.
pub struct VsInterface {
    pub desc: VideoStreaming,
    bcd_uvc: u16,
    clock_frequency: u32,
    pub quirks: StreamQuirks,
    probe: StreamingControl,
    commit: Option<StreamingControl>,
    still_probe: Vec<u8>,
    still_commit: Vec<u8>,
    alt: u8,
    pub error_code: u8,
    stream: Option<Stream>,
    source: Box<dyn FrameSource>,
    pub stats: StreamStats,
    name: String,
}

fn pixel_format(kind: &FormatKind) -> Option<PixelFormat> {
    match kind {
        FormatKind::Uncompressed { guid, .. } => PixelFormat::from_guid(guid),
        FormatKind::Mjpeg => Some(PixelFormat::Mjpeg),
        FormatKind::FrameBased { guid, .. } if &guid[..4] == b"MJPG" => Some(PixelFormat::Mjpeg),
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
            still_probe: vec![0; 11],
            still_commit: vec![0; 11],
            alt: 0,
            error_code: 0,
            stream: None,
            source,
            stats: StreamStats::default(),
            name,
        };
        vs.probe = vs.default_probe();
        vs
    }

    pub fn interface(&self) -> u8 {
        self.desc.interface
    }

    pub fn is_streaming(&self) -> bool {
        self.stream.is_some()
    }

    fn probe_len(&self) -> usize {
        self.quirks
            .probe_len
            .map_or_else(|| StreamingControl::len_for(self.bcd_uvc), |l| l as usize)
    }

    fn header_len(&self) -> usize {
        match self.quirks.header_len {
            Some(n @ (2 | 6 | 12)) => n as usize,
            _ => 12,
        }
    }

    fn default_probe(&self) -> StreamingControl {
        let req = StreamingControl {
            format_index: 1,
            ..Default::default()
        };
        self.negotiate(&req, false).unwrap_or_default()
    }

    /// Size of one frame for the negotiated format/frame.
    fn frame_size(kind: &FormatKind, frame: &Frame) -> u32 {
        match kind {
            FormatKind::Uncompressed { bits_per_pixel, .. } => {
                (frame.width as u32 * frame.height as u32 * *bits_per_pixel as u32).div_ceil(8)
            }
            _ => frame.max_frame_size,
        }
    }

    /// Picks the payload size for a format/frame/interval: for isochronous
    /// streams the smallest alternate setting that carries the required
    /// bandwidth (the largest if none does); for bulk 32 KiB payloads.
    fn payload_size(&self, kind: &FormatKind, frame: &Frame, interval: u32) -> u32 {
        match &self.desc.transfer {
            Transfer::Isochronous { alts } => {
                let fps_x = 10_000_000f64 / interval.max(1) as f64;
                let bytes_per_uframe = match kind {
                    FormatKind::Uncompressed { .. } => {
                        Self::frame_size(kind, frame) as f64 * fps_x / 8000.0
                    }
                    _ => frame.max_bit_rate as f64 / 8.0 / 8000.0,
                };
                let need = bytes_per_uframe.ceil() as usize + self.header_len();
                let mut sizes: Vec<usize> = alts.iter().map(|a| a.bytes_per_interval).collect();
                sizes.sort_unstable();
                sizes
                    .iter()
                    .copied()
                    .find(|&s| s >= need)
                    .or(sizes.last().copied())
                    .unwrap_or(0) as u32
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
            max_video_frame_size: Self::frame_size(&format.kind, frame),
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
    fn report(&self, c: &StreamingControl) -> Vec<u8> {
        let mut c = c.clone();
        if let Some(m) = self.quirks.max_payload_transfer_size {
            c.max_payload_transfer_size = m;
        }
        c.to_bytes(self.probe_len())
    }

    /// Class request to this interface. Returns data or a request error code.
    pub fn control(
        &mut self,
        cx: &mut Ctx,
        request: u8,
        selector: u8,
        data: &[u8],
    ) -> Result<Vec<u8>, u8> {
        match (selector, request) {
            (VS_PROBE_CONTROL, SET_CUR) => {
                let req = StreamingControl::from_bytes(data);
                self.probe = self.negotiate(&req, false)?;
                debug!("{}: probe {:?} -> {:?}", self.name, req, self.probe);
                Ok(Vec::new())
            }
            (VS_PROBE_CONTROL, GET_CUR) => Ok(self.report(&self.probe)),
            (VS_PROBE_CONTROL, GET_DEF) => Ok(self.report(&self.default_probe())),
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
                Ok(self.report(&c))
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
            (VS_COMMIT_CONTROL, GET_CUR) => {
                Ok(self.report(self.commit.as_ref().unwrap_or(&self.probe)))
            }
            (VS_STILL_PROBE_CONTROL, SET_CUR) => {
                self.still_probe = data.to_vec();
                self.still_probe.resize(11, 0);
                Ok(Vec::new())
            }
            (VS_STILL_PROBE_CONTROL, GET_CUR | GET_MIN | GET_MAX | GET_DEF) => {
                Ok(self.still_probe.clone())
            }
            (VS_STILL_COMMIT_CONTROL, SET_CUR) => {
                self.still_commit = data.to_vec();
                self.still_commit.resize(11, 0);
                Ok(Vec::new())
            }
            (VS_STILL_COMMIT_CONTROL, GET_CUR) => Ok(self.still_commit.clone()),
            (VS_STILL_PROBE_CONTROL | VS_STILL_COMMIT_CONTROL, GET_LEN) => Ok(vec![11, 0]),
            (VS_STILL_PROBE_CONTROL | VS_STILL_COMMIT_CONTROL, GET_INFO) => {
                Ok(vec![INFO_GET | INFO_SET])
            }
            (VS_STILL_IMAGE_TRIGGER_CONTROL, SET_CUR) => Ok(Vec::new()),
            (VS_STILL_IMAGE_TRIGGER_CONTROL, GET_CUR) => Ok(vec![0]),
            (VS_STILL_IMAGE_TRIGGER_CONTROL, GET_INFO) => Ok(vec![INFO_GET | INFO_SET]),
            (VS_STREAM_ERROR_CODE_CONTROL, GET_CUR) => Ok(vec![self.error_code]),
            (VS_STREAM_ERROR_CODE_CONTROL, GET_INFO) => Ok(vec![INFO_GET]),
            (VS_PROBE_CONTROL..=VS_STREAM_ERROR_CODE_CONTROL, _) => Err(ERR_INVALID_REQUEST),
            _ => Err(ERR_INVALID_CONTROL),
        }
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
        self.probe = self.default_probe();
        self.error_code = 0;
    }

    pub fn clear_halt(&mut self) {
        // UVC 1.5 §2.4.3.2.4: on bulk, CLEAR_FEATURE(ENDPOINT_HALT) stops the stream.
        if matches!(self.desc.transfer, Transfer::Bulk { .. }) {
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

    fn ticks(&self, ns: u64) -> u32 {
        (ns as u128 * self.clock_frequency as u128 / 1_000_000_000) as u32
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
        let every = |n: Option<u32>| n.is_some_and(|n| n > 0 && latest % n as u64 == n as u64 - 1);
        if every(self.quirks.drop_every) {
            self.stats.frames_dropped += 1;
            return;
        }
        let captured_ns = st.start_ns + latest * st.interval_ns;
        let (spec, interval_ns) = (st.spec, st.interval_ns);
        let mut data = self.source.render(&spec, latest, captured_ns);
        self.source
            .prefetch(&spec, latest + 1, captured_ns + interval_ns);
        if every(self.quirks.truncate_every) {
            data.truncate(data.len() / 2);
        }
        let error = every(self.quirks.err_every);
        let pts = self.ticks(captured_ns);
        let st = self.stream.as_mut().unwrap();
        if !self.quirks.no_fid_toggle {
            st.fid = !st.fid;
        }
        st.cur = Some(FrameTx {
            index: latest,
            data,
            offset: 0,
            pts,
            error,
        });
    }

    /// Builds the next payload of at most `cap` bytes. `None`: nothing to send.
    fn payload(&mut self, now: BusTime, cap: usize) -> Option<Vec<u8>> {
        let now_ns = now * UFRAME_NS;
        self.pick_frame(now_ns);
        let hlen = self.header_len();
        if cap < hlen {
            // The host's packet can't even hold a header.
            return None;
        }
        let stc = self.ticks(now_ns);
        let no_eof = self.quirks.no_eof;
        let st = self.stream.as_mut()?;
        let mut info = HDR_EOH | if st.fid { HDR_FID } else { 0 };
        if hlen >= 6 {
            info |= HDR_PTS;
        }
        if hlen >= 12 {
            info |= HDR_SCR;
        }
        let mut out = Vec::with_capacity(cap.min(st.payload_size.max(hlen)));
        let mut finished = None;
        let pts = match &mut st.cur {
            Some(f) => {
                let n = (f.data.len() - f.offset).min(cap.saturating_sub(hlen));
                if f.offset + n == f.data.len() {
                    if !no_eof {
                        info |= HDR_EOF;
                    }
                    finished = Some(f.index);
                }
                if f.error {
                    info |= HDR_ERR;
                }
                f.pts
            }
            None => 0,
        };
        out.push(hlen as u8);
        out.push(info);
        if hlen >= 6 {
            out.extend_from_slice(&pts.to_le_bytes());
        }
        if hlen >= 12 {
            out.extend_from_slice(&stc.to_le_bytes());
            out.extend_from_slice(&(((now / 8) & 0x7ff) as u16).to_le_bytes());
        }
        if let Some(f) = &mut st.cur {
            let n = (f.data.len() - f.offset).min(cap.saturating_sub(hlen));
            out.extend_from_slice(&f.data[f.offset..f.offset + n]);
            f.offset += n;
        }
        if finished.is_some() {
            st.cur = None;
            self.stats.frames_sent += 1;
        }
        self.stats.payloads += 1;
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
