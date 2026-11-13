//! Exact port of libusb 1.0.30 `descriptor.c` parsing (configuration, BOS and
//! interface-association descriptors) plus the exported, device-independent
//! `libusb_free_*` / `libusb_get_*_descriptor` helpers.
//!
//! All results are allocated with libc `malloc`/`calloc`/`realloc` and must be released with
//! the matching `libusb_free_*` function (which uses `free`). None of the code here panics on
//! malformed input; slice accesses are bounds-checked by construction, mirroring libusb's
//! own length checks.
#![allow(clippy::missing_safety_doc)]

use crate::ffi::*;
use core::ffi::{c_int, c_void};
use core::ptr;

const DESC_HEADER_LENGTH: usize = 2;
const USB_MAXENDPOINTS: u8 = 32;
const USB_MAXINTERFACES: u8 = 32;

#[inline]
fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

#[inline]
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

#[inline]
fn is_proper(t: u8) -> bool {
    t == LIBUSB_DT_ENDPOINT
        || t == LIBUSB_DT_INTERFACE
        || t == LIBUSB_DT_CONFIG
        || t == LIBUSB_DT_DEVICE
}

/// Clamp a slice length to what libusb would see in an `int`.
#[inline]
fn clamp(raw: &[u8]) -> &[u8] {
    &raw[..raw.len().min(i32::MAX as usize)]
}

/// Mimic what libusb hands to the parser: it allocates `wTotalLength` bytes and reads the
/// descriptor into them, so anything past `wTotalLength` is never seen. Callers that hold a
/// device's raw configuration blob should pass `config_bytes(blob)` to [`parse_config`] and
/// [`parse_iads`] to get identical behaviour to libusb (including with trailing junk).
pub fn config_bytes(raw: &[u8]) -> &[u8] {
    if raw.len() < 4 {
        return raw;
    }
    let total = le16(raw, 2) as usize;
    &raw[..raw.len().min(total)]
}

unsafe fn copy_to_malloc(src: &[u8]) -> *mut u8 {
    unsafe {
        let p = libc::malloc(src.len().max(1)) as *mut u8;
        if !p.is_null() {
            ptr::copy_nonoverlapping(src.as_ptr(), p, src.len());
        }
        p
    }
}

// ------------------------------------------------------------------------------------------
// configuration descriptors
// ------------------------------------------------------------------------------------------

unsafe fn clear_endpoint(ep: *mut libusb_endpoint_descriptor) {
    unsafe { libc::free((*ep).extra as *mut c_void) }
}

unsafe fn clear_interface(iface: *mut libusb_interface) {
    unsafe {
        if !(*iface).altsetting.is_null() {
            for i in 0..(*iface).num_altsetting as usize {
                let ifp = ((*iface).altsetting as *mut libusb_interface_descriptor).add(i);
                libc::free((*ifp).extra as *mut c_void);
                (*ifp).extra = ptr::null();
                (*ifp).extra_length = 0;
                if !(*ifp).endpoint.is_null() {
                    for j in 0..(*ifp).bNumEndpoints as usize {
                        clear_endpoint(((*ifp).endpoint as *mut libusb_endpoint_descriptor).add(j));
                    }
                }
                libc::free((*ifp).endpoint as *mut c_void);
                (*ifp).endpoint = ptr::null();
                (*ifp).bNumEndpoints = 0;
            }
        }
        libc::free((*iface).altsetting as *mut c_void);
        (*iface).altsetting = ptr::null();
        (*iface).num_altsetting = 0;
    }
}

unsafe fn clear_configuration(cfg: *mut libusb_config_descriptor) {
    unsafe {
        if !(*cfg).interface.is_null() {
            for i in 0..(*cfg).bNumInterfaces as usize {
                clear_interface(((*cfg).interface as *mut libusb_interface).add(i));
            }
        }
        libc::free((*cfg).interface as *mut c_void);
        (*cfg).interface = ptr::null();
        (*cfg).bNumInterfaces = 0;
        libc::free((*cfg).extra as *mut c_void);
        (*cfg).extra = ptr::null();
        (*cfg).extra_length = 0;
    }
}

/// Returns number of bytes consumed (>= 0) or a negative libusb error.
unsafe fn parse_endpoint(ep: *mut libusb_endpoint_descriptor, buf: &[u8]) -> c_int {
    unsafe {
        let size = buf.len();
        if size < DESC_HEADER_LENGTH {
            log::error!("short endpoint descriptor read {size}/{DESC_HEADER_LENGTH}");
            return LIBUSB_ERROR_IO;
        }
        let blen = buf[0] as usize;
        if buf[1] != LIBUSB_DT_ENDPOINT {
            log::error!(
                "unexpected descriptor 0x{:x} (expected 0x{:x})",
                buf[1],
                LIBUSB_DT_ENDPOINT
            );
            return 0;
        } else if blen < LIBUSB_DT_ENDPOINT_SIZE {
            log::error!("invalid endpoint bLength ({blen})");
            return LIBUSB_ERROR_IO;
        } else if blen > size {
            log::warn!("short endpoint descriptor read {size}/{blen}");
            return 0;
        }

        (*ep).bLength = buf[0];
        (*ep).bDescriptorType = buf[1];
        (*ep).bEndpointAddress = buf[2];
        (*ep).bmAttributes = buf[3];
        (*ep).wMaxPacketSize = le16(buf, 4);
        (*ep).bInterval = buf[6];
        if blen >= LIBUSB_DT_ENDPOINT_AUDIO_SIZE {
            (*ep).bRefresh = buf[7];
            (*ep).bSynchAddress = buf[8];
        }

        let mut pos = blen;
        let begin = pos;
        while size - pos >= DESC_HEADER_LENGTH {
            let l = buf[pos] as usize;
            if l < DESC_HEADER_LENGTH {
                log::error!("invalid extra ep desc len ({l})");
                return LIBUSB_ERROR_IO;
            } else if l > size - pos {
                log::warn!("short extra ep desc read {}/{l}", size - pos);
                return pos as c_int;
            }
            if is_proper(buf[pos + 1]) {
                break;
            }
            pos += l;
        }

        let len = pos - begin;
        if len == 0 {
            return pos as c_int;
        }
        let extra = copy_to_malloc(&buf[begin..pos]);
        if extra.is_null() {
            return LIBUSB_ERROR_NO_MEM;
        }
        (*ep).extra = extra;
        (*ep).extra_length = len as c_int;
        pos as c_int
    }
}

