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
    /// Motion JPEG (variable size).
    Mjpeg,
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

fn fourcc_guid(cc: &[u8; 4]) -> [u8; 16] {
    let mut g = [0u8; 16];
    g[..4].copy_from_slice(cc);
    g[4..].copy_from_slice(&GUID_TAIL);
    g
}

impl PixelFormat {
    /// Size in bytes of an uncompressed frame, `None` for MJPEG.
    pub fn frame_size(self, w: u16, h: u16) -> Option<usize> {
        let px = w as usize * h as usize;
        match self {
            Self::Yuyv | Self::Uyvy => Some(px * 2),
            Self::Nv12 => Some(px * 3 / 2),
            Self::Grey8 => Some(px),
            Self::Mjpeg => None,
        }
    }

    /// Bits per pixel as reported in UVC uncompressed format descriptors.
    pub fn bits_per_pixel(self) -> Option<u8> {
        match self {
            Self::Yuyv | Self::Uyvy => Some(16),
            Self::Nv12 => Some(12),
            Self::Grey8 => Some(8),
            Self::Mjpeg => None,
        }
    }

    /// UVC format GUID; MJPEG has none (it uses its own descriptor type).
    pub fn guid(self) -> Option<[u8; 16]> {
        match self {
            Self::Yuyv => Some(fourcc_guid(b"YUY2")),
            Self::Uyvy => Some(fourcc_guid(b"UYVY")),
            Self::Nv12 => Some(fourcc_guid(b"NV12")),
            Self::Grey8 => Some(fourcc_guid(b"Y800")),
            Self::Mjpeg => None,
        }
    }

    /// Inverse of [`guid`](Self::guid); also accepts [`GUID_L8_IR`].
    pub fn from_guid(guid: &[u8; 16]) -> Option<Self> {
        if *guid == GUID_L8_IR {
            return Some(Self::Grey8);
        }
        [Self::Yuyv, Self::Uyvy, Self::Nv12, Self::Grey8]
            .into_iter()
            .find(|f| f.guid().as_ref() == Some(guid))
    }
}
