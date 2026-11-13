//! Test pattern generator implementing [`FrameSource`].

use crate::format::{FrameSpec, PixelFormat};
use crate::{FrameSource, convert, jpeg, stamp};

/// Which background pattern to draw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PatternKind {
    /// SMPTE-style bars, grey ramp and a moving white square.
    #[default]
    ColorBars,
    /// Diagonal gradient shifting with the frame index.
    Gradient,
    /// 8x8-cell board inverting every 15 frames.
    Checkerboard,
    /// Uniform colour.
    Solid { r: u8, g: u8, b: u8 },
}

/// Cached static backgrounds for one (width, height, kind).
struct Cache {
    key: (u16, u16, PatternKind),
    /// Backgrounds by phase (Checkerboard uses two, others one).
    bg: [Option<Vec<u8>>; 2],
}

/// Frame source drawing a [`PatternKind`] with an optional stamp.
pub struct TestPattern {
    kind: PatternKind,
    stamp: bool,
    quality: u8,
    strip_dht: bool,
    cache: Option<Cache>,
    work: Vec<u8>,
}

impl TestPattern {
    pub fn new(kind: PatternKind) -> Self {
        Self {
            kind,
            stamp: true,
            quality: 85,
            strip_dht: false,
            cache: None,
            work: Vec::new(),
        }
    }

    /// Enable or disable the stamp band (default on).
    pub fn with_stamp(mut self, on: bool) -> Self {
        self.stamp = on;
        self
    }

    /// JPEG quality 1..=100 (default 85).
    pub fn with_jpeg_quality(mut self, quality: u8) -> Self {
        self.quality = quality;
        self
    }

    /// Strip DHT segments from MJPEG output (default off).
    pub fn with_strip_dht(mut self, on: bool) -> Self {
        self.strip_dht = on;
        self
    }

    /// Fill `buf` with the RGB image for `index` (no stamp).
    fn draw(&mut self, w: usize, h: usize, index: u64, buf: &mut Vec<u8>) {
        let kind = self.kind;
        if kind == PatternKind::Gradient {
            gradient(buf, w, h, index);
            return;
        }
        let phase = usize::from(kind == PatternKind::Checkerboard && (index / 15) % 2 == 1);
        let key = (w as u16, h as u16, kind);
        if self.cache.as_ref().is_none_or(|c| c.key != key) {
            self.cache = Some(Cache {
                key,
                bg: [None, None],
            });
        }
        let slot = &mut self.cache.as_mut().expect("cache set above").bg[phase];
        let bg = slot.get_or_insert_with(|| static_background(kind, w, h, phase));
        buf.clear();
        buf.extend_from_slice(bg);
        if kind == PatternKind::ColorBars {
            moving_square(buf, w, h, index);
        }
    }
}

impl Default for TestPattern {
    fn default() -> Self {
        Self::new(PatternKind::default())
    }
}

fn static_background(kind: PatternKind, w: usize, h: usize, phase: usize) -> Vec<u8> {
    let mut buf = vec![0u8; w * h * 3];
    match kind {
        PatternKind::ColorBars => color_bars(&mut buf, w, h),
        PatternKind::Checkerboard => {
            for (y, row) in buf.chunks_exact_mut(w * 3).enumerate() {
                for (x, p) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                    let on = ((x / 8 + y / 8) % 2 == 0) != (phase == 1);
                    p.fill(if on { 235 } else { 20 });
                }
            }
        }
        PatternKind::Solid { r, g, b } => {
            for p in buf.as_chunks_mut::<3>().0 {
                *p = [r, g, b];
            }
        }
        PatternKind::Gradient => gradient(&mut buf, w, h, 0),
    }
    buf
}

fn color_bars(buf: &mut [u8], w: usize, h: usize) {
    const BARS: [[u8; 3]; 7] = [
        [191, 191, 191],
        [191, 191, 0],
        [0, 191, 191],
        [0, 191, 0],
        [191, 0, 191],
        [191, 0, 0],
        [0, 0, 191],
    ];
    let bars_h = h * 2 / 3;
    for (y, row) in buf.chunks_exact_mut(w * 3).enumerate() {
        for (x, p) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            if y < bars_h {
                p.copy_from_slice(&BARS[x * 7 / w]);
            } else {
                p.fill((x * 255 / (w - 1).max(1)) as u8);
            }
        }
    }
}

fn gradient(buf: &mut Vec<u8>, w: usize, h: usize, index: u64) {
    buf.resize(w * h * 3, 0);
    let period = (w + h).max(1);
    let shift = (index as usize).wrapping_mul(4) % period;
    for (y, row) in buf.chunks_exact_mut(w * 3).enumerate() {
        for (x, p) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let t = ((x + y + shift) % period) * 255 / period;
            p.copy_from_slice(&[t as u8, 255 - t as u8, (t as u8).wrapping_mul(2)]);
        }
    }
}

/// White square sweeping horizontally, wrapping around.
fn moving_square(buf: &mut [u8], w: usize, h: usize, index: u64) {
    let side = (h / 8).max(1).min(w);
    let step = (w / 64).max(1);
    let x0 = (index % ((w - side + 1) as u64 / step as u64).max(1)) as usize * step;
    let y0 = (h / 4).min(h - side);
    for y in y0..y0 + side {
        buf[(y * w + x0) * 3..(y * w + x0 + side) * 3].fill(255);
    }
}

impl FrameSource for TestPattern {
    fn render(&mut self, spec: &FrameSpec, index: u64, _time_ns: u64) -> Vec<u8> {
        let (w, h) = (spec.width as usize, spec.height as usize);
        if w == 0 || h == 0 {
            return Vec::new();
        }
        let mut rgb = std::mem::take(&mut self.work);
        self.draw(w, h, index, &mut rgb);
        if self.stamp {
            stamp::paint_rgb(&mut rgb, w, h, index as u32);
        }
        let mut out = Vec::new();
        match spec.format {
            PixelFormat::Yuyv => convert::rgb_to_yuyv(&rgb, w, h, &mut out),
            PixelFormat::Uyvy => convert::rgb_to_uyvy(&rgb, w, h, &mut out),
            PixelFormat::Nv12 => convert::rgb_to_nv12(&rgb, w, h, &mut out),
            PixelFormat::Grey8 => convert::rgb_to_grey(&rgb, w, h, &mut out),
            PixelFormat::Mjpeg => {
                out = jpeg::encode_rgb(&rgb, spec.width, spec.height, self.quality);
                if self.strip_dht {
                    out = jpeg::strip_dht(&out);
                }
            }
        }
        self.work = rgb;
        out
    }
}