/// Returns number of bytes consumed (>= 0) or a negative libusb error.
unsafe fn parse_interface(iface: *mut libusb_interface, buf: &[u8]) -> c_int {
    unsafe {
        let mut pos = 0usize;
        let mut interface_number: i32 = -1;

        while buf.len() - pos >= LIBUSB_DT_INTERFACE_SIZE {
            let size = buf.len() - pos;
            let b = &buf[pos..];

            let n = (*iface).num_altsetting as usize;
            let altsetting = libc::realloc(
                (*iface).altsetting as *mut c_void,
                core::mem::size_of::<libusb_interface_descriptor>() * (n + 1),
            ) as *mut libusb_interface_descriptor;
            if altsetting.is_null() {
                clear_interface(iface);
                return LIBUSB_ERROR_NO_MEM;
            }
            (*iface).altsetting = altsetting;

            let ifp = altsetting.add(n);
            // The slot is uninitialised memory: write all fields before use.
            ptr::write(
                ifp,
                libusb_interface_descriptor {
                    bLength: b[0],
                    bDescriptorType: b[1],
                    bInterfaceNumber: b[2],
                    bAlternateSetting: b[3],
                    bNumEndpoints: b[4],
                    bInterfaceClass: b[5],
                    bInterfaceSubClass: b[6],
                    bInterfaceProtocol: b[7],
                    iInterface: b[8],
                    endpoint: ptr::null(),
                    extra: ptr::null(),
                    extra_length: 0,
                },
            );
            if (*ifp).bDescriptorType != LIBUSB_DT_INTERFACE {
                log::error!(
                    "unexpected descriptor 0x{:x} (expected 0x{:x})",
                    (*ifp).bDescriptorType,
                    LIBUSB_DT_INTERFACE
                );
                return pos as c_int;
            } else if ((*ifp).bLength as usize) < LIBUSB_DT_INTERFACE_SIZE {
                log::error!("invalid interface bLength ({})", (*ifp).bLength);
                clear_interface(iface);
                return LIBUSB_ERROR_IO;
            } else if (*ifp).bLength as usize > size {
                log::warn!("short intf descriptor read {size}/{}", (*ifp).bLength);
                return pos as c_int;
            } else if (*ifp).bNumEndpoints > USB_MAXENDPOINTS {
                log::error!("too many endpoints ({})", (*ifp).bNumEndpoints);
                clear_interface(iface);
                return LIBUSB_ERROR_IO;
            }

            (*iface).num_altsetting += 1;

            if interface_number == -1 {
                interface_number = (*ifp).bInterfaceNumber as i32;
            }

            pos += (*ifp).bLength as usize;
            let begin = pos;

            // Skip over any interface, class or vendor descriptors
            while buf.len() - pos >= DESC_HEADER_LENGTH {
                let l = buf[pos] as usize;
                if l < DESC_HEADER_LENGTH {
                    log::error!("invalid extra intf desc len ({l})");
                    clear_interface(iface);
                    return LIBUSB_ERROR_IO;
                } else if l > buf.len() - pos {
                    log::warn!("short extra intf desc read {}/{l}", buf.len() - pos);
                    (*ifp).bNumEndpoints = 0;
                    return pos as c_int;
                }
                if is_proper(buf[pos + 1]) {
                    break;
                }
                pos += l;
            }

            let len = pos - begin;
            if len > 0 {
                let extra = copy_to_malloc(&buf[begin..pos]);
                if extra.is_null() {
                    clear_interface(iface);
                    return LIBUSB_ERROR_NO_MEM;
                }
                (*ifp).extra = extra;
                (*ifp).extra_length = len as c_int;
            }

            if (*ifp).bNumEndpoints > 0 {
                let endpoint = libc::calloc(
                    (*ifp).bNumEndpoints as usize,
                    core::mem::size_of::<libusb_endpoint_descriptor>(),
                ) as *mut libusb_endpoint_descriptor;
                if endpoint.is_null() {
                    clear_interface(iface);
                    return LIBUSB_ERROR_NO_MEM;
                }
                (*ifp).endpoint = endpoint;
                for i in 0..(*ifp).bNumEndpoints as usize {
                    let r = parse_endpoint(endpoint.add(i), &buf[pos..]);
                    if r < 0 {
                        clear_interface(iface);
                        return r;
                    }
                    if r == 0 {
                        (*ifp).bNumEndpoints = i as u8;
                        break;
                    }
                    pos += r as usize;
                }
            }

            // Is the next descriptor an alternate setting of this interface?
            let rest = &buf[pos..];
            if rest.len() < LIBUSB_DT_INTERFACE_SIZE
                || rest[1] != LIBUSB_DT_INTERFACE
                || rest[2] as i32 != interface_number
            {
                return pos as c_int;
            }
        }
        pos as c_int
    }
}

/// Returns number of bytes left over (>= 0) or a negative libusb error.
unsafe fn parse_configuration(cfg: *mut libusb_config_descriptor, buf: &[u8]) -> c_int {
    unsafe {
        let size = buf.len();
        if size < LIBUSB_DT_CONFIG_SIZE {
            log::error!("short config descriptor read {size}/{LIBUSB_DT_CONFIG_SIZE}");
            return LIBUSB_ERROR_IO;
        }

        (*cfg).bLength = buf[0];
        (*cfg).bDescriptorType = buf[1];
        (*cfg).wTotalLength = le16(buf, 2);
        (*cfg).bNumInterfaces = buf[4];
        (*cfg).bConfigurationValue = buf[5];
        (*cfg).iConfiguration = buf[6];
        (*cfg).bmAttributes = buf[7];
        (*cfg).MaxPower = buf[8];
        if (*cfg).bDescriptorType != LIBUSB_DT_CONFIG {
            log::error!(
                "unexpected descriptor 0x{:x} (expected 0x{:x})",
                (*cfg).bDescriptorType,
                LIBUSB_DT_CONFIG
            );
            return LIBUSB_ERROR_IO;
        } else if ((*cfg).bLength as usize) < LIBUSB_DT_CONFIG_SIZE {
            log::error!("invalid config bLength ({})", (*cfg).bLength);
            return LIBUSB_ERROR_IO;
        } else if (*cfg).bLength as usize > size {
            log::error!("short config descriptor read {size}/{}", (*cfg).bLength);
            return LIBUSB_ERROR_IO;
        } else if (*cfg).bNumInterfaces > USB_MAXINTERFACES {
            log::error!("too many interfaces ({})", (*cfg).bNumInterfaces);
            return LIBUSB_ERROR_IO;
        }

        // glibc returns a unique non-NULL pointer for calloc(0, n); keep that deterministic.
        let ifaces = libc::calloc(
            ((*cfg).bNumInterfaces as usize).max(1),
            core::mem::size_of::<libusb_interface>(),
        ) as *mut libusb_interface;
        if ifaces.is_null() {
            return LIBUSB_ERROR_NO_MEM;
        }
        (*cfg).interface = ifaces;

        let mut pos = (*cfg).bLength as usize;

        let mut i = 0u8;
        while i < (*cfg).bNumInterfaces {
            let begin = pos;
            while size - pos >= DESC_HEADER_LENGTH {
                let l = buf[pos] as usize;
                if l < DESC_HEADER_LENGTH {
                    log::error!("invalid extra config desc len ({l})");
                    clear_configuration(cfg);
                    return LIBUSB_ERROR_IO;
                } else if l > size - pos {
                    log::warn!("short extra config desc read {}/{l}", size - pos);
                    (*cfg).bNumInterfaces = i;
                    return (size - pos) as c_int;
                }
                if is_proper(buf[pos + 1]) {
                    break;
                }
                pos += l;
            }

            let len = pos - begin;
            if len > 0 {
                let old = (*cfg).extra_length as usize;
                let extra = libc::realloc((*cfg).extra as *mut c_void, old + len) as *mut u8;
                if extra.is_null() {
                    clear_configuration(cfg);
                    return LIBUSB_ERROR_NO_MEM;
                }
                ptr::copy_nonoverlapping(buf[begin..pos].as_ptr(), extra.add(old), len);
                (*cfg).extra = extra;
                (*cfg).extra_length += len as c_int;
            }

            let r = parse_interface(ifaces.add(i as usize), &buf[pos..]);
            if r < 0 {
                clear_configuration(cfg);
                return r;
            }
            if r == 0 {
                // libusb leaks any partially-built altsetting slot here; we free it.
                clear_interface(ifaces.add(i as usize));
                (*cfg).bNumInterfaces = i;
                break;
            }
            pos += r as usize;
            i += 1;
        }

        (size - pos) as c_int
    }
}

