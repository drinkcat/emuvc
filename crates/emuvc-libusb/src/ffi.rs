//! `#[repr(C)]` mirrors of the public libusb-1.0 (1.0.30) ABI: structs, unions,
//! callback typedefs and constants.
//!
//! Constants use the C field type they are stored in: descriptor-related values
//! are `u8`/`u16`, error codes, statuses, speeds and other plain `enum` values are
//! `c_int`. Enum-typed struct fields are mirrored as `c_int` (all libusb enums are
//! 4 bytes wide).
#![allow(non_camel_case_types, non_upper_case_globals, non_snake_case)]

use core::ffi::{c_char, c_int, c_short, c_uchar, c_uint, c_void};

pub const LIBUSB_API_VERSION: u32 = 0x0100_010C;

// ---- sizes / masks / hotplug #defines ----
pub const LIBUSB_DT_DEVICE_SIZE: usize = 18;
pub const LIBUSB_DT_CONFIG_SIZE: usize = 9;
pub const LIBUSB_DT_INTERFACE_SIZE: usize = 9;
pub const LIBUSB_DT_ENDPOINT_SIZE: usize = 7;
pub const LIBUSB_DT_ENDPOINT_AUDIO_SIZE: usize = 9;
pub const LIBUSB_DT_HUB_NONVAR_SIZE: usize = 7;
pub const LIBUSB_DT_SS_ENDPOINT_COMPANION_SIZE: usize = 6;
pub const LIBUSB_DT_BOS_SIZE: usize = 5;
pub const LIBUSB_DT_DEVICE_CAPABILITY_SIZE: usize = 3;
pub const LIBUSB_DT_INTERFACE_ASSOCIATION_SIZE: usize = 8;
pub const LIBUSB_BT_USB_2_0_EXTENSION_SIZE: usize = 7;
pub const LIBUSB_BT_SS_USB_DEVICE_CAPABILITY_SIZE: usize = 10;
pub const LIBUSB_BT_SSPLUS_USB_DEVICE_CAPABILITY_SIZE: usize = 12;
pub const LIBUSB_BT_CONTAINER_ID_SIZE: usize = 20;
pub const LIBUSB_BT_PLATFORM_DESCRIPTOR_MIN_SIZE: usize = 20;
pub const LIBUSB_DT_BOS_MAX_SIZE: usize = LIBUSB_DT_BOS_SIZE
    + LIBUSB_BT_USB_2_0_EXTENSION_SIZE
    + LIBUSB_BT_SS_USB_DEVICE_CAPABILITY_SIZE
    + LIBUSB_BT_CONTAINER_ID_SIZE;
pub const LIBUSB_CONTROL_SETUP_SIZE: usize = 8;
pub const LIBUSB_ENDPOINT_ADDRESS_MASK: u8 = 0x0f;
pub const LIBUSB_ENDPOINT_DIR_MASK: u8 = 0x80;
pub const LIBUSB_TRANSFER_TYPE_MASK: u8 = 0x03;
pub const LIBUSB_ISO_SYNC_TYPE_MASK: u8 = 0x0c;
pub const LIBUSB_ISO_USAGE_TYPE_MASK: u8 = 0x30;
pub const LIBUSB_ERROR_COUNT: c_int = 14;
pub const LIBUSB_DEVICE_STRING_BYTES_MAX: usize = 384;
pub const LIBUSB_OPTION_WEAK_AUTHORITY: c_int = LIBUSB_OPTION_NO_DEVICE_DISCOVERY;
pub const LIBUSB_HOTPLUG_NO_FLAGS: c_int = 0;
pub const LIBUSB_HOTPLUG_MATCH_ANY: c_int = -1;
// typedef enum libusb_hotplug_event
pub const LIBUSB_HOTPLUG_EVENT_DEVICE_ARRIVED: c_int = 1 << 0;
pub const LIBUSB_HOTPLUG_EVENT_DEVICE_LEFT: c_int = 1 << 1;
// typedef enum libusb_hotplug_flag
pub const LIBUSB_HOTPLUG_ENUMERATE: c_int = 1 << 0;

