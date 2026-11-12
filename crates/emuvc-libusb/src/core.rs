//! libusb objects (context, device, device handle) and the non-transfer API.
//!
//! Object model:
//! - one process-wide [`Bus`] (emulated devices on an `emuvc_usb::Host`),
//!   shared by every context, like real hardware;
//! - a [`Context`] per `libusb_init`, owning event handling state and its
//!   `libusb_device` objects;
//! - [`Device`] is the refcounted `libusb_device`, [`Handle`] the
//!   `libusb_device_handle`.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_uchar, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use emuvc_usb::desc::{ConfigHeader, ConfigView};
use emuvc_usb::{DeviceId, DeviceInfo, Host, SetupPacket, Speed, UsbError};
use log::{debug, error, warn};

use crate::descriptors::{config_bytes, parse_bos, parse_config, parse_iads};
use crate::ffi::*;
use crate::transfer::{self, Events};

/// Process-wide emulated bus.
pub struct Bus {
    pub host: Host,
    /// Emulate a kernel driver (uvcvideo) bound to video class interfaces,
    /// so apps exercise their detach/attach logic as on Linux.
    pub emulate_kernel_driver: bool,
    drivers: Mutex<HashMap<DeviceId, KernelDrivers>>,
    claims: Mutex<HashMap<(DeviceId, u8), usize>>,
}

#[derive(Clone, Copy, Default)]
struct KernelDrivers {
    bound: u64,
    detached: u64,
}

impl Bus {
    pub fn new(host: Host, emulate_kernel_driver: bool) -> Self {
        Bus {
            host,
            emulate_kernel_driver,
            drivers: Mutex::new(HashMap::new()),
            claims: Mutex::new(HashMap::new()),
        }
    }

    fn drivers(&self, info: &DeviceInfo) -> KernelDrivers {
        let mut map = self.drivers.lock().unwrap();
        *map.entry(info.id).or_insert_with(|| {
            let mut bound = 0;
            if self.emulate_kernel_driver {
                for i in active_view(info).iter().flat_map(|v| v.interfaces.iter()) {
                    if i.alts.first().is_some_and(|a| a.desc.class == 0x0e) && i.number < 64 {
                        bound |= 1 << i.number;
                    }
                }
            }
            KernelDrivers { bound, detached: 0 }
        })
    }
}

static BUS: OnceLock<Result<Bus, String>> = OnceLock::new();

pub fn bus() -> Result<&'static Bus, c_int> {
    match BUS.get_or_init(crate::setup::create_bus) {
        Ok(b) => Ok(b),
        Err(e) => {
            error!("emuvc: cannot create the emulated bus: {e}");
            Err(LIBUSB_ERROR_OTHER)
        }
    }
}

pub struct Context {
    pub bus: &'static Bus,
    pub events: Events,
    devices: Mutex<HashMap<DeviceId, usize>>,
    debug: AtomicI32,
}

pub struct Device {
    refs: AtomicUsize,
    pub ctx: *const Context,
    pub info: Arc<DeviceInfo>,
}

pub struct Handle {
    pub dev: *mut Device,
    pub ctx: *const Context,
    claimed: Mutex<u64>,
    auto_detach: AtomicBool,
    /// Submitted transfers (pointer -> URB id, 0 while submitting).
    pub inflight: Mutex<HashMap<usize, u64>>,
}

static DEFAULT_CTX: Mutex<(usize, usize)> = Mutex::new((0, 0));

/// Resolves a possibly-NULL context argument.
pub unsafe fn ctx_ref<'a>(ctx: *mut libusb_context) -> Option<&'a Context> {
    let p = if ctx.is_null() {
        DEFAULT_CTX.lock().unwrap().0 as *const Context
    } else {
        ctx as *const Context
    };
    unsafe { p.as_ref() }
}

pub unsafe fn dev_ref<'a>(dev: *mut libusb_device) -> Option<&'a Device> {
    unsafe { (dev as *const Device).as_ref() }
}

pub unsafe fn handle_ref<'a>(h: *mut libusb_device_handle) -> Option<&'a Handle> {
    unsafe { (h as *const Handle).as_ref() }
}

impl Handle {
    pub fn device(&self) -> &Device {
        unsafe { &*self.dev }
    }

    pub fn context(&self) -> &Context {
        unsafe { &*self.ctx }
    }

