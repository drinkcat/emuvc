//! UVC 1.5 constants (class specification appendices A and B, payload headers,
//! probe/commit fields).

// A.1 - A.3: class, subclass and protocol codes.
pub const CC_VIDEO: u8 = 0x0e;
pub const SC_UNDEFINED: u8 = 0x00;
pub const SC_VIDEOCONTROL: u8 = 0x01;
pub const SC_VIDEOSTREAMING: u8 = 0x02;
pub const SC_VIDEO_INTERFACE_COLLECTION: u8 = 0x03;
pub const PC_PROTOCOL_UNDEFINED: u8 = 0x00;
pub const PC_PROTOCOL_15: u8 = 0x01;

// A.4: class-specific descriptor types.
pub const CS_UNDEFINED: u8 = 0x20;
pub const CS_DEVICE: u8 = 0x21;
pub const CS_CONFIGURATION: u8 = 0x22;
pub const CS_STRING: u8 = 0x23;
pub const CS_INTERFACE: u8 = 0x24;
pub const CS_ENDPOINT: u8 = 0x25;

// A.5: VC interface descriptor subtypes.
pub const VC_DESCRIPTOR_UNDEFINED: u8 = 0x00;
pub const VC_HEADER: u8 = 0x01;
pub const VC_INPUT_TERMINAL: u8 = 0x02;
pub const VC_OUTPUT_TERMINAL: u8 = 0x03;
pub const VC_SELECTOR_UNIT: u8 = 0x04;
pub const VC_PROCESSING_UNIT: u8 = 0x05;
pub const VC_EXTENSION_UNIT: u8 = 0x06;
pub const VC_ENCODING_UNIT: u8 = 0x07;

// A.6: VS interface descriptor subtypes.
pub const VS_UNDEFINED: u8 = 0x00;
pub const VS_INPUT_HEADER: u8 = 0x01;
pub const VS_OUTPUT_HEADER: u8 = 0x02;
pub const VS_STILL_IMAGE_FRAME: u8 = 0x03;
pub const VS_FORMAT_UNCOMPRESSED: u8 = 0x04;
pub const VS_FRAME_UNCOMPRESSED: u8 = 0x05;
pub const VS_FORMAT_MJPEG: u8 = 0x06;
pub const VS_FRAME_MJPEG: u8 = 0x07;
pub const VS_FORMAT_MPEG2TS: u8 = 0x0a;
pub const VS_FORMAT_DV: u8 = 0x0c;
pub const VS_COLORFORMAT: u8 = 0x0d;
pub const VS_FORMAT_FRAME_BASED: u8 = 0x10;
pub const VS_FRAME_FRAME_BASED: u8 = 0x11;
pub const VS_FORMAT_STREAM_BASED: u8 = 0x12;
pub const VS_FORMAT_H264: u8 = 0x13;
pub const VS_FRAME_H264: u8 = 0x14;
pub const VS_FORMAT_H264_SIMULCAST: u8 = 0x15;
pub const VS_FORMAT_VP8: u8 = 0x16;
pub const VS_FRAME_VP8: u8 = 0x17;
pub const VS_FORMAT_VP8_SIMULCAST: u8 = 0x18;

// A.7: class-specific endpoint descriptor subtypes.
pub const EP_UNDEFINED: u8 = 0x00;
pub const EP_GENERAL: u8 = 0x01;
pub const EP_ENDPOINT: u8 = 0x02;
pub const EP_INTERRUPT: u8 = 0x03;

// A.8: request codes.
pub const RC_UNDEFINED: u8 = 0x00;
pub const SET_CUR: u8 = 0x01;
pub const SET_CUR_ALL: u8 = 0x11;
pub const GET_CUR: u8 = 0x81;
pub const GET_MIN: u8 = 0x82;
pub const GET_MAX: u8 = 0x83;
pub const GET_RES: u8 = 0x84;
pub const GET_LEN: u8 = 0x85;
pub const GET_INFO: u8 = 0x86;
pub const GET_DEF: u8 = 0x87;
pub const GET_CUR_ALL: u8 = 0x91;
pub const GET_MIN_ALL: u8 = 0x92;
pub const GET_MAX_ALL: u8 = 0x93;
pub const GET_RES_ALL: u8 = 0x94;
pub const GET_DEF_ALL: u8 = 0x97;

