//! Small, stateless libusb entry points: error names/strings, version, capabilities.

use crate::ffi::*;
use core::ffi::{c_char, c_int};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_error_name(error_code: c_int) -> *const c_char {
    let s: &'static [u8] = match error_code {
        LIBUSB_ERROR_IO => b"LIBUSB_ERROR_IO\0",
        LIBUSB_ERROR_INVALID_PARAM => b"LIBUSB_ERROR_INVALID_PARAM\0",
        LIBUSB_ERROR_ACCESS => b"LIBUSB_ERROR_ACCESS\0",
        LIBUSB_ERROR_NO_DEVICE => b"LIBUSB_ERROR_NO_DEVICE\0",
        LIBUSB_ERROR_NOT_FOUND => b"LIBUSB_ERROR_NOT_FOUND\0",
        LIBUSB_ERROR_BUSY => b"LIBUSB_ERROR_BUSY\0",
        LIBUSB_ERROR_TIMEOUT => b"LIBUSB_ERROR_TIMEOUT\0",
        LIBUSB_ERROR_OVERFLOW => b"LIBUSB_ERROR_OVERFLOW\0",
        LIBUSB_ERROR_PIPE => b"LIBUSB_ERROR_PIPE\0",
        LIBUSB_ERROR_INTERRUPTED => b"LIBUSB_ERROR_INTERRUPTED\0",
        LIBUSB_ERROR_NO_MEM => b"LIBUSB_ERROR_NO_MEM\0",
        LIBUSB_ERROR_NOT_SUPPORTED => b"LIBUSB_ERROR_NOT_SUPPORTED\0",
        LIBUSB_ERROR_OTHER => b"LIBUSB_ERROR_OTHER\0",
        // LIBUSB_TRANSFER_* statuses (shares value space with errors, as in libusb)
        LIBUSB_TRANSFER_ERROR => b"LIBUSB_TRANSFER_ERROR\0",
        LIBUSB_TRANSFER_TIMED_OUT => b"LIBUSB_TRANSFER_TIMED_OUT\0",
        LIBUSB_TRANSFER_CANCELLED => b"LIBUSB_TRANSFER_CANCELLED\0",
        LIBUSB_TRANSFER_STALL => b"LIBUSB_TRANSFER_STALL\0",
        LIBUSB_TRANSFER_NO_DEVICE => b"LIBUSB_TRANSFER_NO_DEVICE\0",
        LIBUSB_TRANSFER_OVERFLOW => b"LIBUSB_TRANSFER_OVERFLOW\0",
        0 => b"LIBUSB_SUCCESS / LIBUSB_TRANSFER_COMPLETED\0",
        _ => b"**UNKNOWN**\0",
    };
    s.as_ptr() as *const c_char
}

static ERRORS_EN: [&[u8]; LIBUSB_ERROR_COUNT as usize] = [
    b"Success\0",
    b"Input/Output Error\0",
    b"Invalid parameter\0",
    b"Access denied (insufficient permissions)\0",
    b"No such device (it may have been disconnected)\0",
    b"Entity not found\0",
    b"Resource busy\0",
    b"Operation timed out\0",
    b"Overflow\0",
    b"Pipe error\0",
    b"System call interrupted (perhaps due to signal)\0",
    b"Insufficient memory\0",
    b"Operation not supported or unimplemented on this platform\0",
    b"Other error\0",
];

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_strerror(errcode: c_int) -> *const c_char {
    let mut idx = errcode.wrapping_neg();
    if !(0..LIBUSB_ERROR_COUNT).contains(&idx) {
        idx = LIBUSB_ERROR_COUNT - 1;
    }
    ERRORS_EN[idx as usize].as_ptr() as *const c_char
}

/// Only English is provided; any locale is accepted (the argument is not validated).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_setlocale(_locale: *const c_char) -> c_int {
    LIBUSB_SUCCESS
}

static VERSION: libusb_version = libusb_version {
    major: 1,
    minor: 0,
    micro: 30,
    nano: 0,
    rc: c"-emuvc".as_ptr(),
    describe: c"emuvc libusb shim".as_ptr(),
};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_version() -> *const libusb_version {
    &VERSION
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_has_capability(capability: u32) -> c_int {
    match capability {
        LIBUSB_CAP_HAS_CAPABILITY => 1,
        LIBUSB_CAP_HAS_HOTPLUG => 0,
        LIBUSB_CAP_HAS_HID_ACCESS => 0,
        LIBUSB_CAP_SUPPORTS_DETACH_KERNEL_DRIVER => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::CStr;

    #[test]
    fn names_and_strings() {
        unsafe {
            assert_eq!(
                CStr::from_ptr(libusb_error_name(-7)).to_str().unwrap(),
                "LIBUSB_ERROR_TIMEOUT"
            );
            assert_eq!(
                CStr::from_ptr(libusb_error_name(0)).to_str().unwrap(),
                "LIBUSB_SUCCESS / LIBUSB_TRANSFER_COMPLETED"
            );
            assert_eq!(
                CStr::from_ptr(libusb_error_name(42)).to_str().unwrap(),
                "**UNKNOWN**"
            );
            assert_eq!(
                CStr::from_ptr(libusb_strerror(-99)).to_str().unwrap(),
                "Other error"
            );
            assert_eq!(
                CStr::from_ptr(libusb_strerror(5)).to_str().unwrap(),
                "Other error"
            );
            assert_eq!(
                CStr::from_ptr(libusb_strerror(i32::MIN)).to_str().unwrap(),
                "Other error"
            );
            assert_eq!(
                CStr::from_ptr(libusb_strerror(0)).to_str().unwrap(),
                "Success"
            );
            assert_eq!(
                CStr::from_ptr(libusb_strerror(-12)).to_str().unwrap(),
                "Operation not supported or unimplemented on this platform"
            );
            assert_eq!((*libusb_get_version()).micro, 30);
            assert_eq!(
                libusb_has_capability(LIBUSB_CAP_SUPPORTS_DETACH_KERNEL_DRIVER),
                1
            );
        }
    }
}