    pub fn info(&self) -> &DeviceInfo {
        &self.device().info
    }

    pub fn host(&self) -> &Host {
        &self.context().bus.host
    }

    /// Synchronous control transfer through the emulated bus.
    pub fn control(&self, setup: SetupPacket, data: &[u8]) -> Result<Vec<u8>, UsbError> {
        self.host()
            .control(self.info().id, setup, data, Some(Duration::from_secs(5)))
    }
}

fn active_view(info: &DeviceInfo) -> Option<ConfigView> {
    let active = info.active_config();
    info.configs
        .iter()
        .find(|c| ConfigHeader::parse(c).is_some_and(|h| h.value == active))
        .and_then(|c| ConfigView::parse(c).ok())
}

fn error_code(e: UsbError) -> c_int {
    match e {
        UsbError::Stall => LIBUSB_ERROR_PIPE,
        UsbError::Timeout => LIBUSB_ERROR_TIMEOUT,
        UsbError::Overflow => LIBUSB_ERROR_OVERFLOW,
        UsbError::NoDevice | UsbError::Shutdown => LIBUSB_ERROR_NO_DEVICE,
        UsbError::Invalid => LIBUSB_ERROR_INVALID_PARAM,
        _ => LIBUSB_ERROR_IO,
    }
}

// ---------------------------------------------------------------------------
// Library init/exit and options