// GET_INFO bits (4.1.2).
pub const INFO_GET: u8 = 1 << 0;
pub const INFO_SET: u8 = 1 << 1;
pub const INFO_DISABLED: u8 = 1 << 2;
pub const INFO_AUTOUPDATE: u8 = 1 << 3;
pub const INFO_ASYNC: u8 = 1 << 4;

// A.9.1: VideoControl interface control selectors.
pub const VC_VIDEO_POWER_MODE_CONTROL: u8 = 0x01;
pub const VC_REQUEST_ERROR_CODE_CONTROL: u8 = 0x02;

// A.9.3: selector unit.
pub const SU_INPUT_SELECT_CONTROL: u8 = 0x01;

// A.9.4: camera terminal.
pub const CT_SCANNING_MODE_CONTROL: u8 = 0x01;
pub const CT_AE_MODE_CONTROL: u8 = 0x02;
pub const CT_AE_PRIORITY_CONTROL: u8 = 0x03;
pub const CT_EXPOSURE_TIME_ABSOLUTE_CONTROL: u8 = 0x04;
pub const CT_EXPOSURE_TIME_RELATIVE_CONTROL: u8 = 0x05;
pub const CT_FOCUS_ABSOLUTE_CONTROL: u8 = 0x06;
pub const CT_FOCUS_RELATIVE_CONTROL: u8 = 0x07;
pub const CT_FOCUS_AUTO_CONTROL: u8 = 0x08;
pub const CT_IRIS_ABSOLUTE_CONTROL: u8 = 0x09;
pub const CT_IRIS_RELATIVE_CONTROL: u8 = 0x0a;
pub const CT_ZOOM_ABSOLUTE_CONTROL: u8 = 0x0b;
pub const CT_ZOOM_RELATIVE_CONTROL: u8 = 0x0c;
pub const CT_PANTILT_ABSOLUTE_CONTROL: u8 = 0x0d;
pub const CT_PANTILT_RELATIVE_CONTROL: u8 = 0x0e;
pub const CT_ROLL_ABSOLUTE_CONTROL: u8 = 0x0f;
pub const CT_ROLL_RELATIVE_CONTROL: u8 = 0x10;
pub const CT_PRIVACY_CONTROL: u8 = 0x11;
pub const CT_FOCUS_SIMPLE_CONTROL: u8 = 0x12;
pub const CT_WINDOW_CONTROL: u8 = 0x13;
pub const CT_REGION_OF_INTEREST_CONTROL: u8 = 0x14;

// A.9.5: processing unit.
pub const PU_BACKLIGHT_COMPENSATION_CONTROL: u8 = 0x01;
pub const PU_BRIGHTNESS_CONTROL: u8 = 0x02;
pub const PU_CONTRAST_CONTROL: u8 = 0x03;
pub const PU_GAIN_CONTROL: u8 = 0x04;
pub const PU_POWER_LINE_FREQUENCY_CONTROL: u8 = 0x05;
pub const PU_HUE_CONTROL: u8 = 0x06;
pub const PU_SATURATION_CONTROL: u8 = 0x07;
pub const PU_SHARPNESS_CONTROL: u8 = 0x08;
pub const PU_GAMMA_CONTROL: u8 = 0x09;
pub const PU_WHITE_BALANCE_TEMPERATURE_CONTROL: u8 = 0x0a;
pub const PU_WHITE_BALANCE_TEMPERATURE_AUTO_CONTROL: u8 = 0x0b;
pub const PU_WHITE_BALANCE_COMPONENT_CONTROL: u8 = 0x0c;
pub const PU_WHITE_BALANCE_COMPONENT_AUTO_CONTROL: u8 = 0x0d;
pub const PU_DIGITAL_MULTIPLIER_CONTROL: u8 = 0x0e;
pub const PU_DIGITAL_MULTIPLIER_LIMIT_CONTROL: u8 = 0x0f;
pub const PU_HUE_AUTO_CONTROL: u8 = 0x10;
pub const PU_ANALOG_VIDEO_STANDARD_CONTROL: u8 = 0x11;
pub const PU_ANALOG_LOCK_STATUS_CONTROL: u8 = 0x12;
pub const PU_CONTRAST_AUTO_CONTROL: u8 = 0x13;

