//! RGB24 to YUV conversions (BT.601 limited range, integer fixed point).
//!
//! Odd dimensions are handled by replicating the last column/row; the output
//! is laid out for the even-rounded width (`w + (w & 1)`), and NV12 chroma
//! rows are rounded up.

const SHIFT: u32 = 16;

/// `v / 255` in 16.16 fixed point, rounded.
const fn k(v: f64) -> i32 {
    (v / 255.0 * 65536.0 + 0.5) as i32
}
const KY: [i32; 3] = [k(65.481), k(128.553), k(24.966)];
const KCB: [i32; 3] = [k(-37.797), k(-74.203), k(112.0)];
const KCR: [i32; 3] = [k(112.0), k(-93.786), k(-18.214)];

#[inline]
fn dot(c: &[i32; 3], r: i32, g: i32, b: i32, offset: i32) -> u8 {
    let v = offset + ((c[0] * r + c[1] * g + c[2] * b + (1 << (SHIFT - 1))) >> SHIFT);
    v.clamp(0, 255) as u8
}

#[inline]
fn luma(p: &[u8]) -> u8 {
    dot(&KY, p[0] as i32, p[1] as i32, p[2] as i32, 16)
}

/// Chroma from the sum of `n` pixels.
#[inline]
fn chroma(sum: [i32; 3], n: i32) -> (u8, u8) {
    let [r, g, b] = sum.map(|s| (s + n / 2) / n);
    (dot(&KCB, r, g, b, 128), dot(&KCR, r, g, b, 128))
}

#[inline]
fn px(rgb: &[u8], w: usize, x: usize, y: usize) -> &[u8] {
    let i = (y * w + x.min(w - 1)) * 3;
    &rgb[i..i + 3]
}

fn pack422(rgb: &[u8], w: usize, h: usize, out: &mut Vec<u8>, y_first: bool) {
    out.clear();
    if w == 0 || h == 0 {
        return;
    }
    assert!(rgb.len() >= w * h * 3, "rgb buffer too small");
    out.reserve(w.div_ceil(2) * 4 * h);
    for y in 0..h {
        for x in (0..w).step_by(2) {
            let (p0, p1) = (px(rgb, w, x, y), px(rgb, w, x + 1, y));
            let sum = [0, 1, 2].map(|i| p0[i] as i32 + p1[i] as i32);
            let (cb, cr) = chroma(sum, 2);
            let (y0, y1) = (luma(p0), luma(p1));
            if y_first {
                out.extend_from_slice(&[y0, cb, y1, cr]);
            } else {
                out.extend_from_slice(&[cb, y0, cr, y1]);
            }
        }
    }
}

/// Convert to packed YUYV (Y0 U Y1 V).
pub fn rgb_to_yuyv(rgb: &[u8], w: usize, h: usize, out: &mut Vec<u8>) {
    pack422(rgb, w, h, out, true);
}

/// Convert to packed UYVY (U Y0 V Y1).
pub fn rgb_to_uyvy(rgb: &[u8], w: usize, h: usize, out: &mut Vec<u8>) {
    pack422(rgb, w, h, out, false);
}

/// Convert to NV12 (Y plane, then interleaved UV at half resolution).
pub fn rgb_to_nv12(rgb: &[u8], w: usize, h: usize, out: &mut Vec<u8>) {
    out.clear();
    if w == 0 || h == 0 {
        return;
    }
    assert!(rgb.len() >= w * h * 3, "rgb buffer too small");
    let w2 = w.div_ceil(2) * 2;
    out.reserve(w2 * h + w2 * h.div_ceil(2));
    for y in 0..h {
        out.extend((0..w2).map(|x| luma(px(rgb, w, x, y))));
    }
    for y in (0..h).step_by(2) {
        let y1 = (y + 1).min(h - 1);
        for x in (0..w2).step_by(2) {
            let mut sum = [0i32; 3];
            for p in [
                px(rgb, w, x, y),
                px(rgb, w, x + 1, y),
                px(rgb, w, x, y1),
                px(rgb, w, x + 1, y1),
            ] {
                for i in 0..3 {
                    sum[i] += p[i] as i32;
                }
            }
            let (cb, cr) = chroma(sum, 4);
            out.extend_from_slice(&[cb, cr]);
        }
    }
}

/// Convert to 8-bit luma.
pub fn rgb_to_grey(rgb: &[u8], w: usize, h: usize, out: &mut Vec<u8>) {
    out.clear();
    assert!(rgb.len() >= w * h * 3, "rgb buffer too small");
    out.extend(rgb[..w * h * 3].as_chunks::<3>().0.iter().map(|p| luma(p)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_black_luma() {
        assert_eq!(luma(&[255, 255, 255]), 235);
        assert_eq!(luma(&[0, 0, 0]), 16);
    }

    #[test]
    fn odd_sizes_do_not_panic() {
        let rgb = vec![100u8; 5 * 3 * 3];
        let mut out = Vec::new();
        rgb_to_yuyv(&rgb, 5, 3, &mut out);
        assert_eq!(out.len(), 6 * 3 * 2);
        rgb_to_nv12(&rgb, 5, 3, &mut out);
        assert_eq!(out.len(), 6 * 3 + 6 * 2);
    }
}
