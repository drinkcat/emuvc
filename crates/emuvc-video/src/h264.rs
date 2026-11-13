//! Fake H.264 frames for frame-based UVC formats.
//!
//! Hosts like libuvc pass H.264 through without decoding it, so a real
//! encoder isn't needed: each frame is a valid Annex B access unit holding
//! an access unit delimiter, an SEI "user data unregistered" message
//! carrying the frame stamp (value + CRC-8, see [`crate::stamp`]) and a
//! filler NAL giving it a plausible size. Frame 0 of every 30 is flagged as a
//! key frame in the delimiter's primary_pic_type for realism.

use crate::stamp::crc8;

/// UUID of the SEI message carrying the stamp: "emuvc frame stamp".
pub const STAMP_UUID: [u8; 16] = *b"emuvc-stamp-sei!";

/// Adds emulation prevention bytes (00 00 0x -> 00 00 03 0x for x <= 3).
fn escape(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + 4);
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

fn unescape(ebsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ebsp.len());
    let mut zeros = 0;
    for &b in ebsp {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// One access unit for frame `index`, roughly `width * height / 64` bytes.
pub fn frame(index: u64, width: u16, height: u16) -> Vec<u8> {
    let value = index as u32;
    let mut out = vec![
        0,
        0,
        0,
        1,
        0x09,
        if index.is_multiple_of(30) { 0x10 } else { 0x30 },
    ];
    let mut sei = vec![0x05, 21];
    sei.extend_from_slice(&STAMP_UUID);
    sei.extend_from_slice(&value.to_be_bytes());
    sei.push(crc8(&value.to_be_bytes()));
    sei.push(0x80);
    out.extend_from_slice(&[0, 0, 0, 1, 0x06]);
    out.extend(escape(&sei));
    let filler = (width as usize * height as usize / 64).max(16);
    out.extend_from_slice(&[0, 0, 0, 1, 0x0c]);
    out.extend(std::iter::repeat_n(0xff, filler));
    out.push(0x80);
    out
}

/// Recovers the stamp value from a frame made by [`frame`].
pub fn decode(data: &[u8]) -> Option<u32> {
    let pos = data.windows(5).position(|w| w == [0, 0, 0, 1, 0x06])?;
    let end = data[pos + 5..]
        .windows(4)
        .position(|w| w == [0, 0, 0, 1])
        .map_or(data.len(), |e| pos + 5 + e);
    let sei = unescape(&data[pos + 5..end]);
    if sei.len() < 2 + 16 + 5 || sei[0] != 0x05 || sei[2..18] != STAMP_UUID {
        return None;
    }
    let v: [u8; 4] = sei[18..22].try_into().ok()?;
    (crc8(&v) == sei[22]).then(|| u32::from_be_bytes(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for i in [0u64, 1, 255, 0x0000_0100, 0x0003_0000, u32::MAX as u64] {
            let f = frame(i, 64, 48);
            assert_eq!(decode(&f), Some(i as u32), "{i:#x}");
        }
        assert_eq!(escape(&[0, 0, 1, 0, 0, 0]), vec![0, 0, 3, 1, 0, 0, 3, 0]);
        assert_eq!(
            unescape(&escape(&[0, 0, 3, 0, 0, 2])),
            vec![0, 0, 3, 0, 0, 2]
        );
    }
}