// A.9.6: encoding unit.
pub const EU_SELECT_LAYER_CONTROL: u8 = 0x01;
pub const EU_PROFILE_TOOLSET_CONTROL: u8 = 0x02;
pub const EU_VIDEO_RESOLUTION_CONTROL: u8 = 0x03;
pub const EU_MIN_FRAME_INTERVAL_CONTROL: u8 = 0x04;
pub const EU_SLICE_MODE_CONTROL: u8 = 0x05;
pub const EU_RATE_CONTROL_MODE_CONTROL: u8 = 0x06;
pub const EU_AVERAGE_BITRATE_CONTROL: u8 = 0x07;
pub const EU_CPB_SIZE_CONTROL: u8 = 0x08;
pub const EU_PEAK_BIT_RATE_CONTROL: u8 = 0x09;
pub const EU_QUANTIZATION_PARAMS_CONTROL: u8 = 0x0a;
pub const EU_SYNC_REF_FRAME_CONTROL: u8 = 0x0b;
pub const EU_LTR_BUFFER_CONTROL: u8 = 0x0c;
pub const EU_LTR_PICTURE_CONTROL: u8 = 0x0d;
pub const EU_LTR_VALIDATION_CONTROL: u8 = 0x0e;
pub const EU_LEVEL_IDC_LIMIT_CONTROL: u8 = 0x0f;
pub const EU_SEI_PAYLOADTYPE_CONTROL: u8 = 0x10;
pub const EU_QP_RANGE_CONTROL: u8 = 0x11;
pub const EU_PRIORITY_CONTROL: u8 = 0x12;
pub const EU_START_OR_STOP_LAYER_CONTROL: u8 = 0x13;
pub const EU_ERROR_RESILIENCY_CONTROL: u8 = 0x14;

// A.9.8: VideoStreaming interface control selectors.
pub const VS_PROBE_CONTROL: u8 = 0x01;
pub const VS_COMMIT_CONTROL: u8 = 0x02;
pub const VS_STILL_PROBE_CONTROL: u8 = 0x03;
pub const VS_STILL_COMMIT_CONTROL: u8 = 0x04;
pub const VS_STILL_IMAGE_TRIGGER_CONTROL: u8 = 0x05;
pub const VS_STREAM_ERROR_CODE_CONTROL: u8 = 0x06;
pub const VS_GENERATE_KEY_FRAME_CONTROL: u8 = 0x07;
pub const VS_UPDATE_FRAME_SEGMENT_CONTROL: u8 = 0x08;
pub const VS_SYNCH_DELAY_CONTROL: u8 = 0x09;

// Appendix B: terminal types.
pub const TT_VENDOR_SPECIFIC: u16 = 0x0100;
pub const TT_STREAMING: u16 = 0x0101;
pub const ITT_VENDOR_SPECIFIC: u16 = 0x0200;
pub const ITT_CAMERA: u16 = 0x0201;
pub const ITT_MEDIA_TRANSPORT_INPUT: u16 = 0x0202;
pub const OTT_VENDOR_SPECIFIC: u16 = 0x0300;
pub const OTT_DISPLAY: u16 = 0x0301;
pub const OTT_MEDIA_TRANSPORT_OUTPUT: u16 = 0x0302;
pub const EXTERNAL_VENDOR_SPECIFIC: u16 = 0x0400;
pub const COMPOSITE_CONNECTOR: u16 = 0x0401;
pub const SVIDEO_CONNECTOR: u16 = 0x0402;
pub const COMPONENT_CONNECTOR: u16 = 0x0403;

// 4.2.1.2: VC_REQUEST_ERROR_CODE_CONTROL values.
pub const ERR_NO_ERROR: u8 = 0x00;
pub const ERR_NOT_READY: u8 = 0x01;
pub const ERR_WRONG_STATE: u8 = 0x02;
pub const ERR_POWER: u8 = 0x03;
pub const ERR_OUT_OF_RANGE: u8 = 0x04;
pub const ERR_INVALID_UNIT: u8 = 0x05;
pub const ERR_INVALID_CONTROL: u8 = 0x06;
pub const ERR_INVALID_REQUEST: u8 = 0x07;
pub const ERR_INVALID_VALUE_WITHIN_RANGE: u8 = 0x08;
pub const ERR_UNKNOWN: u8 = 0xff;