unsafe fn new_context() -> Result<*mut Context, c_int> {
    crate::init_logging();
    let bus = bus()?;
    let ctx = Box::new(Context {
        bus,
        events: Events::default(),
        devices: Mutex::new(HashMap::new()),
        debug: AtomicI32::new(0),
    });
    Ok(Box::into_raw(ctx))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_init(ctx: *mut *mut libusb_context) -> c_int {
    unsafe { libusb_init_context(ctx, ptr::null(), 0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_init_context(
    ctx: *mut *mut libusb_context,
    _options: *const libusb_init_option,
    _num_options: c_int,
) -> c_int {
    unsafe {
        if ctx.is_null() {
            let mut d = DEFAULT_CTX.lock().unwrap();
            if d.0 == 0 {
                match new_context() {
                    Ok(p) => d.0 = p as usize,
                    Err(e) => return e,
                }
            }
            d.1 += 1;
            return LIBUSB_SUCCESS;
        }
        match new_context() {
            Ok(p) => {
                *ctx = p as *mut libusb_context;
                LIBUSB_SUCCESS
            }
            Err(e) => e,
        }
    }
}

unsafe fn destroy_context(p: *mut Context) {
    let ctx = unsafe { Box::from_raw(p) };
    for (_, d) in ctx.devices.lock().unwrap().drain() {
        unsafe { libusb_unref_device(d as *mut libusb_device) };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_exit(ctx: *mut libusb_context) {
    if ctx.is_null() {
        let mut d = DEFAULT_CTX.lock().unwrap();
        if d.1 > 0 {
            d.1 -= 1;
            if d.1 == 0 {
                let p = std::mem::replace(&mut d.0, 0);
                unsafe { destroy_context(p as *mut Context) };
            }
        }
        return;
    }
    unsafe { destroy_context(ctx as *mut Context) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_set_debug(ctx: *mut libusb_context, level: c_int) {
    if let Some(c) = unsafe { ctx_ref(ctx) } {
        c.debug.store(level, Ordering::Relaxed);
    }
}

/// C-variadic in libusb.h. Stable Rust can't define variadic functions; on
/// the x86-64 and AArch64 Linux ABIs the single integer/pointer argument of
/// every libusb option is passed exactly like a third named argument.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_set_option(
    ctx: *mut libusb_context,
    option: c_int,
    arg: usize,
) -> c_int {
    match option {
        LIBUSB_OPTION_LOG_LEVEL => {
            if let Some(c) = unsafe { ctx_ref(ctx) } {
                c.debug.store(arg as c_int, Ordering::Relaxed);
            }
            LIBUSB_SUCCESS
        }
        LIBUSB_OPTION_LOG_CB | LIBUSB_OPTION_NO_DEVICE_DISCOVERY => LIBUSB_SUCCESS,
        LIBUSB_OPTION_USE_USBDK => LIBUSB_ERROR_NOT_SUPPORTED,
        _ => LIBUSB_ERROR_INVALID_PARAM,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_set_log_cb(
    _ctx: *mut libusb_context,
    _cb: libusb_log_cb,
    _mode: c_int,
) {
}

// ---------------------------------------------------------------------------
// Devices

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_device_list(
    ctx: *mut libusb_context,
    list: *mut *mut *mut libusb_device,
) -> isize {
    let Some(c) = (unsafe { ctx_ref(ctx) }) else {
        return LIBUSB_ERROR_INVALID_PARAM as isize;
    };
    let connected = c.bus.host.devices();
    let mut devs = c.devices.lock().unwrap();
    // Drop devices that went away.
    devs.retain(|id, d| {
        let keep = connected.iter().any(|i| i.id == *id);
        if !keep {
            unsafe { libusb_unref_device(*d as *mut libusb_device) };
        }
        keep
    });
    let mut out: Vec<*mut libusb_device> = Vec::new();
    for info in connected {
        let d = *devs.entry(info.id).or_insert_with(|| {
            Box::into_raw(Box::new(Device {
                refs: AtomicUsize::new(1),
                ctx: c,
                info: info.clone(),
            })) as usize
        });
        out.push(unsafe { libusb_ref_device(d as *mut libusb_device) });
    }
    let n = out.len();
    unsafe {
        let arr = libc::calloc(n + 1, size_of::<*mut libusb_device>()) as *mut *mut libusb_device;
        if arr.is_null() {
            return LIBUSB_ERROR_NO_MEM as isize;
        }
        ptr::copy_nonoverlapping(out.as_ptr(), arr, n);
        *list = arr;
    }
    n as isize
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_free_device_list(
    list: *mut *mut libusb_device,
    unref_devices: c_int,
) {
    if list.is_null() {
        return;
    }
    unsafe {
        if unref_devices != 0 {
            let mut p = list;
            while !(*p).is_null() {
                libusb_unref_device(*p);
                p = p.add(1);
            }
        }
        libc::free(list as *mut c_void);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_ref_device(dev: *mut libusb_device) -> *mut libusb_device {
    if let Some(d) = unsafe { dev_ref(dev) } {
        d.refs.fetch_add(1, Ordering::AcqRel);
    }
    dev
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_unref_device(dev: *mut libusb_device) {
    let Some(d) = (unsafe { dev_ref(dev) }) else {
        return;
    };
    if d.refs.fetch_sub(1, Ordering::AcqRel) == 1 {
        drop(unsafe { Box::from_raw(dev as *mut Device) });
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_device_descriptor(
    dev: *mut libusb_device,
    desc: *mut libusb_device_descriptor,
) -> c_int {
    let (Some(d), false) = (unsafe { dev_ref(dev) }, desc.is_null()) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    let b = &d.info.device_descriptor;
    if b.len() < 18 {
        return LIBUSB_ERROR_IO;
    }
    let le = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    unsafe {
        *desc = libusb_device_descriptor {
            bLength: b[0],
            bDescriptorType: b[1],
            bcdUSB: le(2),
            bDeviceClass: b[4],
            bDeviceSubClass: b[5],
            bDeviceProtocol: b[6],
            bMaxPacketSize0: b[7],
            idVendor: le(8),
            idProduct: le(10),
            bcdDevice: le(12),
            iManufacturer: b[14],
            iProduct: b[15],
            iSerialNumber: b[16],
            bNumConfigurations: b[17],
        };
    }
    LIBUSB_SUCCESS
}

unsafe fn put_config(raw: &[u8], out: *mut *mut libusb_config_descriptor) -> c_int {
    if out.is_null() {
        return LIBUSB_ERROR_INVALID_PARAM;
    }
    match parse_config(config_bytes(raw)) {
        Ok(p) => {
            unsafe { *out = p };
            LIBUSB_SUCCESS
        }
        Err(e) => e,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_config_descriptor(
    dev: *mut libusb_device,
    config_index: u8,
    config: *mut *mut libusb_config_descriptor,
) -> c_int {
    let Some(d) = (unsafe { dev_ref(dev) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    match d.info.configs.get(config_index as usize) {
        Some(raw) => unsafe { put_config(raw, config) },
        None => LIBUSB_ERROR_NOT_FOUND,
    }
}

fn config_by_value(info: &DeviceInfo, value: u8) -> Option<&Vec<u8>> {
    info.configs
        .iter()
        .find(|c| ConfigHeader::parse(c).is_some_and(|h| h.value == value))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_active_config_descriptor(
    dev: *mut libusb_device,
    config: *mut *mut libusb_config_descriptor,
) -> c_int {
    let Some(d) = (unsafe { dev_ref(dev) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    match config_by_value(&d.info, d.info.active_config()) {
        Some(raw) => unsafe { put_config(raw, config) },
        None => LIBUSB_ERROR_NOT_FOUND,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_config_descriptor_by_value(
    dev: *mut libusb_device,
    value: u8,
    config: *mut *mut libusb_config_descriptor,
) -> c_int {
    let Some(d) = (unsafe { dev_ref(dev) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    match config_by_value(&d.info, value) {
        Some(raw) => unsafe { put_config(raw, config) },
        None => LIBUSB_ERROR_NOT_FOUND,
    }
}

unsafe fn put_iads(
    raw: &[u8],
    out: *mut *mut libusb_interface_association_descriptor_array,
) -> c_int {
    if out.is_null() {
        return LIBUSB_ERROR_INVALID_PARAM;
    }
    match parse_iads(config_bytes(raw)) {
        Ok(p) => {
            unsafe { *out = p };
            LIBUSB_SUCCESS
        }
        Err(e) => e,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_interface_association_descriptors(
    dev: *mut libusb_device,
    config_index: u8,
    iad_array: *mut *mut libusb_interface_association_descriptor_array,
) -> c_int {
    let Some(d) = (unsafe { dev_ref(dev) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    match d.info.configs.get(config_index as usize) {
        Some(raw) => unsafe { put_iads(raw, iad_array) },
        None => LIBUSB_ERROR_NOT_FOUND,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_active_interface_association_descriptors(
    dev: *mut libusb_device,
    iad_array: *mut *mut libusb_interface_association_descriptor_array,
) -> c_int {
    let Some(d) = (unsafe { dev_ref(dev) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    match config_by_value(&d.info, d.info.active_config()) {
        Some(raw) => unsafe { put_iads(raw, iad_array) },
        None => LIBUSB_ERROR_NOT_FOUND,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_bos_descriptor(
    handle: *mut libusb_device_handle,
    bos: *mut *mut libusb_bos_descriptor,
) -> c_int {
    let (Some(h), false) = (unsafe { handle_ref(handle) }, bos.is_null()) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    // Like libusb: read it from the device (header first, then everything).
    let get = |len: u16| h.control(SetupPacket::new(0x80, 0x06, 0x0f00, 0, len), &[]);
    let head = match get(5) {
        Ok(v) if v.len() >= 5 => v,
        Ok(_) => return LIBUSB_ERROR_IO,
        Err(e) => return error_code(e),
    };
    let total = u16::from_le_bytes([head[2], head[3]]);
    match get(total) {
        Ok(full) => match parse_bos(&full) {
            Ok(p) => {
                unsafe { *bos = p };
                LIBUSB_SUCCESS
            }
            Err(e) => e,
        },
        Err(e) => error_code(e),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_bus_number(dev: *mut libusb_device) -> u8 {
    unsafe { dev_ref(dev) }.map_or(0, |d| d.info.bus_number)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_port_number(dev: *mut libusb_device) -> u8 {
    unsafe { dev_ref(dev) }.map_or(0, |d| d.info.port)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_port_numbers(
    dev: *mut libusb_device,
    port_numbers: *mut u8,
    len: c_int,
) -> c_int {
    let Some(d) = (unsafe { dev_ref(dev) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    if len < 1 || port_numbers.is_null() {
        return LIBUSB_ERROR_OVERFLOW;
    }
    unsafe { *port_numbers = d.info.port };
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_port_path(
    _ctx: *mut libusb_context,
    dev: *mut libusb_device,
    path: *mut u8,
    path_length: u8,
) -> c_int {
    unsafe { libusb_get_port_numbers(dev, path, path_length as c_int) }
}

/// Root hubs aren't emulated, so devices have no parent.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_parent(_dev: *mut libusb_device) -> *mut libusb_device {
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_device_address(dev: *mut libusb_device) -> u8 {
    unsafe { dev_ref(dev) }.map_or(0, |d| d.info.address)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_device_speed(dev: *mut libusb_device) -> c_int {
    match unsafe { dev_ref(dev) }.map(|d| d.info.speed) {
        Some(Speed::Low) => LIBUSB_SPEED_LOW,
        Some(Speed::Full) => LIBUSB_SPEED_FULL,
        Some(Speed::High) => LIBUSB_SPEED_HIGH,
        Some(Speed::Super) => LIBUSB_SPEED_SUPER,
        Some(Speed::SuperPlus) => LIBUSB_SPEED_SUPER_PLUS,
        None => LIBUSB_SPEED_UNKNOWN,
    }
}

/// libusb's `find_endpoint`: first match over all interfaces/alt settings of the active config.
fn find_endpoint(
    info: &DeviceInfo,
    ep: u8,
    alt: Option<(u8, u8)>,
) -> Option<emuvc_usb::desc::EndpointView> {
    let view = active_view(info)?;
    view.interfaces
        .iter()
        .flat_map(|i| i.alts.iter())
        .filter(|a| alt.is_none_or(|(i, n)| a.desc.number == i && a.desc.alt == n))
        .flat_map(|a| a.endpoints.iter())
        .find(|e| e.desc.address == ep)
        .cloned()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_max_packet_size(
    dev: *mut libusb_device,
    endpoint: c_uchar,
) -> c_int {
    let Some(d) = (unsafe { dev_ref(dev) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    if d.info.active_config() == 0 {
        return LIBUSB_ERROR_OTHER;
    }
    find_endpoint(&d.info, endpoint, None)
        .map_or(LIBUSB_ERROR_NOT_FOUND, |e| e.desc.max_packet_size as c_int)
}

fn iso_packet_size(info: &DeviceInfo, ep: Option<emuvc_usb::desc::EndpointView>) -> c_int {
    match ep {
        Some(e) => match e.desc.transfer_type() {
            emuvc_usb::TransferType::Isochronous | emuvc_usb::TransferType::Interrupt => {
                e.bytes_per_interval(info.speed) as c_int
            }
            _ => (e.desc.max_packet_size & 0x7ff) as c_int,
        },
        None => LIBUSB_ERROR_NOT_FOUND,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_max_iso_packet_size(
    dev: *mut libusb_device,
    endpoint: c_uchar,
) -> c_int {
    let Some(d) = (unsafe { dev_ref(dev) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    if d.info.active_config() == 0 {
        return LIBUSB_ERROR_OTHER;
    }
    iso_packet_size(&d.info, find_endpoint(&d.info, endpoint, None))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_max_alt_packet_size(
    dev: *mut libusb_device,
    interface_number: c_int,
    alternate_setting: c_int,
    endpoint: c_uchar,
) -> c_int {
    let Some(d) = (unsafe { dev_ref(dev) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    if d.info.active_config() == 0 {
        return LIBUSB_ERROR_OTHER;
    }
    iso_packet_size(
        &d.info,
        find_endpoint(
            &d.info,
            endpoint,
            Some((interface_number as u8, alternate_setting as u8)),
        ),
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_device_string(
    dev: *mut libusb_device,
    string_type: c_int,
    data: *mut c_char,
    length: c_int,
) -> c_int {
    let Some(d) = (unsafe { dev_ref(dev) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    let s = match string_type {
        LIBUSB_DEVICE_STRING_MANUFACTURER => &d.info.manufacturer,
        LIBUSB_DEVICE_STRING_PRODUCT => &d.info.product,
        LIBUSB_DEVICE_STRING_SERIAL_NUMBER => &d.info.serial,
        _ => return LIBUSB_ERROR_INVALID_PARAM,
    };
    let Some(s) = s else {
        return LIBUSB_ERROR_NOT_FOUND;
    };
    if data.is_null() || length <= 0 {
        return LIBUSB_ERROR_INVALID_PARAM;
    }
    let n = s.len().min(length as usize - 1);
    unsafe {
        ptr::copy_nonoverlapping(s.as_ptr(), data as *mut u8, n);
        *data.add(n) = 0;
    }
    n as c_int
}

// ---------------------------------------------------------------------------
// Device handles

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_wrap_sys_device(
    _ctx: *mut libusb_context,
    _sys_dev: isize,
    _handle: *mut *mut libusb_device_handle,
) -> c_int {
    warn!("emuvc: libusb_wrap_sys_device is not supported");
    LIBUSB_ERROR_NOT_SUPPORTED
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_open(
    dev: *mut libusb_device,
    handle: *mut *mut libusb_device_handle,
) -> c_int {
    let (Some(d), false) = (unsafe { dev_ref(dev) }, handle.is_null()) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    if !d.info.connected() {
        return LIBUSB_ERROR_NO_DEVICE;
    }
    unsafe { libusb_ref_device(dev) };
    let h = Box::new(Handle {
        dev: dev as *mut Device,
        ctx: d.ctx,
        claimed: Mutex::new(0),
        auto_detach: AtomicBool::new(false),
        inflight: Mutex::new(HashMap::new()),
    });
    debug!("open {}", d.info.name);
    unsafe { *handle = Box::into_raw(h) as *mut libusb_device_handle };
    LIBUSB_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_open_device_with_vid_pid(
    ctx: *mut libusb_context,
    vendor_id: u16,
    product_id: u16,
) -> *mut libusb_device_handle {
    let mut list: *mut *mut libusb_device = ptr::null_mut();
    let n = unsafe { libusb_get_device_list(ctx, &mut list) };
    let mut handle: *mut libusb_device_handle = ptr::null_mut();
    for i in 0..n.max(0) as usize {
        let dev = unsafe { *list.add(i) };
        let d = unsafe { dev_ref(dev) }.unwrap();
        if d.info
            .descriptor()
            .is_some_and(|dd| dd.vendor_id == vendor_id && dd.product_id == product_id)
        {
            if unsafe { libusb_open(dev, &mut handle) } != 0 {
                handle = ptr::null_mut();
            }
            break;
        }
    }
    unsafe { libusb_free_device_list(list, 1) };
    handle
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_close(handle: *mut libusb_device_handle) {
    let Some(h) = (unsafe { handle_ref(handle) }) else {
        return;
    };
    let claimed = *h.claimed.lock().unwrap();
    for i in 0..64 {
        if claimed & (1 << i) != 0 {
            unsafe { libusb_release_interface(handle, i) };
        }
    }
    // Like libusb (and the kernel closing the usbfs fd), discard transfers
    // still in flight: they never complete and may be freed right away.
    let inflight: Vec<(usize, u64)> = h.inflight.lock().unwrap().drain().collect();
    if !inflight.is_empty() {
        log::info!(
            "emuvc: device handle closed with {} transfer(s) in flight",
            inflight.len()
        );
        h.context()
            .events
            .orphan(inflight.iter().map(|&(_, id)| id));
        for (t, id) in inflight {
            h.host().cancel(id);
            unsafe { transfer::mark_idle(t as *mut libusb_transfer) };
        }
    }
    // Like libusb, closing a handle wakes up event handlers.
    h.context().events.interrupt();
    let dev = h.dev;
    drop(unsafe { Box::from_raw(handle as *mut Handle) });
    unsafe { libusb_unref_device(dev as *mut libusb_device) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_device(
    handle: *mut libusb_device_handle,
) -> *mut libusb_device {
    unsafe { handle_ref(handle) }.map_or(ptr::null_mut(), |h| h.dev as *mut libusb_device)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_configuration(
    handle: *mut libusb_device_handle,
    config: *mut c_int,
) -> c_int {
    let (Some(h), false) = (unsafe { handle_ref(handle) }, config.is_null()) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    unsafe { *config = h.info().active_config() as c_int };
    LIBUSB_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_set_configuration(
    handle: *mut libusb_device_handle,
    configuration: c_int,
) -> c_int {
    let Some(h) = (unsafe { handle_ref(handle) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    if *h.claimed.lock().unwrap() != 0 {
        return LIBUSB_ERROR_BUSY;
    }
    let value = if configuration == -1 {
        0
    } else {
        configuration as u16
    };
    match h.control(SetupPacket::new(0x00, 0x09, value, 0, 0), &[]) {
        Ok(_) => LIBUSB_SUCCESS,
        Err(UsbError::Stall) => LIBUSB_ERROR_NOT_FOUND,
        Err(e) => error_code(e),
    }
}

fn interface_exists(info: &DeviceInfo, iface: c_int) -> bool {
    (0..64).contains(&iface)
        && active_view(info).is_some_and(|v| v.interface(iface as u8).is_some())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_claim_interface(
    handle: *mut libusb_device_handle,
    interface_number: c_int,
) -> c_int {
    let Some(h) = (unsafe { handle_ref(handle) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    let info = h.info();
    if !info.connected() {
        return LIBUSB_ERROR_NO_DEVICE;
    }
    if !interface_exists(info, interface_number) {
        return LIBUSB_ERROR_NOT_FOUND;
    }
    let bit = 1u64 << interface_number;
    if *h.claimed.lock().unwrap() & bit != 0 {
        return LIBUSB_SUCCESS;
    }
    let bus = h.context().bus;
    let drv = bus.drivers(info);
    if drv.bound & bit != 0 && drv.detached & bit == 0 {
        if !h.auto_detach.load(Ordering::Relaxed) {
            debug!("claim {interface_number}: kernel driver active");
            return LIBUSB_ERROR_BUSY;
        }
        let r = unsafe { libusb_detach_kernel_driver(handle, interface_number) };
        if r != 0 {
            return r;
        }
    }
    let mut claims = bus.claims.lock().unwrap();
    let key = (info.id, interface_number as u8);
    if claims.get(&key).is_some_and(|&o| o != handle as usize) {
        return LIBUSB_ERROR_BUSY;
    }
    claims.insert(key, handle as usize);
    *h.claimed.lock().unwrap() |= bit;
    LIBUSB_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_release_interface(
    handle: *mut libusb_device_handle,
    interface_number: c_int,
) -> c_int {
    let Some(h) = (unsafe { handle_ref(handle) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    if !(0..64).contains(&interface_number) {
        return LIBUSB_ERROR_INVALID_PARAM;
    }
    let bit = 1u64 << interface_number;
    {
        let mut c = h.claimed.lock().unwrap();
        if *c & bit == 0 {
            return LIBUSB_ERROR_NOT_FOUND;
        }
        *c &= !bit;
    }
    h.context()
        .bus
        .claims
        .lock()
        .unwrap()
        .remove(&(h.info().id, interface_number as u8));
    if h.auto_detach.load(Ordering::Relaxed) {
        unsafe { libusb_attach_kernel_driver(handle, interface_number) };
    }
    if !h.info().connected() {
        return LIBUSB_ERROR_NO_DEVICE;
    }
    LIBUSB_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_set_interface_alt_setting(
    handle: *mut libusb_device_handle,
    interface_number: c_int,
    alternate_setting: c_int,
) -> c_int {
    let Some(h) = (unsafe { handle_ref(handle) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    if !(0..64).contains(&interface_number)
        || *h.claimed.lock().unwrap() & (1 << interface_number) == 0
    {
        return LIBUSB_ERROR_NOT_FOUND;
    }
    // The kernel validates against the descriptors before sending SET_INTERFACE.
    let exists = active_view(h.info()).is_some_and(|v| {
        v.alt(interface_number as u8, alternate_setting as u8)
            .is_some()
    });
    if !exists {
        return LIBUSB_ERROR_NOT_FOUND;
    }
    match h.control(
        SetupPacket::new(
            0x01,
            0x0b,
            alternate_setting as u16,
            interface_number as u16,
            0,
        ),
        &[],
    ) {
        Ok(_) => LIBUSB_SUCCESS,
        Err(UsbError::NoDevice) => LIBUSB_ERROR_NO_DEVICE,
        Err(_) => LIBUSB_ERROR_OTHER,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_clear_halt(
    handle: *mut libusb_device_handle,
    endpoint: c_uchar,
) -> c_int {
    let Some(h) = (unsafe { handle_ref(handle) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    if h.info().endpoint(endpoint).is_none() {
        return LIBUSB_ERROR_NOT_FOUND;
    }
    match h.control(SetupPacket::new(0x02, 0x01, 0, endpoint as u16, 0), &[]) {
        Ok(_) => LIBUSB_SUCCESS,
        Err(e) => error_code(e),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_reset_device(handle: *mut libusb_device_handle) -> c_int {
    let Some(h) = (unsafe { handle_ref(handle) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    match h.host().reset_device(h.info().id) {
        Ok(()) => LIBUSB_SUCCESS,
        Err(UsbError::NoDevice) => LIBUSB_ERROR_NOT_FOUND,
        Err(e) => error_code(e),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_kernel_driver_active(
    handle: *mut libusb_device_handle,
    interface_number: c_int,
) -> c_int {
    let Some(h) = (unsafe { handle_ref(handle) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    if !interface_exists(h.info(), interface_number) {
        return LIBUSB_ERROR_NOT_FOUND;
    }
    let bit = 1u64 << interface_number;
    let d = h.context().bus.drivers(h.info());
    (d.bound & bit != 0 && d.detached & bit == 0) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_detach_kernel_driver(
    handle: *mut libusb_device_handle,
    interface_number: c_int,
) -> c_int {
    let Some(h) = (unsafe { handle_ref(handle) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    if !interface_exists(h.info(), interface_number) {
        return LIBUSB_ERROR_NOT_FOUND;
    }
    let bit = 1u64 << interface_number;
    let bus = h.context().bus;
    bus.drivers(h.info());
    let mut map = bus.drivers.lock().unwrap();
    let d = map.get_mut(&h.info().id).unwrap();
    if d.bound & bit == 0 || d.detached & bit != 0 {
        return LIBUSB_ERROR_NOT_FOUND;
    }
    debug!("detach kernel driver from interface {interface_number}");
    d.detached |= bit;
    LIBUSB_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_attach_kernel_driver(
    handle: *mut libusb_device_handle,
    interface_number: c_int,
) -> c_int {
    let Some(h) = (unsafe { handle_ref(handle) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    if !interface_exists(h.info(), interface_number) {
        return LIBUSB_ERROR_NOT_FOUND;
    }
    let bit = 1u64 << interface_number;
    let bus = h.context().bus;
    if bus
        .claims
        .lock()
        .unwrap()
        .contains_key(&(h.info().id, interface_number as u8))
    {
        return LIBUSB_ERROR_BUSY;
    }
    bus.drivers(h.info());
    let mut map = bus.drivers.lock().unwrap();
    let d = map.get_mut(&h.info().id).unwrap();
    if d.bound & bit == 0 {
        return LIBUSB_ERROR_NOT_FOUND;
    }
    if d.detached & bit == 0 {
        return LIBUSB_ERROR_BUSY;
    }
    debug!("re-attach kernel driver to interface {interface_number}");
    d.detached &= !bit;
    LIBUSB_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_set_auto_detach_kernel_driver(
    handle: *mut libusb_device_handle,
    enable: c_int,
) -> c_int {
    let Some(h) = (unsafe { handle_ref(handle) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    h.auto_detach.store(enable != 0, Ordering::Relaxed);
    LIBUSB_SUCCESS
}

/// Same algorithm as libusb: first LANGID, then the string, non-ASCII → '?'.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_string_descriptor_ascii(
    handle: *mut libusb_device_handle,
    desc_index: u8,
    data: *mut c_uchar,
    length: c_int,
) -> c_int {
    if desc_index == 0 || data.is_null() || length < 1 {
        return LIBUSB_ERROR_INVALID_PARAM;
    }
    let mut tbuf = [0u8; 255];
    let get = |index: u8, lang: u16, buf: &mut [u8; 255]| unsafe {
        transfer::libusb_control_transfer(
            handle,
            0x80,
            0x06,
            0x0300 | index as u16,
            lang,
            buf.as_mut_ptr(),
            255,
            1000,
        )
    };
    let r = get(0, 0, &mut tbuf);
    if r < 0 {
        return r;
    }
    if r < 4 {
        return LIBUSB_ERROR_IO;
    }
    let langid = u16::from_le_bytes([tbuf[2], tbuf[3]]);
    let r = get(desc_index, langid, &mut tbuf);
    if r < 0 {
        return r;
    }
    if tbuf[1] != 0x03 || tbuf[0] as c_int > r || r < 2 {
        return LIBUSB_ERROR_IO;
    }
    let mut di = 0usize;
    let mut si = 2usize;
    while si + 1 < tbuf[0] as usize && di < length as usize - 1 {
        let c = if tbuf[si + 1] != 0 { b'?' } else { tbuf[si] };
        unsafe { *data.add(di) = c };
        di += 1;
        si += 2;
    }
    unsafe { *data.add(di) = 0 };
    di as c_int
}