/// Parse a raw configuration descriptor blob exactly like libusb 1.0.30
/// (`raw_desc_to_config`). `raw` is treated as the bytes libusb read from the device (see
/// [`config_bytes`] to apply the `wTotalLength` cap). On success the returned pointer must be
/// freed with `libusb_free_config_descriptor`.
pub fn parse_config(raw: &[u8]) -> Result<*mut libusb_config_descriptor, c_int> {
    unsafe {
        let cfg = libc::calloc(1, core::mem::size_of::<libusb_config_descriptor>())
            as *mut libusb_config_descriptor;
        if cfg.is_null() {
            return Err(LIBUSB_ERROR_NO_MEM);
        }
        let r = parse_configuration(cfg, clamp(raw));
        if r < 0 {
            log::error!("parse_configuration failed with error {r}");
            libc::free(cfg as *mut c_void);
            return Err(r);
        } else if r > 0 {
            log::warn!("still {r} bytes of descriptor data left");
        }
        Ok(cfg)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_free_config_descriptor(config: *mut libusb_config_descriptor) {
    unsafe {
        if config.is_null() {
            return;
        }
        clear_configuration(config);
        libc::free(config as *mut c_void);
    }
}

// ------------------------------------------------------------------------------------------
// SuperSpeed endpoint companion
// ------------------------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_ss_endpoint_companion_descriptor(
    _ctx: *mut libusb_context,
    endpoint: *const libusb_endpoint_descriptor,
    ep_comp: *mut *mut libusb_ss_endpoint_companion_descriptor,
) -> c_int {
    unsafe {
        if endpoint.is_null() || ep_comp.is_null() {
            return LIBUSB_ERROR_INVALID_PARAM;
        }
        *ep_comp = ptr::null_mut();
        let extra_len = (*endpoint).extra_length;
        if (*endpoint).extra.is_null() || extra_len <= 0 {
            return LIBUSB_ERROR_NOT_FOUND;
        }
        let buf = core::slice::from_raw_parts((*endpoint).extra, extra_len as usize);
        let mut pos = 0usize;
        while buf.len() - pos >= DESC_HEADER_LENGTH {
            let l = buf[pos] as usize;
            let t = buf[pos + 1];
            let size = buf.len() - pos;
            if t != LIBUSB_DT_SS_ENDPOINT_COMPANION {
                if l < DESC_HEADER_LENGTH {
                    log::error!("invalid descriptor length {l}");
                    return LIBUSB_ERROR_IO;
                }
                pos += l;
                // libusb's `size -= bLength` wraps below zero on overrun; stop instead.
                if pos > buf.len() {
                    break;
                }
                continue;
            } else if l < LIBUSB_DT_SS_ENDPOINT_COMPANION_SIZE {
                log::error!("invalid ss-ep-comp-desc length {l}");
                return LIBUSB_ERROR_IO;
            } else if l > size {
                log::error!("short ss-ep-comp-desc read {size}/{l}");
                return LIBUSB_ERROR_IO;
            }
            let p = libc::malloc(core::mem::size_of::<libusb_ss_endpoint_companion_descriptor>())
                as *mut libusb_ss_endpoint_companion_descriptor;
            if p.is_null() {
                return LIBUSB_ERROR_NO_MEM;
            }
            let b = &buf[pos..];
            ptr::write(
                p,
                libusb_ss_endpoint_companion_descriptor {
                    bLength: b[0],
                    bDescriptorType: b[1],
                    bMaxBurst: b[2],
                    bmAttributes: b[3],
                    wBytesPerInterval: le16(b, 4),
                },
            );
            *ep_comp = p;
            return LIBUSB_SUCCESS;
        }
        LIBUSB_ERROR_NOT_FOUND
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_free_ss_endpoint_companion_descriptor(
    ep_comp: *mut libusb_ss_endpoint_companion_descriptor,
) {
    unsafe { libc::free(ep_comp as *mut c_void) }
}

// ------------------------------------------------------------------------------------------
// BOS
// ------------------------------------------------------------------------------------------

/// Parse the BOS bytes exactly like libusb's `parse_bos`. Free with `libusb_free_bos_descriptor`.
pub fn parse_bos(raw: &[u8]) -> Result<*mut libusb_bos_descriptor, c_int> {
    unsafe {
        let buf = clamp(raw);
        let mut size = buf.len();
        if size < LIBUSB_DT_BOS_SIZE {
            log::error!("short bos descriptor read {size}/{LIBUSB_DT_BOS_SIZE}");
            return Err(LIBUSB_ERROR_IO);
        }
        let blen = buf[0] as usize;
        if buf[1] != LIBUSB_DT_BOS {
            log::error!(
                "unexpected descriptor 0x{:x} (expected 0x{:x})",
                buf[1],
                LIBUSB_DT_BOS
            );
            return Err(LIBUSB_ERROR_IO);
        } else if blen < LIBUSB_DT_BOS_SIZE {
            log::error!("invalid bos bLength ({blen})");
            return Err(LIBUSB_ERROR_IO);
        } else if blen > size {
            log::error!("short bos descriptor read {size}/{blen}");
            return Err(LIBUSB_ERROR_IO);
        }

        let ncaps = buf[4] as usize;
        let bos = libc::calloc(
            1,
            core::mem::size_of::<libusb_bos_descriptor>()
                + ncaps * core::mem::size_of::<*mut c_void>(),
        ) as *mut libusb_bos_descriptor;
        if bos.is_null() {
            return Err(LIBUSB_ERROR_NO_MEM);
        }
        (*bos).bLength = buf[0];
        (*bos).bDescriptorType = buf[1];
        (*bos).wTotalLength = le16(buf, 2);
        (*bos).bNumDeviceCaps = buf[4];
        let caps = (*bos).dev_capability.as_mut_ptr();

        let mut pos = blen;
        size -= blen;

        let mut i = 0usize;
        while i < ncaps {
            if size < LIBUSB_DT_DEVICE_CAPABILITY_SIZE {
                log::warn!(
                    "short dev-cap descriptor read {size}/{LIBUSB_DT_DEVICE_CAPABILITY_SIZE}"
                );
                break;
            }
            let l = buf[pos] as usize;
            let t = buf[pos + 1];
            if t != LIBUSB_DT_DEVICE_CAPABILITY {
                log::warn!(
                    "unexpected descriptor 0x{:x} (expected 0x{:x})",
                    t,
                    LIBUSB_DT_DEVICE_CAPABILITY
                );
                break;
            } else if l < LIBUSB_DT_DEVICE_CAPABILITY_SIZE {
                log::error!("invalid dev-cap bLength ({l})");
                libusb_free_bos_descriptor(bos);
                return Err(LIBUSB_ERROR_IO);
            } else if l > size {
                log::warn!("short dev-cap descriptor read {size}/{l}");
                break;
            }
            let p = copy_to_malloc(&buf[pos..pos + l]);
            if p.is_null() {
                libusb_free_bos_descriptor(bos);
                return Err(LIBUSB_ERROR_NO_MEM);
            }
            *caps.add(i) = p as *mut libusb_bos_dev_capability_descriptor;
            pos += l;
            size -= l;
            i += 1;
        }
        (*bos).bNumDeviceCaps = i as u8;
        Ok(bos)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_free_bos_descriptor(bos: *mut libusb_bos_descriptor) {
    unsafe {
        if bos.is_null() {
            return;
        }
        let caps = (*bos).dev_capability.as_mut_ptr();
        for i in 0..(*bos).bNumDeviceCaps as usize {
            libc::free(*caps.add(i) as *mut c_void);
        }
        libc::free(bos as *mut c_void);
    }
}

// ------------------------------------------------------------------------------------------
// BOS device capabilities
// ------------------------------------------------------------------------------------------

#[inline]
unsafe fn cap_data<'a>(dev_cap: *const libusb_bos_dev_capability_descriptor) -> &'a [u8] {
    // The whole descriptor (header + data), bLength bytes, as stored by parse_bos.
    unsafe { core::slice::from_raw_parts(dev_cap as *const u8, (*dev_cap).bLength as usize) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_usb_2_0_extension_descriptor(
    _ctx: *mut libusb_context,
    dev_cap: *mut libusb_bos_dev_capability_descriptor,
    out: *mut *mut libusb_usb_2_0_extension_descriptor,
) -> c_int {
    unsafe {
        if dev_cap.is_null() || out.is_null() {
            return LIBUSB_ERROR_INVALID_PARAM;
        }
        if (*dev_cap).bDevCapabilityType != LIBUSB_BT_USB_2_0_EXTENSION {
            log::error!(
                "unexpected bDevCapabilityType 0x{:x}",
                (*dev_cap).bDevCapabilityType
            );
            return LIBUSB_ERROR_INVALID_PARAM;
        } else if ((*dev_cap).bLength as usize) < LIBUSB_BT_USB_2_0_EXTENSION_SIZE {
            log::error!(
                "short dev-cap descriptor read {}/{}",
                (*dev_cap).bLength,
                LIBUSB_BT_USB_2_0_EXTENSION_SIZE
            );
            return LIBUSB_ERROR_IO;
        }
        let d = cap_data(dev_cap);
        let p = libc::malloc(core::mem::size_of::<libusb_usb_2_0_extension_descriptor>())
            as *mut libusb_usb_2_0_extension_descriptor;
        if p.is_null() {
            return LIBUSB_ERROR_NO_MEM;
        }
        ptr::write(
            p,
            libusb_usb_2_0_extension_descriptor {
                bLength: d[0],
                bDescriptorType: d[1],
                bDevCapabilityType: d[2],
                bmAttributes: le32(d, 3),
            },
        );
        *out = p;
        LIBUSB_SUCCESS
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_free_usb_2_0_extension_descriptor(
    p: *mut libusb_usb_2_0_extension_descriptor,
) {
    unsafe { libc::free(p as *mut c_void) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_ss_usb_device_capability_descriptor(
    _ctx: *mut libusb_context,
    dev_cap: *mut libusb_bos_dev_capability_descriptor,
    out: *mut *mut libusb_ss_usb_device_capability_descriptor,
) -> c_int {
    unsafe {
        if dev_cap.is_null() || out.is_null() {
            return LIBUSB_ERROR_INVALID_PARAM;
        }
        if (*dev_cap).bDevCapabilityType != LIBUSB_BT_SS_USB_DEVICE_CAPABILITY {
            log::error!(
                "unexpected bDevCapabilityType 0x{:x}",
                (*dev_cap).bDevCapabilityType
            );
            return LIBUSB_ERROR_INVALID_PARAM;
        } else if ((*dev_cap).bLength as usize) < LIBUSB_BT_SS_USB_DEVICE_CAPABILITY_SIZE {
            log::error!(
                "short dev-cap descriptor read {}/{}",
                (*dev_cap).bLength,
                LIBUSB_BT_SS_USB_DEVICE_CAPABILITY_SIZE
            );
            return LIBUSB_ERROR_IO;
        }
        let d = cap_data(dev_cap);
        let p = libc::malloc(core::mem::size_of::<
            libusb_ss_usb_device_capability_descriptor,
        >()) as *mut libusb_ss_usb_device_capability_descriptor;
        if p.is_null() {
            return LIBUSB_ERROR_NO_MEM;
        }
        ptr::write(
            p,
            libusb_ss_usb_device_capability_descriptor {
                bLength: d[0],
                bDescriptorType: d[1],
                bDevCapabilityType: d[2],
                bmAttributes: d[3],
                wSpeedSupported: le16(d, 4),
                bFunctionalitySupport: d[6],
                bU1DevExitLat: d[7],
                bU2DevExitLat: le16(d, 8),
            },
        );
        *out = p;
        LIBUSB_SUCCESS
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_free_ss_usb_device_capability_descriptor(
    p: *mut libusb_ss_usb_device_capability_descriptor,
) {
    unsafe { libc::free(p as *mut c_void) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_ssplus_usb_device_capability_descriptor(
    _ctx: *mut libusb_context,
    dev_cap: *mut libusb_bos_dev_capability_descriptor,
    out: *mut *mut libusb_ssplus_usb_device_capability_descriptor,
) -> c_int {
    unsafe {
        if dev_cap.is_null() || out.is_null() {
            return LIBUSB_ERROR_INVALID_PARAM;
        }
        if (*dev_cap).bDevCapabilityType != LIBUSB_BT_SUPERSPEED_PLUS_CAPABILITY {
            log::error!(
                "unexpected bDevCapabilityType 0x{:x}",
                (*dev_cap).bDevCapabilityType
            );
            return LIBUSB_ERROR_INVALID_PARAM;
        } else if ((*dev_cap).bLength as usize) < LIBUSB_BT_SSPLUS_USB_DEVICE_CAPABILITY_SIZE {
            log::error!(
                "short dev-cap descriptor read {}/{}",
                (*dev_cap).bLength,
                LIBUSB_BT_SSPLUS_USB_DEVICE_CAPABILITY_SIZE
            );
            return LIBUSB_ERROR_IO;
        }
        let d = cap_data(dev_cap);
        // data = d[3..]: [0]=reserved, [1..5]=bmAttributes, [5..7]=wFunctionalitySupport
        let bm_attributes = le32(d, 4);
        let w_func = le16(d, 8);

        let n_attrs = ((bm_attributes & 0xF) + 1) as u8;
        let hdr = core::mem::size_of::<libusb_ssplus_usb_device_capability_descriptor>();
        let attr_sz = core::mem::size_of::<libusb_ssplus_sublink_attribute>();
        let cap = libc::malloc(hdr + n_attrs as usize * attr_sz)
            as *mut libusb_ssplus_usb_device_capability_descriptor;
        if cap.is_null() {
            return LIBUSB_ERROR_NO_MEM;
        }
        (*cap).numSublinkSpeedAttributes = n_attrs;
        (*cap).numSublinkSpeedIDs = (((bm_attributes & 0xF0) >> 4) + 1) as u8;
        (*cap).ssid = (w_func & 0xF) as u8;
        (*cap).minRxLaneCount = ((w_func & 0x0F00) >> 8) as u8;
        (*cap).minTxLaneCount = ((w_func & 0xF000) >> 12) as u8;

        if ((*dev_cap).bLength as usize)
            < LIBUSB_BT_SSPLUS_USB_DEVICE_CAPABILITY_SIZE + n_attrs as usize * 4
        {
            libc::free(cap as *mut c_void);
            log::error!(
                "short ssplus capability descriptor, unable to read sublinks: Not enough data"
            );
            return LIBUSB_ERROR_IO;
        }

        let attrs = (*cap).sublinkSpeedAttributes.as_mut_ptr();
        for i in 0..n_attrs as usize {
            let attr = le32(d, LIBUSB_BT_SSPLUS_USB_DEVICE_CAPABILITY_SIZE + i * 4);
            ptr::write(
                attrs.add(i),
                libusb_ssplus_sublink_attribute {
                    ssid: (attr & 0x0f) as u8,
                    mantissa: (attr >> 16) as u16,
                    exponent: ((attr >> 4) & 0x3) as c_int,
                    type_: if attr & 0x40 != 0 {
                        LIBUSB_SSPLUS_ATTR_TYPE_ASYM
                    } else {
                        LIBUSB_SSPLUS_ATTR_TYPE_SYM
                    },
                    direction: if attr & 0x80 != 0 {
                        LIBUSB_SSPLUS_ATTR_DIR_TX
                    } else {
                        LIBUSB_SSPLUS_ATTR_DIR_RX
                    },
                    protocol: if attr & 0x4000 != 0 {
                        LIBUSB_SSPLUS_ATTR_PROT_SSPLUS
                    } else {
                        LIBUSB_SSPLUS_ATTR_PROT_SS
                    },
                },
            );
        }
        *out = cap;
        LIBUSB_SUCCESS
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_free_ssplus_usb_device_capability_descriptor(
    p: *mut libusb_ssplus_usb_device_capability_descriptor,
) {
    unsafe { libc::free(p as *mut c_void) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_container_id_descriptor(
    _ctx: *mut libusb_context,
    dev_cap: *mut libusb_bos_dev_capability_descriptor,
    out: *mut *mut libusb_container_id_descriptor,
) -> c_int {
    unsafe {
        if dev_cap.is_null() || out.is_null() {
            return LIBUSB_ERROR_INVALID_PARAM;
        }
        if (*dev_cap).bDevCapabilityType != LIBUSB_BT_CONTAINER_ID {
            log::error!(
                "unexpected bDevCapabilityType 0x{:x}",
                (*dev_cap).bDevCapabilityType
            );
            return LIBUSB_ERROR_INVALID_PARAM;
        } else if ((*dev_cap).bLength as usize) < LIBUSB_BT_CONTAINER_ID_SIZE {
            log::error!(
                "short dev-cap descriptor read {}/{}",
                (*dev_cap).bLength,
                LIBUSB_BT_CONTAINER_ID_SIZE
            );
            return LIBUSB_ERROR_IO;
        }
        let d = cap_data(dev_cap);
        let p = libc::malloc(core::mem::size_of::<libusb_container_id_descriptor>())
            as *mut libusb_container_id_descriptor;
        if p.is_null() {
            return LIBUSB_ERROR_NO_MEM;
        }
        let mut id = [0u8; 16];
        id.copy_from_slice(&d[4..20]);
        ptr::write(
            p,
            libusb_container_id_descriptor {
                bLength: d[0],
                bDescriptorType: d[1],
                bDevCapabilityType: d[2],
                bReserved: d[3],
                ContainerID: id,
            },
        );
        *out = p;
        LIBUSB_SUCCESS
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_free_container_id_descriptor(
    p: *mut libusb_container_id_descriptor,
) {
    unsafe { libc::free(p as *mut c_void) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_platform_descriptor(
    _ctx: *mut libusb_context,
    dev_cap: *mut libusb_bos_dev_capability_descriptor,
    out: *mut *mut libusb_platform_descriptor,
) -> c_int {
    unsafe {
        if dev_cap.is_null() || out.is_null() {
            return LIBUSB_ERROR_INVALID_PARAM;
        }
        if (*dev_cap).bDevCapabilityType != LIBUSB_BT_PLATFORM_DESCRIPTOR {
            log::error!(
                "unexpected bDevCapabilityType 0x{:x}",
                (*dev_cap).bDevCapabilityType
            );
            return LIBUSB_ERROR_INVALID_PARAM;
        } else if ((*dev_cap).bLength as usize) < LIBUSB_BT_PLATFORM_DESCRIPTOR_MIN_SIZE {
            log::error!(
                "short dev-cap descriptor read {}/{}",
                (*dev_cap).bLength,
                LIBUSB_BT_PLATFORM_DESCRIPTOR_MIN_SIZE
            );
            return LIBUSB_ERROR_IO;
        }
        let d = cap_data(dev_cap);
        let p = libc::malloc(d.len()) as *mut libusb_platform_descriptor;
        if p.is_null() {
            return LIBUSB_ERROR_NO_MEM;
        }
        // malloc'd bLength bytes: 4-byte header, 16-byte UUID, then CapabilityData[].
        ptr::copy_nonoverlapping(d.as_ptr(), p as *mut u8, d.len());
        *out = p;
        LIBUSB_SUCCESS
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_free_platform_descriptor(p: *mut libusb_platform_descriptor) {
    unsafe { libc::free(p as *mut c_void) }
}

// ------------------------------------------------------------------------------------------
// Interface association descriptors
// ------------------------------------------------------------------------------------------

/// Parse the IADs of a raw configuration blob exactly like libusb's `parse_iad_array`.
/// Free with `libusb_free_interface_association_descriptors`.
pub fn parse_iads(
    raw_config: &[u8],
) -> Result<*mut libusb_interface_association_descriptor_array, c_int> {
    unsafe {
        let buf = clamp(raw_config);
        let size = buf.len();
        if size < LIBUSB_DT_CONFIG_SIZE {
            log::error!("short config descriptor read {size}/{LIBUSB_DT_CONFIG_SIZE}");
            return Err(LIBUSB_ERROR_IO);
        }

        // First pass: count IADs.
        let mut count = 0usize;
        let mut consumed = 0usize;
        while size - consumed >= DESC_HEADER_LENGTH {
            let l = buf[consumed] as usize;
            if l < DESC_HEADER_LENGTH {
                log::error!("invalid descriptor bLength {l}");
                return Err(LIBUSB_ERROR_IO);
            } else if l > size - consumed {
                log::warn!("short config descriptor read {}/{l}", size - consumed);
                return Err(LIBUSB_ERROR_IO);
            }
            if buf[consumed + 1] == LIBUSB_DT_INTERFACE_ASSOCIATION {
                count += 1;
            }
            consumed += l;
        }

        let arr = libc::calloc(
            1,
            core::mem::size_of::<libusb_interface_association_descriptor_array>(),
        ) as *mut libusb_interface_association_descriptor_array;
        if arr.is_null() {
            return Err(LIBUSB_ERROR_NO_MEM);
        }
        (*arr).iad = ptr::null();
        (*arr).length = count as c_int;

        if count > 0 {
            let iad = libc::calloc(
                count,
                core::mem::size_of::<libusb_interface_association_descriptor>(),
            ) as *mut libusb_interface_association_descriptor;
            if iad.is_null() {
                libc::free(arr as *mut c_void);
                return Err(LIBUSB_ERROR_NO_MEM);
            }
            (*arr).iad = iad;

            // Second pass: fill. Entries whose bLength < 8 stay zeroed, as in libusb.
            let mut remaining = size as i64;
            let mut pos = 0usize;
            let mut i = 0usize;
            loop {
                let l = buf[pos] as i64;
                let t = buf[pos + 1];
                if t == LIBUSB_DT_INTERFACE_ASSOCIATION
                    && remaining >= LIBUSB_DT_INTERFACE_ASSOCIATION_SIZE as i64
                    && i < count
                {
                    *iad.add(i) = libusb_interface_association_descriptor {
                        bLength: buf[pos],
                        bDescriptorType: buf[pos + 1],
                        bFirstInterface: buf[pos + 2],
                        bInterfaceCount: buf[pos + 3],
                        bFunctionClass: buf[pos + 4],
                        bFunctionSubClass: buf[pos + 5],
                        bFunctionProtocol: buf[pos + 6],
                        iFunction: buf[pos + 7],
                    };
                    i += 1;
                }
                remaining -= l;
                if remaining < DESC_HEADER_LENGTH as i64 {
                    break;
                }
                pos += l as usize;
            }
        }
        Ok(arr)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_free_interface_association_descriptors(
    iad_array: *mut libusb_interface_association_descriptor_array,
) {
    unsafe {
        if iad_array.is_null() {
            return;
        }
        if !(*iad_array).iad.is_null() {
            libc::free((*iad_array).iad as *mut c_void);
        }
        libc::free(iad_array as *mut c_void);
    }
}

// ------------------------------------------------------------------------------------------
// tests
// ------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        let p = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/30c9_0052.descriptors.bin"
        );
        std::fs::read(p).expect("fixture")
    }

    fn config_blob() -> Vec<u8> {
        let all = fixture();
        all[18..].to_vec()
    }

    unsafe fn extra<'a>(p: *const u8, n: c_int) -> &'a [u8] {
        if p.is_null() {
            &[]
        } else {
            unsafe { core::slice::from_raw_parts(p, n as usize) }
        }
    }

    unsafe fn iface<'a>(c: *mut libusb_config_descriptor, i: usize) -> &'a libusb_interface {
        unsafe { &*(*c).interface.add(i) }
    }

    unsafe fn alt<'a>(
        c: *mut libusb_config_descriptor,
        i: usize,
        a: usize,
    ) -> &'a libusb_interface_descriptor {
        unsafe { &*iface(c, i).altsetting.add(a) }
    }

    #[test]
    fn real_webcam_config() {
        let blob = config_blob();
        let cfg = parse_config(config_bytes(&blob)).unwrap();
        unsafe {
            assert_eq!((*cfg).bNumInterfaces, 5);
            assert_eq!((*cfg).wTotalLength as usize, config_bytes(&blob).len());
            assert_eq!(iface(cfg, 1).num_altsetting, 12);
            assert_eq!(iface(cfg, 3).num_altsetting, 2);
            for i in 0..5 {
                for a in 0..iface(cfg, i).num_altsetting as usize {
                    assert_eq!(alt(cfg, i, a).bInterfaceNumber as usize, i);
                    assert_eq!(alt(cfg, i, a).bAlternateSetting as usize, a);
                }
            }
            let a00 = alt(cfg, 0, 0);
            let e = extra(a00.extra, a00.extra_length);
            assert_eq!(&e[..3], &[0x0d, 0x24, 0x01]);
            let a10 = alt(cfg, 1, 0);
            let e = extra(a10.extra, a10.extra_length);
            assert_eq!(&e[..3], &[0x0f, 0x24, 0x01]);

            let mut found = false;
            for i in 0..5 {
                for a in 0..iface(cfg, i).num_altsetting as usize {
                    let ifp = alt(cfg, i, a);
                    for k in 0..ifp.bNumEndpoints as usize {
                        let ep = &*ifp.endpoint.add(k);
                        if ep.bEndpointAddress == 0x87 {
                            let e = extra(ep.extra, ep.extra_length);
                            assert_eq!(e.len(), 5, "{e:x?}");
                            assert_eq!(e[1], 0x25);
                            found = true;
                        }
                    }
                }
            }
            assert!(found);
            libusb_free_config_descriptor(cfg);
        }
    }

    #[test]
    fn real_webcam_iads() {
        let blob = config_blob();
        let arr = parse_iads(config_bytes(&blob)).unwrap();
        unsafe {
            assert_eq!((*arr).length, 2);
            assert_eq!((*(*arr).iad).bFirstInterface, 0);
            assert_eq!((*(*arr).iad.add(1)).bFirstInterface, 2);
            libusb_free_interface_association_descriptors(arr);
        }
    }

    #[test]
    fn trailing_junk_is_trimmed_by_config_bytes() {
        let mut blob = config_blob();
        let n = blob.len();
        blob.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef, 0x01]);
        assert_eq!(config_bytes(&blob).len(), n);
        let cfg = parse_config(&blob).unwrap(); // raw junk: tolerated (libusb warns only)
        unsafe { libusb_free_config_descriptor(cfg) };
    }

    #[test]
    fn corrupt_and_truncated_do_not_crash() {
        let blob = config_blob();
        for n in 0..=blob.len() {
            if let Ok(c) = parse_config(&blob[..n]) {
                unsafe { libusb_free_config_descriptor(c) };
            }
            if let Ok(a) = parse_iads(&blob[..n]) {
                unsafe { libusb_free_interface_association_descriptors(a) };
            }
        }
        let mut seed = 0x1234_5678_u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as usize
        };
        for _ in 0..20000 {
            let mut b = blob.clone();
            for _ in 0..(1 + rnd() % 6) {
                let i = rnd() % b.len();
                b[i] = rnd() as u8;
            }
            let cut = if rnd() % 2 == 0 {
                b.len()
            } else {
                rnd() % (b.len() + 1)
            };
            let b = &b[..cut];
            if let Ok(c) = parse_config(b) {
                unsafe {
                    // walk everything to catch bad pointers
                    for i in 0..(*c).bNumInterfaces as usize {
                        for a in 0..iface(c, i).num_altsetting as usize {
                            let ifp = alt(c, i, a);
                            let _ = extra(ifp.extra, ifp.extra_length);
                            for k in 0..ifp.bNumEndpoints as usize {
                                let ep = &*ifp.endpoint.add(k);
                                let _ = extra(ep.extra, ep.extra_length);
                                let mut comp = ptr::null_mut();
                                let r = libusb_get_ss_endpoint_companion_descriptor(
                                    ptr::null_mut(),
                                    ep,
                                    &mut comp,
                                );
                                if r == 0 {
                                    libusb_free_ss_endpoint_companion_descriptor(comp);
                                }
                            }
                        }
                    }
                    libusb_free_config_descriptor(c);
                }
            }
            if let Ok(a) = parse_iads(b) {
                unsafe { libusb_free_interface_association_descriptors(a) };
            }
        }
    }

    #[test]
    fn config_error_cases() {
        assert_eq!(parse_config(&[]).err(), Some(LIBUSB_ERROR_IO));
        assert_eq!(
            parse_config(&[9, 2, 9, 0, 0, 1, 0, 0x80]).err(),
            Some(LIBUSB_ERROR_IO)
        );
        // wrong type
        assert_eq!(
            parse_config(&[9, 1, 9, 0, 0, 1, 0, 0x80, 50]).err(),
            Some(LIBUSB_ERROR_IO)
        );
        // too many interfaces
        assert_eq!(
            parse_config(&[9, 2, 9, 0, 33, 1, 0, 0x80, 50]).err(),
            Some(LIBUSB_ERROR_IO)
        );
        // bLength < 2 for extra descriptor is fatal
        let r = parse_config(&[9, 2, 11, 0, 1, 1, 0, 0x80, 50, 1, 0x24]);
        assert_eq!(r.err(), Some(LIBUSB_ERROR_IO));
        // config with 1 interface declared but nothing follows: bNumInterfaces becomes 0
        let c = parse_config(&[9, 2, 9, 0, 1, 1, 0, 0x80, 50]).unwrap();
        unsafe {
            assert_eq!((*c).bNumInterfaces, 0);
            libusb_free_config_descriptor(c);
        }
    }

    #[test]
    fn extra_before_first_interface_and_ss_companion() {
        let raw: Vec<u8> = [
            &[9u8, 2, 0, 0, 1, 1, 0, 0x80, 50][..],
            &[4, 0x24, 1, 2][..], // class specific before interface -> config.extra
            &[9, 4, 0, 0, 1, 0xff, 0, 0, 0][..],
            &[3, 0x24, 9][..], // interface extra
            &[7, 5, 0x81, 2, 0, 4, 0][..],
            &[6, 0x30, 3, 0, 0, 8][..], // ss companion -> endpoint extra
        ]
        .concat();
        let cfg = parse_config(&raw).unwrap();
        unsafe {
            assert_eq!(extra((*cfg).extra, (*cfg).extra_length), &[4, 0x24, 1, 2]);
            let a = alt(cfg, 0, 0);
            assert_eq!(extra(a.extra, a.extra_length), &[3, 0x24, 9]);
            let ep = &*a.endpoint;
            assert_eq!(ep.wMaxPacketSize, 0x0400);
            assert_eq!(extra(ep.extra, ep.extra_length), &[6, 0x30, 3, 0, 0, 8]);
            let mut comp = ptr::null_mut();
            assert_eq!(
                libusb_get_ss_endpoint_companion_descriptor(ptr::null_mut(), ep, &mut comp),
                0
            );
            assert_eq!((*comp).bMaxBurst, 3);
            assert_eq!((*comp).wBytesPerInterval, 0x0800);
            libusb_free_ss_endpoint_companion_descriptor(comp);
            libusb_free_config_descriptor(cfg);
        }
    }

    #[test]
    fn bos_parsing() {
        let raw: Vec<u8> = [
            &[5u8, 0x0f, 0, 0, 5][..],
            &[7, 0x10, 2, 0x02, 0, 0, 0][..], // USB 2.0 ext, bmAttributes = 2
            &[10, 0x10, 3, 0, 0x0e, 0, 1, 10, 0xff, 0x07][..],
            &[
                20, 0x10, 4, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
            ][..],
            &[12, 0x10, 0x0a, 0, 0x01, 0, 0, 0, 0x21, 0x11, 0, 0][..], // ssplus, 2 attrs: needs 20 bytes
            &[1][..],
        ]
        .concat();
        let bos = parse_bos(&raw).unwrap();
        unsafe {
            // 4th cap (ssplus) is stored (bLength fits), the trailing stray byte is ignored.
            assert_eq!((*bos).bNumDeviceCaps, 4);
            let caps = (*bos).dev_capability.as_ptr();
            let mut u2 = ptr::null_mut();
            assert_eq!(
                libusb_get_usb_2_0_extension_descriptor(ptr::null_mut(), *caps, &mut u2),
                0
            );
            assert_eq!((*u2).bmAttributes, 2);
            libusb_free_usb_2_0_extension_descriptor(u2);
            let mut ss = ptr::null_mut();
            assert_eq!(
                libusb_get_ss_usb_device_capability_descriptor(
                    ptr::null_mut(),
                    *caps.add(1),
                    &mut ss
                ),
                0
            );
            assert_eq!((*ss).wSpeedSupported, 0x000e);
            assert_eq!((*ss).bU2DevExitLat, 0x07ff);
            libusb_free_ss_usb_device_capability_descriptor(ss);
            let mut cid = ptr::null_mut();
            assert_eq!(
                libusb_get_container_id_descriptor(ptr::null_mut(), *caps.add(2), &mut cid),
                0
            );
            assert_eq!((*cid).ContainerID[0], 1);
            assert_eq!((*cid).ContainerID[15], 16);
            libusb_free_container_id_descriptor(cid);
            // wrong type
            let mut pl = ptr::null_mut();
            assert_eq!(
                libusb_get_platform_descriptor(ptr::null_mut(), *caps, &mut pl),
                LIBUSB_ERROR_INVALID_PARAM
            );
            // ssplus with 2 sublink attrs needs bLength >= 12+8
            let mut sp = ptr::null_mut();
            assert_eq!(
                libusb_get_ssplus_usb_device_capability_descriptor(
                    ptr::null_mut(),
                    *caps.add(3),
                    &mut sp
                ),
                LIBUSB_ERROR_IO
            );
            libusb_free_bos_descriptor(bos);
        }
        // truncated / garbage BOS never crashes
        for n in 0..=raw.len() {
            if let Ok(b) = parse_bos(&raw[..n]) {
                unsafe { libusb_free_bos_descriptor(b) };
            }
        }
        assert_eq!(parse_bos(&[5, 2, 0, 0, 0]).err(), Some(LIBUSB_ERROR_IO));
    }

    #[test]
    fn ssplus_and_platform() {
        let mut cap = vec![20u8, 0x10, 0x0a, 0, 0x01, 0, 0, 0, 0x21, 0x11, 0, 0];
        cap.extend_from_slice(&0x0001_00c1u32.to_le_bytes()); // ssid 1, exp 0, asym.. etc
        cap.extend_from_slice(&0x4000_0000u32.to_le_bytes());
        let mut raw = vec![5u8, 0x0f, 25, 0, 1];
        raw.extend_from_slice(&cap);
        let bos = parse_bos(&raw).unwrap();
        unsafe {
            let c = *(*bos).dev_capability.as_ptr();
            let mut sp = ptr::null_mut();
            assert_eq!(
                libusb_get_ssplus_usb_device_capability_descriptor(ptr::null_mut(), c, &mut sp),
                0
            );
            assert_eq!((*sp).numSublinkSpeedAttributes, 2);
            assert_eq!((*sp).ssid, 1);
            assert_eq!((*sp).minRxLaneCount, 1);
            let a = &*(*sp).sublinkSpeedAttributes.as_ptr();
            assert_eq!(a.ssid, 1);
            assert_eq!(a.mantissa, 1);
            assert_eq!(a.direction, LIBUSB_SSPLUS_ATTR_DIR_TX);
            assert_eq!(a.type_, LIBUSB_SSPLUS_ATTR_TYPE_ASYM);
            libusb_free_ssplus_usb_device_capability_descriptor(sp);
            libusb_free_bos_descriptor(bos);
        }
        let mut cap = vec![22u8, 0x10, 5, 0];
        cap.extend_from_slice(&[7u8; 16]);
        cap.extend_from_slice(&[9, 8]);
        let mut raw = vec![5u8, 0x0f, 27, 0, 1];
        raw.extend_from_slice(&cap);
        let bos = parse_bos(&raw).unwrap();
        unsafe {
            let c = *(*bos).dev_capability.as_ptr();
            let mut p = ptr::null_mut();
            assert_eq!(
                libusb_get_platform_descriptor(ptr::null_mut(), c, &mut p),
                0
            );
            assert_eq!((*p).PlatformCapabilityUUID, [7u8; 16]);
            assert_eq!(*(*p).CapabilityData.as_ptr(), 9);
            assert_eq!(*(*p).CapabilityData.as_ptr().add(1), 8);
            libusb_free_platform_descriptor(p);
            libusb_free_bos_descriptor(bos);
        }
    }

    #[test]
    fn iad_errors() {
        assert_eq!(parse_iads(&[9, 2]).err(), Some(LIBUSB_ERROR_IO));
        // bLength = 1 -> invalid
        assert_eq!(
            parse_iads(&[9, 2, 9, 0, 0, 1, 0, 0x80, 50, 1, 0x0b]).err(),
            Some(LIBUSB_ERROR_IO)
        );
        // IAD with bLength 4 is counted but left zeroed
        let a = parse_iads(&[9, 2, 13, 0, 0, 1, 0, 0x80, 50, 4, 0x0b, 1, 2]).unwrap();
        unsafe {
            assert_eq!((*a).length, 1);
            assert_eq!((*(*a).iad).bLength, 0);
            libusb_free_interface_association_descriptors(a);
        }
    }
}