// 4.3.1.7: VS_STREAM_ERROR_CODE_CONTROL values.
pub const STREAM_ERR_NONE: u8 = 0;
pub const STREAM_ERR_PROTECTED_CONTENT: u8 = 1;
pub const STREAM_ERR_INPUT_BUFFER_UNDERRUN: u8 = 2;
pub const STREAM_ERR_DATA_DISCONTINUITY: u8 = 3;
pub const STREAM_ERR_OUTPUT_BUFFER_UNDERRUN: u8 = 4;
pub const STREAM_ERR_OUTPUT_BUFFER_OVERRUN: u8 = 5;
pub const STREAM_ERR_FORMAT_CHANGE: u8 = 6;
pub const STREAM_ERR_STILL_IMAGE_CAPTURE_ERROR: u8 = 7;

// 2.4.2.2: status interrupt packet (tables 2-1 .. 2-3).
pub const STATUS_ORIGINATOR_MASK: u8 = 0x0f;
pub const STATUS_ORIGINATOR_VC: u8 = 1;
pub const STATUS_ORIGINATOR_VS: u8 = 2;
/// VC originator `bEvent`: control change.
pub const STATUS_EVENT_CONTROL_CHANGE: u8 = 0x00;
/// VS originator `bEvent`: button press (values 1..=255 are stream errors).
pub const STATUS_EVENT_BUTTON_PRESS: u8 = 0x00;
pub const STATUS_ATTR_VALUE: u8 = 0x00;
pub const STATUS_ATTR_INFO: u8 = 0x01;
pub const STATUS_ATTR_FAILURE: u8 = 0x02;
pub const STATUS_ATTR_MIN: u8 = 0x03;
pub const STATUS_ATTR_MAX: u8 = 0x04;

// 2.4.3.3: payload header `bmHeaderInfo` bits.
pub const HDR_FID: u8 = 1 << 0;
pub const HDR_EOF: u8 = 1 << 1;
pub const HDR_PTS: u8 = 1 << 2;
pub const HDR_SCR: u8 = 1 << 3;
pub const HDR_RES: u8 = 1 << 4;
pub const HDR_STI: u8 = 1 << 5;
pub const HDR_ERR: u8 = 1 << 6;
pub const HDR_EOH: u8 = 1 << 7;

// Probe/commit `bmHint` bits.
pub const HINT_FRAME_INTERVAL: u16 = 1 << 0;
pub const HINT_KEY_FRAME_RATE: u16 = 1 << 1;
pub const HINT_P_FRAME_RATE: u16 = 1 << 2;
pub const HINT_COMP_QUALITY: u16 = 1 << 3;
pub const HINT_COMP_WINDOW_SIZE: u16 = 1 << 4;

// Probe/commit `bmFramingInfo` bits.
pub const FRAMING_FID: u8 = 1 << 0;
pub const FRAMING_EOF: u8 = 1 << 1;
pub const FRAMING_EOS: u8 = 1 << 2;

/// Selector for bit `bit` of a Camera Terminal `bmControls` (table 3-6).
pub fn ct_selector(bit: u32) -> Option<u8> {
    Some(match bit {
        0..=6 => bit as u8 + 1,  // scanning mode .. focus (relative)
        7..=14 => bit as u8 + 2, // iris (absolute) .. roll (relative); selector 8 is Focus Auto
        17 => CT_FOCUS_AUTO_CONTROL,
        18 => CT_PRIVACY_CONTROL,
        19 => CT_FOCUS_SIMPLE_CONTROL,
        20 => CT_WINDOW_CONTROL,
        21 => CT_REGION_OF_INTEREST_CONTROL,
        _ => return None,
    })
}

