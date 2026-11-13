//! Pixel formats, frame geometry and UVC format GUIDs.

/// Pixel format of a video frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// Packed 4:2:2, Y0 U Y1 V.
    Yuyv,
    /// Packed 4:2:2, U Y0 V Y1.
    Uyvy,
    /// Planar Y followed by interleaved UV, 4:2:0.
    Nv12,
    /// 8-bit luma only.
    Grey8,
    /// 16-bit little-endian luma (`luma << 8 | luma`).
    Grey16,
    /// 4:2:0, Y plane then interleaved UV, 16-bit LE words with 10-bit value in the top bits.
    P010,
    /// Planar Y, U, V (4:2:0).
    I420,
    /// Like NV12 with V/U swapped.
    Nv21,
    /// 3 bytes per pixel, B G R (MEDIASUBTYPE_RGB24 GUID).
    Bgr24,
    /// 3 bytes per pixel, R G B.
    Rgb24,
    /// 8-bit Bayer "BY8 " (BGGR).
    By8,
    /// 8-bit Bayer "BA81" (BGGR).
    Ba81,
    /// 8-bit Bayer GRBG.
    Grbg8,
    /// 8-bit Bayer GBRG.
    Gbrg8,
    /// 8-bit Bayer RGGB.
    Rggb8,
    /// 8-bit Bayer BGGR.
    Bggr8,
    /// Motion JPEG (variable size).
    Mjpeg,
    /// H.264 in a frame-based format (fake bitstream, see [`crate::h264`]).
    H264,
}

/// Format and resolution of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FrameSpec {
    pub format: PixelFormat,
    pub width: u16,
    pub height: u16,
}

/// Common tail of UVC format GUIDs (after the FourCC).
const GUID_TAIL: [u8; 12] = [
    0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
];

/// D3DFMT_L8 GUID as used by Windows Hello IR cameras; maps to [`PixelFormat::Grey8`].
pub const GUID_L8_IR: [u8; 16] = [
    0x32, 0, 0, 0, 0x02, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
];

const GUID_BGR24: [u8; 16] = [
    0x7d, 0xeb, 0x36, 0xe4, 0x4f, 0x52, 0xce, 0x11, 0x9f, 0x53, 0x00, 0x20, 0xaf, 0x0b, 0xa7, 0x70,
];
const GUID_RGB24: [u8; 16] = [
    0x7e, 0xeb, 0x36, 0xe4, 0x4f, 0x52, 0xce, 0x11, 0x9f, 0x53, 0x00, 0x20, 0xaf, 0x0b, 0xa7, 0x70,
];

fn fourcc_guid(cc: &[u8; 4]) -> [u8; 16] {
    let mut g = [0u8; 16];
    g[..4].copy_from_slice(cc);
    g[4..].copy_from_slice(&GUID_TAIL);
    g
}

impl PixelFormat {
    /// Every variant.
    pub const ALL: [Self; 18] = [
        Self::Yuyv,
        Self::Uyvy,
        Self::Nv12,
        Self::Grey8,
        Self::Grey16,
        Self::P010,
        Self::I420,
        Self::Nv21,
        Self::Bgr24,
        Self::Rgb24,
        Self::By8,
        Self::Ba81,
        Self::Grbg8,
        Self::Gbrg8,
        Self::Rggb8,
        Self::Bggr8,
        Self::Mjpeg,
        Self::H264,
    ];

    /// Size in bytes of an uncompressed frame, `None` for MJPEG.
    pub fn frame_size(self, w: u16, h: u16) -> Option<usize> {
        let px = w as usize * h as usize;
        match self {
            Self::Yuyv | Self::Uyvy => Some(px * 2),
            Self::Nv12 | Self::Nv21 | Self::I420 => Some(px * 3 / 2),
            Self::P010 => Some(px * 3),
            Self::Grey16 => Some(px * 2),
            Self::Bgr24 | Self::Rgb24 => Some(px * 3),
            Self::Grey8
            | Self::By8
            | Self::Ba81
            | Self::Grbg8
            | Self::Gbrg8
            | Self::Rggb8
            | Self::Bggr8 => Some(px),
            Self::Mjpeg | Self::H264 => None,
        }
    }

