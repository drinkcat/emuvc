//! Frame stamp: a 40-bit machine-readable code painted at the top of each frame.
//!
//! # Layout (normative)
//!
//! * **Payload**: 40 bits, MSB first: the `u32` value (bits 31..0) followed by
//!   the CRC-8 of the value's 4 bytes in big-endian order (polynomial 0x07,
//!   init 0x00, no input/output reflection, no final xor).
//! * **Band**: image rows `[0, band_h)` where `band_h = max(8, height / 10)`
//!   rounded down to an even number and clamped to `height`.
//! * **Cells**: the band is split horizontally into 42 cells; cell `i` spans
//!   columns `[i * width / 42, (i + 1) * width / 42)` (integer division).
//!   Cell 0 is white (start), cells 1..=40 are the payload bits (1 = white,
//!   0 = black), cell 41 is black (stop).
//! * **Colours**: RGB white (255,255,255) and black (0,0,0); after BT.601
//!   limited-range conversion these have luma 235 and 16.
//! * **Decoding**: for each cell average the luma over the central 50% of the
//!   cell's columns and the central 50% of the band's rows; bit = average >= 128.
//!   Accept only if start = 1, stop = 0 and the CRC matches.

use crate::format::PixelFormat;

const CELLS: usize = 42;

/// CRC-8 (poly 0x07, init 0, no reflection, no xorout).
pub fn crc8(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |mut crc, &b| {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
        crc
    })
}

/// Height in rows of the stamp band for a frame of the given height.
pub fn band_height(height: usize) -> usize {
    ((height / 10).max(8) & !1).min(height)
}

/// Column range of cell `i`.
fn cell_range(i: usize, width: usize) -> (usize, usize) {
    (i * width / CELLS, (i + 1) * width / CELLS)
}

/// The 42 cell values (true = white) for `value`.
fn cells(value: u32) -> [bool; CELLS] {
    let payload = (u64::from(value) << 8) | u64::from(crc8(&value.to_be_bytes()));
    let mut c = [false; CELLS];
    c[0] = true;
    for (i, cell) in c[1..=40].iter_mut().enumerate() {
        *cell = (payload >> (39 - i)) & 1 == 1;
    }
    c
}

/// Paint the stamp into the top band of an RGB24 buffer.
pub fn paint_rgb(rgb: &mut [u8], width: usize, height: usize, value: u32) {
    let band = band_height(height);
    let cells = cells(value);
    let first_row = &mut rgb[..width * 3];
    for (i, &white) in cells.iter().enumerate() {
        let (x0, x1) = cell_range(i, width);
        first_row[x0 * 3..x1 * 3].fill(if white { 255 } else { 0 });
    }
    let (head, rest) = rgb.split_at_mut(width * 3);
    for row in rest[..(band.saturating_sub(1)) * width * 3].chunks_exact_mut(width * 3) {
        row.copy_from_slice(head);
    }
}

/// Decode a stamp from a luma plane with the given row `stride`.
pub fn decode_luma(luma: &[u8], width: usize, height: usize, stride: usize) -> Option<u32> {
    let band = band_height(height);
    if width < CELLS || band == 0 || luma.len() < (band - 1) * stride + width {
        return None;
    }
    let (y0, y1) = (band / 4, band - band / 4);
    let mut bits = [false; CELLS];
    for (i, bit) in bits.iter_mut().enumerate() {
        let (cx0, cx1) = cell_range(i, width);
        let q = (cx1 - cx0) / 4;
        let (x0, x1) = (cx0 + q, (cx1 - q).max(cx0 + q + 1));
        let mut sum = 0usize;
        for y in y0..y1 {
            sum += luma[y * stride + x0..y * stride + x1]
                .iter()
                .map(|&v| v as usize)
                .sum::<usize>();
        }
        let n = (y1 - y0) * (x1 - x0);
        *bit = sum >= 128 * n;
    }
    if !bits[0] || bits[41] {
        return None;
    }
    let payload = bits[1..=40]
        .iter()
        .fold(0u64, |acc, &b| (acc << 1) | u64::from(b));
    let value = (payload >> 8) as u32;
    (payload as u8 == crc8(&value.to_be_bytes())).then_some(value)
}

/// Decode the stamp from a complete frame in `format`.
pub fn decode_frame(format: PixelFormat, data: &[u8], width: usize, height: usize) -> Option<u32> {
    let band = band_height(height);
    let w2 = width.div_ceil(2) * 2;
    match format {
        PixelFormat::Grey8 | PixelFormat::Nv12 => decode_luma(
            data,
            width,
            height,
            if format == PixelFormat::Nv12 {
                w2
            } else {
                width
            },
        ),
        PixelFormat::Yuyv | PixelFormat::Uyvy => {
            let off = usize::from(format == PixelFormat::Uyvy);
            let stride = w2 * 2;
            if data.len() < stride * band {
                return None;
            }
            let mut luma = Vec::with_capacity(width * band);
            for y in 0..band {
                luma.extend(data[y * stride..][..width * 2].iter().skip(off).step_by(2));
            }
            decode_luma(&luma, width, height, width)
        }
        PixelFormat::Grey16 | PixelFormat::P010 => {
            // High byte of each little-endian word is the 8-bit luma.
            let stride = if format == PixelFormat::P010 {
                w2 * 2
            } else {
                width * 2
            };
            if data.len() < stride * band {
                return None;
            }
            let mut luma = Vec::with_capacity(width * band);
            for y in 0..band {
                luma.extend(data[y * stride..][..width * 2].iter().skip(1).step_by(2));
            }
            decode_luma(&luma, width, height, width)
        }
        PixelFormat::I420 | PixelFormat::Nv21 => decode_luma(data, width, height, w2),
        PixelFormat::Bgr24 | PixelFormat::Rgb24 => {
            let stride = width * 3;
            if data.len() < stride * band {
                return None;
            }
            let mut luma = Vec::with_capacity(width * band);
            for y in 0..band {
                luma.extend(data[y * stride..][..stride].iter().step_by(3));
            }
            decode_luma(&luma, width, height, width)
        }
        PixelFormat::By8
        | PixelFormat::Ba81
        | PixelFormat::Grbg8
        | PixelFormat::Gbrg8
        | PixelFormat::Rggb8
        | PixelFormat::Bggr8 => decode_luma(data, width, height, width),
        PixelFormat::Mjpeg => decode_mjpeg(data, width, height),
        PixelFormat::H264 => crate::h264::decode(data),
    }
}

#[cfg(feature = "decode")]
fn decode_mjpeg(data: &[u8], width: usize, height: usize) -> Option<u32> {
    use zune_jpeg::zune_core::{
        bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions,
    };
    let data = crate::jpeg::insert_default_dht(data);
    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::Luma);
    let mut dec = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(&data), opts);
    let luma = dec.decode().ok()?;
    let info = dec.info()?;
    if info.width as usize != width || info.height as usize != height {
        return None;
    }
    decode_luma(&luma, width, height, width)
}

#[cfg(not(feature = "decode"))]
fn decode_mjpeg(_: &[u8], _: usize, _: usize) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_check_value() {
        assert_eq!(crc8(b"123456789"), 0xf4);
    }

    #[test]
    fn band_heights() {
        assert_eq!(band_height(240), 24);
        assert_eq!(band_height(1080), 108);
        assert_eq!(band_height(50), 8);
        assert_eq!(band_height(4), 4);
    }
}