/// Selector for bit `bit` of a Processing Unit `bmControls` (table 3-8).
pub fn pu_selector(bit: u32) -> Option<u8> {
    Some(match bit {
        0 => PU_BRIGHTNESS_CONTROL,
        1 => PU_CONTRAST_CONTROL,
        2 => PU_HUE_CONTROL,
        3 => PU_SATURATION_CONTROL,
        4 => PU_SHARPNESS_CONTROL,
        5 => PU_GAMMA_CONTROL,
        6 => PU_WHITE_BALANCE_TEMPERATURE_CONTROL,
        7 => PU_WHITE_BALANCE_COMPONENT_CONTROL,
        8 => PU_BACKLIGHT_COMPENSATION_CONTROL,
        9 => PU_GAIN_CONTROL,
        10 => PU_POWER_LINE_FREQUENCY_CONTROL,
        11 => PU_HUE_AUTO_CONTROL,
        12 => PU_WHITE_BALANCE_TEMPERATURE_AUTO_CONTROL,
        13 => PU_WHITE_BALANCE_COMPONENT_AUTO_CONTROL,
        14 => PU_DIGITAL_MULTIPLIER_CONTROL,
        15 => PU_DIGITAL_MULTIPLIER_LIMIT_CONTROL,
        16 => PU_ANALOG_VIDEO_STANDARD_CONTROL,
        17 => PU_ANALOG_LOCK_STATUS_CONTROL,
        18 => PU_CONTRAST_AUTO_CONTROL,
        _ => return None,
    })
}

/// Selector for bit `bit` of an Encoding Unit `bmControls` (table 3-9): bit n = selector n+1.
pub fn eu_selector(bit: u32) -> Option<u8> {
    (bit <= 19).then(|| bit as u8 + 1)
}

/// Which kind of entity (or interface) a control selector belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntityKindTag {
    Camera,
    Processing,
    Selector,
    Extension,
    Encoding,
    /// The VideoControl interface itself (entity 0).
    Interface,
    /// A VideoStreaming interface.
    Streaming,
}