// ---- enum values ----
// enum libusb_class_code
pub const LIBUSB_CLASS_PER_INTERFACE: u8 = 0x00;
pub const LIBUSB_CLASS_AUDIO: u8 = 0x01;
pub const LIBUSB_CLASS_COMM: u8 = 0x02;
pub const LIBUSB_CLASS_HID: u8 = 0x03;
pub const LIBUSB_CLASS_PHYSICAL: u8 = 0x05;
pub const LIBUSB_CLASS_IMAGE: u8 = 0x06;
pub const LIBUSB_CLASS_PTP: u8 = 0x06;
pub const LIBUSB_CLASS_PRINTER: u8 = 0x07;
pub const LIBUSB_CLASS_MASS_STORAGE: u8 = 0x08;
pub const LIBUSB_CLASS_HUB: u8 = 0x09;
pub const LIBUSB_CLASS_DATA: u8 = 0x0a;
pub const LIBUSB_CLASS_SMART_CARD: u8 = 0x0b;
pub const LIBUSB_CLASS_CONTENT_SECURITY: u8 = 0x0d;
pub const LIBUSB_CLASS_VIDEO: u8 = 0x0e;
pub const LIBUSB_CLASS_PERSONAL_HEALTHCARE: u8 = 0x0f;
pub const LIBUSB_CLASS_AUDIO_VIDEO: u8 = 0x10;
pub const LIBUSB_CLASS_BILLBOARD: u8 = 0x11;
pub const LIBUSB_CLASS_TYPE_C_BRIDGE: u8 = 0x12;
pub const LIBUSB_CLASS_BULK_DISPLAY_PROTOCOL: u8 = 0x13;
pub const LIBUSB_CLASS_MCTP: u8 = 0x14;
pub const LIBUSB_CLASS_I3C: u8 = 0x3c;
pub const LIBUSB_CLASS_DIAGNOSTIC_DEVICE: u8 = 0xdc;
pub const LIBUSB_CLASS_WIRELESS: u8 = 0xe0;
pub const LIBUSB_CLASS_MISCELLANEOUS: u8 = 0xef;
pub const LIBUSB_CLASS_APPLICATION: u8 = 0xfe;
pub const LIBUSB_CLASS_VENDOR_SPEC: u8 = 0xff;
// enum libusb_descriptor_type
pub const LIBUSB_DT_DEVICE: u8 = 0x01;
pub const LIBUSB_DT_CONFIG: u8 = 0x02;
pub const LIBUSB_DT_STRING: u8 = 0x03;
pub const LIBUSB_DT_INTERFACE: u8 = 0x04;
pub const LIBUSB_DT_ENDPOINT: u8 = 0x05;
pub const LIBUSB_DT_INTERFACE_ASSOCIATION: u8 = 0x0b;
pub const LIBUSB_DT_BOS: u8 = 0x0f;
pub const LIBUSB_DT_DEVICE_CAPABILITY: u8 = 0x10;
pub const LIBUSB_DT_HID: u8 = 0x21;
pub const LIBUSB_DT_REPORT: u8 = 0x22;
pub const LIBUSB_DT_PHYSICAL: u8 = 0x23;
pub const LIBUSB_DT_HUB: u8 = 0x29;
pub const LIBUSB_DT_SUPERSPEED_HUB: u8 = 0x2a;
pub const LIBUSB_DT_SS_ENDPOINT_COMPANION: u8 = 0x30;
// enum libusb_endpoint_direction
pub const LIBUSB_ENDPOINT_OUT: u8 = 0x00;
pub const LIBUSB_ENDPOINT_IN: u8 = 0x80;
// enum libusb_endpoint_transfer_type
pub const LIBUSB_ENDPOINT_TRANSFER_TYPE_CONTROL: u8 = 0x0;
pub const LIBUSB_ENDPOINT_TRANSFER_TYPE_ISOCHRONOUS: u8 = 0x1;
pub const LIBUSB_ENDPOINT_TRANSFER_TYPE_BULK: u8 = 0x2;
pub const LIBUSB_ENDPOINT_TRANSFER_TYPE_INTERRUPT: u8 = 0x3;
// enum libusb_standard_request
pub const LIBUSB_REQUEST_GET_STATUS: u8 = 0x00;
pub const LIBUSB_REQUEST_CLEAR_FEATURE: u8 = 0x01;
pub const LIBUSB_REQUEST_SET_FEATURE: u8 = 0x03;
pub const LIBUSB_REQUEST_SET_ADDRESS: u8 = 0x05;
pub const LIBUSB_REQUEST_GET_DESCRIPTOR: u8 = 0x06;
pub const LIBUSB_REQUEST_SET_DESCRIPTOR: u8 = 0x07;
pub const LIBUSB_REQUEST_GET_CONFIGURATION: u8 = 0x08;
pub const LIBUSB_REQUEST_SET_CONFIGURATION: u8 = 0x09;
pub const LIBUSB_REQUEST_GET_INTERFACE: u8 = 0x0a;
pub const LIBUSB_REQUEST_SET_INTERFACE: u8 = 0x0b;
pub const LIBUSB_REQUEST_SYNCH_FRAME: u8 = 0x0c;
pub const LIBUSB_REQUEST_SET_SEL: u8 = 0x30;
pub const LIBUSB_SET_ISOCH_DELAY: u8 = 0x31;
// enum libusb_request_type
pub const LIBUSB_REQUEST_TYPE_STANDARD: u8 = 0x00 << 5;
pub const LIBUSB_REQUEST_TYPE_CLASS: u8 = 0x01 << 5;
pub const LIBUSB_REQUEST_TYPE_VENDOR: u8 = 0x02 << 5;
pub const LIBUSB_REQUEST_TYPE_RESERVED: u8 = 0x03 << 5;
// enum libusb_request_recipient
pub const LIBUSB_RECIPIENT_DEVICE: u8 = 0x00;
pub const LIBUSB_RECIPIENT_INTERFACE: u8 = 0x01;
pub const LIBUSB_RECIPIENT_ENDPOINT: u8 = 0x02;
pub const LIBUSB_RECIPIENT_OTHER: u8 = 0x03;
// enum libusb_iso_sync_type
pub const LIBUSB_ISO_SYNC_TYPE_NONE: u8 = 0x0;
pub const LIBUSB_ISO_SYNC_TYPE_ASYNC: u8 = 0x1;
pub const LIBUSB_ISO_SYNC_TYPE_ADAPTIVE: u8 = 0x2;
pub const LIBUSB_ISO_SYNC_TYPE_SYNC: u8 = 0x3;
// enum libusb_iso_usage_type
pub const LIBUSB_ISO_USAGE_TYPE_DATA: u8 = 0x0;
pub const LIBUSB_ISO_USAGE_TYPE_FEEDBACK: u8 = 0x1;
pub const LIBUSB_ISO_USAGE_TYPE_IMPLICIT: u8 = 0x2;
// enum libusb_supported_speed
pub const LIBUSB_LOW_SPEED_OPERATION: u16 = 1 << 0;
pub const LIBUSB_FULL_SPEED_OPERATION: u16 = 1 << 1;
pub const LIBUSB_HIGH_SPEED_OPERATION: u16 = 1 << 2;
pub const LIBUSB_SUPER_SPEED_OPERATION: u16 = 1 << 3;
// enum libusb_usb_2_0_extension_attributes
pub const LIBUSB_BM_LPM_SUPPORT: u32 = 1 << 1;
// enum libusb_ss_usb_device_capability_attributes
pub const LIBUSB_BM_LTM_SUPPORT: u8 = 1 << 1;
// enum libusb_bos_type
pub const LIBUSB_BT_WIRELESS_USB_DEVICE_CAPABILITY: u8 = 0x01;
pub const LIBUSB_BT_USB_2_0_EXTENSION: u8 = 0x02;
pub const LIBUSB_BT_SS_USB_DEVICE_CAPABILITY: u8 = 0x03;
pub const LIBUSB_BT_CONTAINER_ID: u8 = 0x04;
pub const LIBUSB_BT_PLATFORM_DESCRIPTOR: u8 = 0x05;
pub const LIBUSB_BT_SUPERSPEED_PLUS_CAPABILITY: u8 = 0x0A;
// enum libusb_superspeedplus_sublink_attribute_sublink_type
pub const LIBUSB_SSPLUS_ATTR_TYPE_SYM: c_int = 0;
pub const LIBUSB_SSPLUS_ATTR_TYPE_ASYM: c_int = 1;
// enum libusb_superspeedplus_sublink_attribute_sublink_direction
pub const LIBUSB_SSPLUS_ATTR_DIR_RX: c_int = 0;
pub const LIBUSB_SSPLUS_ATTR_DIR_TX: c_int = 1;
// enum libusb_superspeedplus_sublink_attribute_exponent
pub const LIBUSB_SSPLUS_ATTR_EXP_BPS: c_int = 0;
pub const LIBUSB_SSPLUS_ATTR_EXP_KBS: c_int = 1;
pub const LIBUSB_SSPLUS_ATTR_EXP_MBS: c_int = 2;
pub const LIBUSB_SSPLUS_ATTR_EXP_GBS: c_int = 3;
// enum libusb_superspeedplus_sublink_attribute_link_protocol
pub const LIBUSB_SSPLUS_ATTR_PROT_SS: c_int = 0;
pub const LIBUSB_SSPLUS_ATTR_PROT_SSPLUS: c_int = 1;
// enum libusb_speed
pub const LIBUSB_SPEED_UNKNOWN: c_int = 0;
pub const LIBUSB_SPEED_LOW: c_int = 1;
pub const LIBUSB_SPEED_FULL: c_int = 2;
pub const LIBUSB_SPEED_HIGH: c_int = 3;
pub const LIBUSB_SPEED_SUPER: c_int = 4;
pub const LIBUSB_SPEED_SUPER_PLUS: c_int = 5;
pub const LIBUSB_SPEED_SUPER_PLUS_X2: c_int = 6;
// enum libusb_error
pub const LIBUSB_SUCCESS: c_int = 0;
pub const LIBUSB_ERROR_IO: c_int = -1;
pub const LIBUSB_ERROR_INVALID_PARAM: c_int = -2;
pub const LIBUSB_ERROR_ACCESS: c_int = -3;
pub const LIBUSB_ERROR_NO_DEVICE: c_int = -4;
pub const LIBUSB_ERROR_NOT_FOUND: c_int = -5;
pub const LIBUSB_ERROR_BUSY: c_int = -6;
pub const LIBUSB_ERROR_TIMEOUT: c_int = -7;
pub const LIBUSB_ERROR_OVERFLOW: c_int = -8;
pub const LIBUSB_ERROR_PIPE: c_int = -9;
pub const LIBUSB_ERROR_INTERRUPTED: c_int = -10;
pub const LIBUSB_ERROR_NO_MEM: c_int = -11;
pub const LIBUSB_ERROR_NOT_SUPPORTED: c_int = -12;
pub const LIBUSB_ERROR_OTHER: c_int = -99;
// enum libusb_transfer_type
pub const LIBUSB_TRANSFER_TYPE_CONTROL: u8 = 0;
pub const LIBUSB_TRANSFER_TYPE_ISOCHRONOUS: u8 = 1;
pub const LIBUSB_TRANSFER_TYPE_BULK: u8 = 2;
pub const LIBUSB_TRANSFER_TYPE_INTERRUPT: u8 = 3;
pub const LIBUSB_TRANSFER_TYPE_BULK_STREAM: u8 = 4;
// enum libusb_transfer_status
pub const LIBUSB_TRANSFER_COMPLETED: c_int = 0;
pub const LIBUSB_TRANSFER_ERROR: c_int = 1;
pub const LIBUSB_TRANSFER_TIMED_OUT: c_int = 2;
pub const LIBUSB_TRANSFER_CANCELLED: c_int = 3;
pub const LIBUSB_TRANSFER_STALL: c_int = 4;
pub const LIBUSB_TRANSFER_NO_DEVICE: c_int = 5;
pub const LIBUSB_TRANSFER_OVERFLOW: c_int = 6;
// enum libusb_transfer_flags
pub const LIBUSB_TRANSFER_SHORT_NOT_OK: u8 = 1 << 0;
pub const LIBUSB_TRANSFER_FREE_BUFFER: u8 = 1 << 1;
pub const LIBUSB_TRANSFER_FREE_TRANSFER: u8 = 1 << 2;
pub const LIBUSB_TRANSFER_ADD_ZERO_PACKET: u8 = 1 << 3;
// enum libusb_capability
pub const LIBUSB_CAP_HAS_CAPABILITY: u32 = 0x0000;
pub const LIBUSB_CAP_HAS_HOTPLUG: u32 = 0x0001;
pub const LIBUSB_CAP_HAS_HID_ACCESS: u32 = 0x0100;
pub const LIBUSB_CAP_SUPPORTS_DETACH_KERNEL_DRIVER: u32 = 0x0101;
// enum libusb_log_level
pub const LIBUSB_LOG_LEVEL_NONE: c_int = 0;
pub const LIBUSB_LOG_LEVEL_ERROR: c_int = 1;
pub const LIBUSB_LOG_LEVEL_WARNING: c_int = 2;
pub const LIBUSB_LOG_LEVEL_INFO: c_int = 3;
pub const LIBUSB_LOG_LEVEL_DEBUG: c_int = 4;
// enum libusb_log_cb_mode
pub const LIBUSB_LOG_CB_GLOBAL: c_int = 1 << 0;
pub const LIBUSB_LOG_CB_CONTEXT: c_int = 1 << 1;
// enum libusb_option
pub const LIBUSB_OPTION_LOG_LEVEL: c_int = 0;
pub const LIBUSB_OPTION_USE_USBDK: c_int = 1;
pub const LIBUSB_OPTION_NO_DEVICE_DISCOVERY: c_int = 2;
pub const LIBUSB_OPTION_LOG_CB: c_int = 3;
pub const LIBUSB_OPTION_MAX: c_int = 4;
// enum libusb_device_string_type
pub const LIBUSB_DEVICE_STRING_MANUFACTURER: c_int = 0;
pub const LIBUSB_DEVICE_STRING_PRODUCT: c_int = 1;
pub const LIBUSB_DEVICE_STRING_SERIAL_NUMBER: c_int = 2;
pub const LIBUSB_DEVICE_STRING_COUNT: c_int = 3;

