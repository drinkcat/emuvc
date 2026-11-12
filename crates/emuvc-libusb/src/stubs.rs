//! libusb entry points that emuvc does not support. Each logs a warning the first time it is
//! called and returns a "not supported" style result.

use crate::ffi::*;
use core::ffi::{c_int, c_ulong, c_void};
use core::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

macro_rules! warn_once {
    ($name:literal) => {{
        static WARNED: AtomicBool = AtomicBool::new(false);
        if !WARNED.swap(true, Ordering::Relaxed) {
            log::warn!(concat!("emuvc: ", $name, " not supported"));
        }
    }};
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_alloc_streams(
    _dev_handle: *mut libusb_device_handle,
    _num_streams: u32,
    _endpoints: *mut u8,
    _num_endpoints: c_int,
) -> c_int {
    warn_once!("libusb_alloc_streams");
    LIBUSB_ERROR_NOT_SUPPORTED
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_free_streams(
    _dev_handle: *mut libusb_device_handle,
    _endpoints: *mut u8,
    _num_endpoints: c_int,
) -> c_int {
    warn_once!("libusb_free_streams");
    LIBUSB_ERROR_NOT_SUPPORTED
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_dev_mem_alloc(
    _dev_handle: *mut libusb_device_handle,
    _length: usize,
) -> *mut u8 {
    warn_once!("libusb_dev_mem_alloc");
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_dev_mem_free(
    _dev_handle: *mut libusb_device_handle,
    _buffer: *mut u8,
    _length: usize,
) -> c_int {
    warn_once!("libusb_dev_mem_free");
    LIBUSB_ERROR_NOT_SUPPORTED
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_endpoint_set_raw_io(
    _dev_handle: *mut libusb_device_handle,
    _endpoint: u8,
    _enable: c_int,
) -> c_int {
    warn_once!("libusb_endpoint_set_raw_io");
    LIBUSB_ERROR_NOT_SUPPORTED
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_endpoint_supports_raw_io(
    _dev_handle: *mut libusb_device_handle,
    _endpoint: u8,
) -> c_int {
    warn_once!("libusb_endpoint_supports_raw_io");
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_max_raw_io_transfer_size(
    _dev_handle: *mut libusb_device_handle,
    _endpoint: u8,
) -> c_int {
    warn_once!("libusb_get_max_raw_io_transfer_size");
    LIBUSB_ERROR_NOT_SUPPORTED
}

/// Returns an empty, NULL-terminated pollfd list (the emulator has no pollable fds).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_pollfds(
    _ctx: *mut libusb_context,
) -> *mut *const libusb_pollfd {
    warn_once!("libusb_get_pollfds");
    unsafe {
        libc::calloc(1, core::mem::size_of::<*const libusb_pollfd>()) as *mut *const libusb_pollfd
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_free_pollfds(pollfds: *mut *const libusb_pollfd) {
    unsafe { libc::free(pollfds as *mut c_void) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_set_pollfd_notifiers(
    _ctx: *mut libusb_context,
    _added_cb: libusb_pollfd_added_cb,
    _removed_cb: libusb_pollfd_removed_cb,
    _user_data: *mut c_void,
) {
    warn_once!("libusb_set_pollfd_notifiers");
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_pollfds_handle_timeouts(_ctx: *mut libusb_context) -> c_int {
    warn_once!("libusb_pollfds_handle_timeouts");
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_hotplug_register_callback(
    _ctx: *mut libusb_context,
    _events: c_int,
    _flags: c_int,
    _vendor_id: c_int,
    _product_id: c_int,
    _dev_class: c_int,
    _cb_fn: libusb_hotplug_callback_fn,
    _user_data: *mut c_void,
    _callback_handle: *mut libusb_hotplug_callback_handle,
) -> c_int {
    warn_once!("libusb_hotplug_register_callback");
    LIBUSB_ERROR_NOT_SUPPORTED
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_hotplug_deregister_callback(
    _ctx: *mut libusb_context,
    _callback_handle: libusb_hotplug_callback_handle,
) {
    warn_once!("libusb_hotplug_deregister_callback");
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_hotplug_get_user_data(
    _ctx: *mut libusb_context,
    _callback_handle: libusb_hotplug_callback_handle,
) -> *mut c_void {
    warn_once!("libusb_hotplug_get_user_data");
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_transfer_get_stream_id(_transfer: *mut libusb_transfer) -> u32 {
    warn_once!("libusb_transfer_get_stream_id");
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_transfer_set_stream_id(
    _transfer: *mut libusb_transfer,
    _stream_id: u32,
) {
    warn_once!("libusb_transfer_set_stream_id");
}

/// `unsigned long libusb_get_session_data(libusb_device *dev)`: the backend-specific device
/// identifier. The emulator has none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_session_data(_dev: *mut libusb_device) -> c_ulong {
    warn_once!("libusb_get_session_data");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pollfds_roundtrip() {
        unsafe {
            let p = libusb_get_pollfds(ptr::null_mut());
            assert!(!p.is_null());
            assert!((*p).is_null());
            libusb_free_pollfds(p);
        }
    }
}