    /// Bits per pixel as reported in UVC uncompressed format descriptors.
    pub fn bits_per_pixel(self) -> Option<u8> {
        match self {
            Self::Yuyv | Self::Uyvy => Some(16),
            Self::Nv12 | Self::Nv21 | Self::I420 => Some(12),
            Self::P010 | Self::Grey16 => Some(16),
            Self::Bgr24 | Self::Rgb24 => Some(24),
            Self::Grey8
            | Self::By8
            | Self::Ba81
            | Self::Grbg8
            | Self::Gbrg8
            | Self::Rggb8
            | Self::Bggr8 => Some(8),
            Self::Mjpeg | Self::H264 => None,
        }
    }

    /// UVC format GUID; MJPEG has none (it uses its own descriptor type).
    pub fn guid(self) -> Option<[u8; 16]> {
        match self {
            Self::Yuyv => Some(fourcc_guid(b"YUY2")),
            Self::Uyvy => Some(fourcc_guid(b"UYVY")),
            Self::Nv12 => Some(fourcc_guid(b"NV12")),
            Self::Grey8 => Some(fourcc_guid(b"Y800")),
            Self::H264 => Some(fourcc_guid(b"H264")),
            Self::Grey16 => Some(fourcc_guid(b"Y16 ")),
            Self::P010 => Some(fourcc_guid(b"P010")),
            Self::I420 => Some(fourcc_guid(b"I420")),
            Self::Nv21 => Some(fourcc_guid(b"NV21")),
            Self::Bgr24 => Some(GUID_BGR24),
            Self::Rgb24 => Some(GUID_RGB24),
            Self::By8 => Some(fourcc_guid(b"BY8 ")),
            Self::Ba81 => Some(fourcc_guid(b"BA81")),
            Self::Grbg8 => Some(fourcc_guid(b"GRBG")),
            Self::Gbrg8 => Some(fourcc_guid(b"GBRG")),
            Self::Rggb8 => Some(fourcc_guid(b"RGGB")),
            Self::Bggr8 => Some(fourcc_guid(b"BGGR")),
            Self::Mjpeg => None,
        }
    }

    /// Inverse of [`guid`](Self::guid); also accepts [`GUID_L8_IR`].
    pub fn from_guid(guid: &[u8; 16]) -> Option<Self> {
        if *guid == GUID_L8_IR {
            return Some(Self::Grey8);
        }
        Self::ALL
            .into_iter()
            .find(|f| f.guid().as_ref() == Some(guid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_round_trip() {
        for f in PixelFormat::ALL {
            match f.guid() {
                Some(g) => assert_eq!(PixelFormat::from_guid(&g), Some(f), "{f:?}"),
                None => assert_eq!(f, PixelFormat::Mjpeg),
            }
        }
        assert_eq!(
            PixelFormat::from_guid(&GUID_L8_IR),
            Some(PixelFormat::Grey8)
        );
    }

    #[test]
    fn sizes_and_bpp() {
        use PixelFormat::*;
        assert_eq!(Grey16.frame_size(4, 2), Some(16));
        assert_eq!(P010.frame_size(4, 2), Some(24));
        assert_eq!(I420.frame_size(4, 2), Some(12));
        assert_eq!(Nv21.frame_size(4, 2), Some(12));
        assert_eq!(Bgr24.frame_size(4, 2), Some(24));
        assert_eq!(Rgb24.bits_per_pixel(), Some(24));
        assert_eq!(Rggb8.frame_size(4, 2), Some(8));
        assert_eq!(P010.bits_per_pixel(), Some(16));
        assert_eq!(I420.bits_per_pixel(), Some(12));
    }
}