// ---- opaque handles ----
#[repr(C)]
pub struct libusb_context {
    _private: [u8; 0],
}
#[repr(C)]
pub struct libusb_device {
    _private: [u8; 0],
}
#[repr(C)]
pub struct libusb_device_handle {
    _private: [u8; 0],
}

// ---- descriptors ----
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct libusb_device_descriptor {
    pub bLength: u8,
    pub bDescriptorType: u8,
    pub bcdUSB: u16,
    pub bDeviceClass: u8,
    pub bDeviceSubClass: u8,
    pub bDeviceProtocol: u8,
    pub bMaxPacketSize0: u8,
    pub idVendor: u16,
    pub idProduct: u16,
    pub bcdDevice: u16,
    pub iManufacturer: u8,
    pub iProduct: u8,
    pub iSerialNumber: u8,
    pub bNumConfigurations: u8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct libusb_endpoint_descriptor {
    pub bLength: u8,
    pub bDescriptorType: u8,
    pub bEndpointAddress: u8,
    pub bmAttributes: u8,
    pub wMaxPacketSize: u16,
    pub bInterval: u8,
    pub bRefresh: u8,
    pub bSynchAddress: u8,
    pub extra: *const c_uchar,
    pub extra_length: c_int,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct libusb_interface_association_descriptor {
    pub bLength: u8,
    pub bDescriptorType: u8,
    pub bFirstInterface: u8,
    pub bInterfaceCount: u8,
    pub bFunctionClass: u8,
    pub bFunctionSubClass: u8,
    pub bFunctionProtocol: u8,
    pub iFunction: u8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct libusb_interface_association_descriptor_array {
    pub iad: *const libusb_interface_association_descriptor,
    pub length: c_int,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct libusb_interface_descriptor {
    pub bLength: u8,
    pub bDescriptorType: u8,
    pub bInterfaceNumber: u8,
    pub bAlternateSetting: u8,
    pub bNumEndpoints: u8,
    pub bInterfaceClass: u8,
    pub bInterfaceSubClass: u8,
    pub bInterfaceProtocol: u8,
    pub iInterface: u8,
    pub endpoint: *const libusb_endpoint_descriptor,
    pub extra: *const c_uchar,
    pub extra_length: c_int,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct libusb_interface {
    pub altsetting: *const libusb_interface_descriptor,
    pub num_altsetting: c_int,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct libusb_config_descriptor {
    pub bLength: u8,
    pub bDescriptorType: u8,
    pub wTotalLength: u16,
    pub bNumInterfaces: u8,
    pub bConfigurationValue: u8,
    pub iConfiguration: u8,
    pub bmAttributes: u8,
    pub MaxPower: u8,
    pub interface: *const libusb_interface,
    pub extra: *const c_uchar,
    pub extra_length: c_int,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct libusb_ss_endpoint_companion_descriptor {
    pub bLength: u8,
    pub bDescriptorType: u8,
    pub bMaxBurst: u8,
    pub bmAttributes: u8,
    pub wBytesPerInterval: u16,
}

#[repr(C)]
#[derive(Debug)]
pub struct libusb_bos_dev_capability_descriptor {
    pub bLength: u8,
    pub bDescriptorType: u8,
    pub bDevCapabilityType: u8,
    pub dev_capability_data: [u8; 0],
}

#[repr(C)]
#[derive(Debug)]
pub struct libusb_bos_descriptor {
    pub bLength: u8,
    pub bDescriptorType: u8,
    pub wTotalLength: u16,
    pub bNumDeviceCaps: u8,
    pub dev_capability: [*mut libusb_bos_dev_capability_descriptor; 0],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct libusb_usb_2_0_extension_descriptor {
    pub bLength: u8,
    pub bDescriptorType: u8,
    pub bDevCapabilityType: u8,
    pub bmAttributes: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct libusb_ss_usb_device_capability_descriptor {
    pub bLength: u8,
    pub bDescriptorType: u8,
    pub bDevCapabilityType: u8,
    pub bmAttributes: u8,
    pub wSpeedSupported: u16,
    pub bFunctionalitySupport: u8,
    pub bU1DevExitLat: u8,
    pub bU2DevExitLat: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct libusb_ssplus_sublink_attribute {
    pub ssid: u8,
    pub exponent: c_int,
    pub type_: c_int,
    pub direction: c_int,
    pub protocol: c_int,
    pub mantissa: u16,
}

#[repr(C)]
#[derive(Debug)]
pub struct libusb_ssplus_usb_device_capability_descriptor {
    pub numSublinkSpeedAttributes: u8,
    pub numSublinkSpeedIDs: u8,
    pub ssid: u8,
    pub minRxLaneCount: u8,
    pub minTxLaneCount: u8,
    pub sublinkSpeedAttributes: [libusb_ssplus_sublink_attribute; 0],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct libusb_container_id_descriptor {
    pub bLength: u8,
    pub bDescriptorType: u8,
    pub bDevCapabilityType: u8,
    pub bReserved: u8,
    pub ContainerID: [u8; 16],
}

#[repr(C)]
#[derive(Debug)]
pub struct libusb_platform_descriptor {
    pub bLength: u8,
    pub bDescriptorType: u8,
    pub bDevCapabilityType: u8,
    pub bReserved: u8,
    pub PlatformCapabilityUUID: [u8; 16],
    pub CapabilityData: [u8; 0],
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct libusb_control_setup {
    pub bmRequestType: u8,
    pub bRequest: u8,
    pub wValue: u16,
    pub wIndex: u16,
    pub wLength: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct libusb_version {
    pub major: u16,
    pub minor: u16,
    pub micro: u16,
    pub nano: u16,
    pub rc: *const c_char,
    pub describe: *const c_char,
}
// Contains only pointers to 'static C strings when used by misc.rs.
unsafe impl Sync for libusb_version {}

// ---- transfers ----
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct libusb_iso_packet_descriptor {
    pub length: c_uint,
    pub actual_length: c_uint,
    /// `enum libusb_transfer_status`
    pub status: c_int,
}

pub type libusb_transfer_cb_fn = Option<unsafe extern "C" fn(transfer: *mut libusb_transfer)>;

#[repr(C)]
pub struct libusb_transfer {
    pub dev_handle: *mut libusb_device_handle,
    pub flags: u8,
    pub endpoint: c_uchar,
    pub type_: c_uchar,
    pub timeout: c_uint,
    /// `enum libusb_transfer_status`
    pub status: c_int,
    pub length: c_int,
    pub actual_length: c_int,
    pub callback: libusb_transfer_cb_fn,
    pub user_data: *mut c_void,
    pub buffer: *mut c_uchar,
    pub num_iso_packets: c_int,
    pub iso_packet_desc: [libusb_iso_packet_descriptor; 0],
}

// ---- logging / init ----
pub type libusb_log_cb =
    Option<unsafe extern "C" fn(ctx: *mut libusb_context, level: c_int, str_: *const c_char)>;

#[repr(C)]
#[derive(Clone, Copy)]
pub union libusb_init_option_value {
    pub ival: c_int,
    pub log_cbval: libusb_log_cb,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct libusb_init_option {
    /// `enum libusb_option`
    pub option: c_int,
    pub value: libusb_init_option_value,
}

// ---- polling / hotplug ----
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct libusb_pollfd {
    pub fd: c_int,
    pub events: c_short,
}

pub type libusb_pollfd_added_cb =
    Option<unsafe extern "C" fn(fd: c_int, events: c_short, user_data: *mut c_void)>;
pub type libusb_pollfd_removed_cb = Option<unsafe extern "C" fn(fd: c_int, user_data: *mut c_void)>;

pub type libusb_hotplug_callback_handle = c_int;
pub type libusb_hotplug_event = c_int;
pub type libusb_hotplug_flag = c_int;
pub type libusb_hotplug_callback_fn = Option<
    unsafe extern "C" fn(
        ctx: *mut libusb_context,
        device: *mut libusb_device,
        event: libusb_hotplug_event,
        user_data: *mut c_void,
    ) -> c_int,
>;