/// Human-readable control name for logs.
pub fn control_name(kind: EntityKindTag, selector: u8) -> &'static str {
    use EntityKindTag::*;
    match (kind, selector) {
        (Interface, 0x01) => "VIDEO_POWER_MODE",
        (Interface, 0x02) => "REQUEST_ERROR_CODE",
        (Selector, 0x01) => "SU_INPUT_SELECT",
        (Camera, 0x01) => "CT_SCANNING_MODE",
        (Camera, 0x02) => "CT_AE_MODE",
        (Camera, 0x03) => "CT_AE_PRIORITY",
        (Camera, 0x04) => "CT_EXPOSURE_TIME_ABSOLUTE",
        (Camera, 0x05) => "CT_EXPOSURE_TIME_RELATIVE",
        (Camera, 0x06) => "CT_FOCUS_ABSOLUTE",
        (Camera, 0x07) => "CT_FOCUS_RELATIVE",
        (Camera, 0x08) => "CT_FOCUS_AUTO",
        (Camera, 0x09) => "CT_IRIS_ABSOLUTE",
        (Camera, 0x0a) => "CT_IRIS_RELATIVE",
        (Camera, 0x0b) => "CT_ZOOM_ABSOLUTE",
        (Camera, 0x0c) => "CT_ZOOM_RELATIVE",
        (Camera, 0x0d) => "CT_PANTILT_ABSOLUTE",
        (Camera, 0x0e) => "CT_PANTILT_RELATIVE",
        (Camera, 0x0f) => "CT_ROLL_ABSOLUTE",
        (Camera, 0x10) => "CT_ROLL_RELATIVE",
        (Camera, 0x11) => "CT_PRIVACY",
        (Camera, 0x12) => "CT_FOCUS_SIMPLE",
        (Camera, 0x13) => "CT_WINDOW",
        (Camera, 0x14) => "CT_REGION_OF_INTEREST",
        (Processing, 0x01) => "PU_BACKLIGHT_COMPENSATION",
        (Processing, 0x02) => "PU_BRIGHTNESS",
        (Processing, 0x03) => "PU_CONTRAST",
        (Processing, 0x04) => "PU_GAIN",
        (Processing, 0x05) => "PU_POWER_LINE_FREQUENCY",
        (Processing, 0x06) => "PU_HUE",
        (Processing, 0x07) => "PU_SATURATION",
        (Processing, 0x08) => "PU_SHARPNESS",
        (Processing, 0x09) => "PU_GAMMA",
        (Processing, 0x0a) => "PU_WHITE_BALANCE_TEMPERATURE",
        (Processing, 0x0b) => "PU_WHITE_BALANCE_TEMPERATURE_AUTO",
        (Processing, 0x0c) => "PU_WHITE_BALANCE_COMPONENT",
        (Processing, 0x0d) => "PU_WHITE_BALANCE_COMPONENT_AUTO",
        (Processing, 0x0e) => "PU_DIGITAL_MULTIPLIER",
        (Processing, 0x0f) => "PU_DIGITAL_MULTIPLIER_LIMIT",
        (Processing, 0x10) => "PU_HUE_AUTO",
        (Processing, 0x11) => "PU_ANALOG_VIDEO_STANDARD",
        (Processing, 0x12) => "PU_ANALOG_LOCK_STATUS",
        (Processing, 0x13) => "PU_CONTRAST_AUTO",
        (Encoding, 0x01) => "EU_SELECT_LAYER",
        (Encoding, 0x02) => "EU_PROFILE_TOOLSET",
        (Encoding, 0x03) => "EU_VIDEO_RESOLUTION",
        (Encoding, 0x04) => "EU_MIN_FRAME_INTERVAL",
        (Encoding, 0x05) => "EU_SLICE_MODE",
        (Encoding, 0x06) => "EU_RATE_CONTROL_MODE",
        (Encoding, 0x07) => "EU_AVERAGE_BITRATE",
        (Encoding, 0x08) => "EU_CPB_SIZE",
        (Encoding, 0x09) => "EU_PEAK_BIT_RATE",
        (Encoding, 0x0a) => "EU_QUANTIZATION_PARAMS",
        (Encoding, 0x0b) => "EU_SYNC_REF_FRAME",
        (Encoding, 0x0c) => "EU_LTR_BUFFER",
        (Encoding, 0x0d) => "EU_LTR_PICTURE",
        (Encoding, 0x0e) => "EU_LTR_VALIDATION",
        (Encoding, 0x0f) => "EU_LEVEL_IDC_LIMIT",
        (Encoding, 0x10) => "EU_SEI_PAYLOADTYPE",
        (Encoding, 0x11) => "EU_QP_RANGE",
        (Encoding, 0x12) => "EU_PRIORITY",
        (Encoding, 0x13) => "EU_START_OR_STOP_LAYER",
        (Encoding, 0x14) => "EU_ERROR_RESILIENCY",
        (Streaming, 0x01) => "VS_PROBE",
        (Streaming, 0x02) => "VS_COMMIT",
        (Streaming, 0x03) => "VS_STILL_PROBE",
        (Streaming, 0x04) => "VS_STILL_COMMIT",
        (Streaming, 0x05) => "VS_STILL_IMAGE_TRIGGER",
        (Streaming, 0x06) => "VS_STREAM_ERROR_CODE",
        (Streaming, 0x07) => "VS_GENERATE_KEY_FRAME",
        (Streaming, 0x08) => "VS_UPDATE_FRAME_SEGMENT",
        (Streaming, 0x09) => "VS_SYNCH_DELAY",
        (Extension, _) => "XU_CONTROL",
        _ => "UNKNOWN_CONTROL",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_table() {
        assert_eq!(ct_selector(0), Some(0x01));
        assert_eq!(ct_selector(1), Some(CT_AE_MODE_CONTROL));
        assert_eq!(ct_selector(6), Some(CT_FOCUS_RELATIVE_CONTROL));
        assert_eq!(ct_selector(7), Some(CT_IRIS_ABSOLUTE_CONTROL));
        assert_eq!(ct_selector(14), Some(CT_ROLL_RELATIVE_CONTROL));
        assert_eq!(ct_selector(15), None);
        assert_eq!(ct_selector(17), Some(CT_FOCUS_AUTO_CONTROL));
        assert_eq!(ct_selector(18), Some(CT_PRIVACY_CONTROL));
        assert_eq!(ct_selector(21), Some(CT_REGION_OF_INTEREST_CONTROL));
        assert_eq!(ct_selector(22), None);
    }

    #[test]
    fn pu_table() {
        assert_eq!(pu_selector(0), Some(PU_BRIGHTNESS_CONTROL));
        assert_eq!(pu_selector(8), Some(PU_BACKLIGHT_COMPENSATION_CONTROL));
        assert_eq!(
            pu_selector(12),
            Some(PU_WHITE_BALANCE_TEMPERATURE_AUTO_CONTROL)
        );
        assert_eq!(pu_selector(19), None);
        assert_eq!(
            control_name(EntityKindTag::Processing, 0x02),
            "PU_BRIGHTNESS"
        );
    }
}
