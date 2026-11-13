//! JPEG encoding (baseline, 4:2:2) and DHT segment handling.

use jpeg_encoder::{ColorType, Encoder, SamplingFactor};
use std::sync::OnceLock;

/// Encode RGB24 as a baseline JPEG with 4:2:2 chroma subsampling.
pub fn encode_rgb(rgb: &[u8], width: u16, height: u16, quality: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(width as usize * height as usize / 8 + 1024);
    let mut enc = Encoder::new(&mut out, quality.clamp(1, 100));
    enc.set_sampling_factor(SamplingFactor::F_2_1);
    enc.encode(rgb, width, height, ColorType::Rgb)
        .expect("JPEG encoding of an in-memory buffer cannot fail");
    out
}

/// Iterate over `(marker, segment_start, segment_end)` for segments before SOS.
/// Returns the offset of the first SOS marker (or `jpeg.len()`).
fn walk(jpeg: &[u8], mut f: impl FnMut(u8, usize, usize)) -> usize {
    let mut i = 2;
    while i + 4 <= jpeg.len() && jpeg[i] == 0xff {
        let marker = jpeg[i + 1];
        if marker == 0xda {
            return i;
        }
        let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
        let end = (i + 2 + len).min(jpeg.len());
        f(marker, i, end);
        i = end;
    }
    jpeg.len()
}

/// Whether the JPEG contains a DHT (Huffman table) segment.
pub fn has_dht(jpeg: &[u8]) -> bool {
    let mut found = false;
    walk(jpeg, |m, _, _| found |= m == 0xc4);
    found
}

/// Remove all DHT segments (as UVC MJPEG cameras commonly do).
pub fn strip_dht(jpeg: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(jpeg.len());
    let mut copied = 0;
    let sos = walk(jpeg, |m, s, e| {
        if m == 0xc4 {
            out.extend_from_slice(&jpeg[copied..s]);
            copied = e;
        }
    });
    let _ = sos;
    out.extend_from_slice(&jpeg[copied..]);
    out
}

/// The standard (ITU-T T.81 Annex K) DHT segments, harvested from the encoder.
fn default_dht() -> &'static [u8] {
    static DHT: OnceLock<Vec<u8>> = OnceLock::new();
    DHT.get_or_init(|| {
        let j = encode_rgb(&[0u8; 8 * 8 * 3], 8, 8, 50);
        let mut dht = Vec::new();
        walk(&j, |m, s, e| {
            if m == 0xc4 {
                dht.extend_from_slice(&j[s..e]);
            }
        });
        dht
    })
}

/// Insert the standard Huffman tables before SOS if the JPEG has none.
pub fn insert_default_dht(jpeg: &[u8]) -> Vec<u8> {
    if has_dht(jpeg) {
        return jpeg.to_vec();
    }
    let sos = walk(jpeg, |_, _, _| {});
    let mut out = Vec::with_capacity(jpeg.len() + 450);
    out.extend_from_slice(&jpeg[..sos]);
    out.extend_from_slice(default_dht());
    out.extend_from_slice(&jpeg[sos..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_and_insert() {
        let j = encode_rgb(&[128u8; 16 * 16 * 3], 16, 16, 80);
        assert!(has_dht(&j));
        let s = strip_dht(&j);
        assert!(!has_dht(&s));
        assert!(s.len() < j.len());
        assert_eq!(&s[..2], &[0xff, 0xd8]);
        assert_eq!(insert_default_dht(&s), j);
    }
}
