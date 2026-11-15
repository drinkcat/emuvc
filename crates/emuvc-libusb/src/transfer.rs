//! Asynchronous transfers, event handling and the synchronous transfer API.
//!
//! Completions from the host engine are queued on the context and handed to
//! the application inside `libusb_handle_events*`, on the calling thread,
//! following libusb's event-lock protocol: one thread at a time handles
//! events; others wait for it to finish a cycle. Synchronous transfers are
//! built on the async API exactly like in libusb, so callbacks of unrelated
//! transfers may run inside `libusb_control_transfer` too.

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::collections::{HashSet, VecDeque};
use std::ffi::{c_int, c_uchar, c_uint, c_void};
use std::mem::offset_of;
use std::ptr;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use emuvc_usb::host::IsoPacketResult;
use emuvc_usb::{Completion, SetupPacket, Urb, UrbKind, UsbError};
use log::{trace, warn};

use crate::core::{Context, ctx_ref, handle_ref};
use crate::ffi::*;

const IDLE: u8 = 0;
const SUBMITTED: u8 = 1;
const CANCELLING: u8 = 2;

/// Private header in front of every `libusb_transfer` we allocate.
#[repr(C)]
struct Block {
    iso_alloc: usize,
    state: AtomicU8,
    urb: AtomicU64,
    t: libusb_transfer,
}

fn layout(iso_packets: usize) -> Layout {
    let size = size_of::<Block>() + iso_packets * size_of::<libusb_iso_packet_descriptor>();
    Layout::from_size_align(size, align_of::<Block>()).unwrap()
}

unsafe fn block<'a>(t: *mut libusb_transfer) -> &'a Block {
    unsafe { &*((t as *mut u8).sub(offset_of!(Block, t)) as *const Block) }
}

unsafe fn iso_descs<'a>(t: *mut libusb_transfer) -> &'a mut [libusb_iso_packet_descriptor] {
    unsafe {
        let n = (*t).num_iso_packets.max(0) as usize;
        std::slice::from_raw_parts_mut((*t).iso_packet_desc.as_mut_ptr(), n)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_alloc_transfer(iso_packets: c_int) -> *mut libusb_transfer {
    if crate::faults::check("alloc_transfer").is_some() {
        return ptr::null_mut();
    }
    let n = iso_packets.max(0) as usize;
    unsafe {
        let p = alloc_zeroed(layout(n)) as *mut Block;
        if p.is_null() {
            return ptr::null_mut();
        }
        (*p).iso_alloc = n;
        &raw mut (*p).t
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_free_transfer(transfer: *mut libusb_transfer) {
    if transfer.is_null() {
        return;
    }
    unsafe {
        let b = block(transfer);
        if b.state.load(Ordering::Acquire) != IDLE {
            // Freeing an in-flight transfer is undefined behaviour in libusb;
            // leak it rather than letting the completion scribble on freed memory.
            warn!("emuvc: libusb_free_transfer on an in-flight transfer (leaked)");
            return;
        }
        if (*transfer).flags & LIBUSB_TRANSFER_FREE_BUFFER != 0 && !(*transfer).buffer.is_null() {
            libc::free((*transfer).buffer as *mut c_void);
        }
        let n = b.iso_alloc;
        dealloc(b as *const Block as *mut u8, layout(n));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_submit_transfer(transfer: *mut libusb_transfer) -> c_int {
    if let Some(e) = crate::faults::check("submit_transfer") {
        return e;
    }
    if transfer.is_null() {
        return LIBUSB_ERROR_INVALID_PARAM;
    }
    let per_type = match unsafe { (*transfer).type_ } {
        LIBUSB_TRANSFER_TYPE_ISOCHRONOUS => "submit_transfer_iso",
        LIBUSB_TRANSFER_TYPE_BULK => "submit_transfer_bulk",
        LIBUSB_TRANSFER_TYPE_INTERRUPT => "submit_transfer_interrupt",
        _ => "submit_transfer_control",
    };
    if let Some(e) = crate::faults::check(per_type) {
        return e;
    }
    unsafe {
        let t = &mut *transfer;
        let Some(h) = handle_ref(t.dev_handle) else {
            return LIBUSB_ERROR_INVALID_PARAM;
        };
        let b = block(transfer);
        if b.state.load(Ordering::Acquire) != IDLE {
            return LIBUSB_ERROR_BUSY;
        }
        let dev = h.info().id;
        let len = t.length.max(0) as usize;
        let buf: &[u8] = if t.buffer.is_null() {
            &[]
        } else {
            std::slice::from_raw_parts(t.buffer, len)
        };
        let is_in = t.endpoint & 0x80 != 0;
        let mut urb = match t.type_ {
            LIBUSB_TRANSFER_TYPE_CONTROL => {
                if len < 8 {
                    return LIBUSB_ERROR_INVALID_PARAM;
                }
                let setup = SetupPacket::from_bytes(buf[..8].try_into().unwrap());
                let wlen = setup.length as usize;
                if wlen > len - 8 {
                    return LIBUSB_ERROR_INVALID_PARAM;
                }
                let data = if setup.request_type & 0x80 == 0 {
                    buf[8..8 + wlen].to_vec()
                } else {
                    Vec::new()
                };
                Urb::control(dev, setup, data)
            }
            LIBUSB_TRANSFER_TYPE_BULK | LIBUSB_TRANSFER_TYPE_INTERRUPT => {
                let kind = if t.type_ == LIBUSB_TRANSFER_TYPE_BULK {
                    UrbKind::Bulk
                } else {
                    UrbKind::Interrupt
                };
                let data = if is_in { Vec::new() } else { buf.to_vec() };
                Urb {
                    device: dev,
                    endpoint: t.endpoint,
                    kind,
                    data,
                    length: len,
                    timeout: None,
                    short_not_ok: false,
                }
            }
            LIBUSB_TRANSFER_TYPE_ISOCHRONOUS => {
                let packets: Vec<usize> = iso_descs(transfer)
                    .iter()
                    .map(|d| d.length as usize)
                    .collect();
                if packets.is_empty() || packets.iter().sum::<usize>() > len {
                    return LIBUSB_ERROR_INVALID_PARAM;
                }
                let data = if is_in {
                    Vec::new()
                } else {
                    buf[..packets.iter().sum::<usize>()].to_vec()
                };
                let mut urb = Urb::iso_in(dev, t.endpoint, packets);
                urb.data = data;
                urb
            }
            _ => return LIBUSB_ERROR_NOT_SUPPORTED,
        };
        if t.timeout > 0 {
            urb.timeout = Some(Duration::from_millis(t.timeout as u64));
        }
        urb.short_not_ok = is_in && t.flags & LIBUSB_TRANSFER_SHORT_NOT_OK != 0;
        b.state.store(SUBMITTED, Ordering::Release);
        let events = h.context().events.clone();
        let tp = transfer as usize;
        h.inflight.lock().unwrap().insert(tp, 0);
        let done = Box::new(move |c: Completion| {
            events.push(tp, c);
        });
        match h.host().submit(urb, done) {
            Ok(id) => {
                trace!("submit transfer {transfer:p} as urb {id}");
                b.urb.store(id, Ordering::Release);
                // Unless it already completed.
                if let Some(v) = h.inflight.lock().unwrap().get_mut(&tp) {
                    *v = id;
                }
                LIBUSB_SUCCESS
            }
            Err(e) => {
                h.inflight.lock().unwrap().remove(&tp);
                b.state.store(IDLE, Ordering::Release);
                match e {
                    UsbError::NoDevice => LIBUSB_ERROR_NO_DEVICE,
                    _ => LIBUSB_ERROR_IO,
                }
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_cancel_transfer(transfer: *mut libusb_transfer) -> c_int {
    if let Some(e) = crate::faults::check("cancel_transfer") {
        return e;
    }
    if transfer.is_null() {
        return LIBUSB_ERROR_INVALID_PARAM;
    }
    unsafe {
        let b = block(transfer);
        if b.state
            .compare_exchange(SUBMITTED, CANCELLING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return LIBUSB_ERROR_NOT_FOUND;
        }
        let Some(h) = handle_ref((*transfer).dev_handle) else {
            return LIBUSB_ERROR_INVALID_PARAM;
        };
        h.host().cancel(b.urb.load(Ordering::Acquire));
        LIBUSB_SUCCESS
    }
}

fn transfer_status(s: Result<(), UsbError>) -> c_int {
    match s {
        Ok(()) => LIBUSB_TRANSFER_COMPLETED,
        Err(UsbError::Timeout) => LIBUSB_TRANSFER_TIMED_OUT,
        Err(UsbError::Cancelled) => LIBUSB_TRANSFER_CANCELLED,
        Err(UsbError::Stall) => LIBUSB_TRANSFER_STALL,
        Err(UsbError::NoDevice | UsbError::Shutdown) => LIBUSB_TRANSFER_NO_DEVICE,
        Err(UsbError::Overflow) => LIBUSB_TRANSFER_OVERFLOW,
        Err(_) => LIBUSB_TRANSFER_ERROR,
    }
}

/// Fills in a completed transfer and runs its callback (event handler thread).
unsafe fn complete(transfer: *mut libusb_transfer, c: Completion) {
    unsafe {
        let t = &mut *transfer;
        let is_in = t.endpoint & 0x80 != 0;
        let copy = |dst: *mut c_uchar, src: &[u8], room: usize| {
            let n = src.len().min(room);
            if n > 0 && !dst.is_null() {
                ptr::copy_nonoverlapping(src.as_ptr(), dst, n);
            }
            n
        };
        match t.type_ {
            LIBUSB_TRANSFER_TYPE_CONTROL => {
                t.actual_length = if is_control_in(t) {
                    copy(t.buffer.add(8), &c.data, t.length.max(8) as usize - 8) as c_int
                } else {
                    c.actual_length as c_int
                };
            }
            LIBUSB_TRANSFER_TYPE_ISOCHRONOUS => {
                let mut off = 0usize;
                for (d, r) in
                    iso_descs(transfer)
                        .iter_mut()
                        .zip(c.iso.iter().chain(std::iter::repeat(&IsoPacketResult {
                            status: Err(UsbError::Cancelled),
                            data: Vec::new(),
                            actual_length: 0,
                        })))
                {
                    d.actual_length = r.actual_length as c_uint;
                    d.status = transfer_status(r.status);
                    if is_in {
                        copy(t.buffer.add(off), &r.data, d.length as usize);
                    }
                    off += d.length as usize;
                }
                t.actual_length = c.actual_length as c_int;
            }
            _ => {
                t.actual_length = if is_in {
                    copy(t.buffer, &c.data, t.length.max(0) as usize)
                } else {
                    c.actual_length
                } as c_int;
            }
        }
        t.status = transfer_status(c.status);
        if let Some(h) = handle_ref(t.dev_handle) {
            h.inflight.lock().unwrap().remove(&(transfer as usize));
        }
        block(transfer).state.store(IDLE, Ordering::Release);
        let flags = t.flags;
        if let Some(cb) = t.callback {
            cb(transfer);
        }
        if flags & LIBUSB_TRANSFER_FREE_TRANSFER != 0 {
            libusb_free_transfer(transfer);
        }
    }
}

/// Detaches a transfer from its URB (handle closed while in flight).
pub(crate) unsafe fn mark_idle(transfer: *mut libusb_transfer) {
    unsafe { block(transfer).state.store(IDLE, Ordering::Release) };
}

unsafe fn is_control_in(t: &libusb_transfer) -> bool {
    !t.buffer.is_null() && unsafe { *t.buffer } & 0x80 != 0
}

// ---------------------------------------------------------------------------
// Event handling

#[derive(Default)]
struct EvState {
    queue: VecDeque<(usize, Completion)>,
    /// Somebody holds the events lock (is the event handler).
    locked: bool,
    /// Bumped by `libusb_interrupt_event_handler` and `libusb_close`.
    interrupts: u64,
    /// Bumped whenever an event handler releases the lock.
    cycles: u64,
    /// URBs whose transfer was discarded by libusb_close: drop their completions.
    orphans: HashSet<u64>,
    /// The context is gone (libusb_exit): drop every completion.
    closed: bool,
}

#[derive(Default)]
pub struct Events {
    m: Mutex<EvState>,
    cv: Condvar,
}

impl Events {
    pub fn push(&self, transfer: usize, c: Completion) {
        let mut g = self.m.lock().unwrap();
        if g.closed || g.orphans.remove(&c.id) {
            return;
        }
        g.queue.push_back((transfer, c));
        drop(g);
        self.cv.notify_all();
    }

    /// The context was destroyed: late completions are dropped.
    pub fn close(&self) {
        let mut g = self.m.lock().unwrap();
        g.closed = true;
        g.queue.clear();
    }

    /// Forgets these URBs: queued or future completions are dropped.
    pub fn orphan(&self, ids: impl Iterator<Item = u64>) {
        let mut g = self.m.lock().unwrap();
        g.orphans.extend(ids.filter(|&id| id != 0));
        let EvState { queue, orphans, .. } = &mut *g;
        queue.retain(|(_, c)| !orphans.remove(&c.id));
    }

    pub fn interrupt(&self) {
        self.m.lock().unwrap().interrupts += 1;
        self.cv.notify_all();
    }

    /// Waits on the condvar until `deadline`; returns None on timeout.
    fn wait<'a>(
        &self,
        g: MutexGuard<'a, EvState>,
        deadline: Option<Instant>,
    ) -> Option<MutexGuard<'a, EvState>> {
        match deadline {
            None => Some(self.cv.wait(g).unwrap()),
            Some(d) => {
                let now = Instant::now();
                if now >= d {
                    return None;
                }
                Some(self.cv.wait_timeout(g, d - now).unwrap().0)
            }
        }
    }

    /// One event handling cycle; the caller holds the events lock.
    unsafe fn run_cycle<'a>(
        &'a self,
        mut g: MutexGuard<'a, EvState>,
        deadline: Option<Instant>,
        completed: *const c_int,
    ) -> MutexGuard<'a, EvState> {
        let interrupts = g.interrupts;
        let mut handled = false;
        loop {
            if let Some((t, c)) = g.queue.pop_front() {
                drop(g);
                unsafe { complete(t as *mut libusb_transfer, c) };
                handled = true;
                g = self.m.lock().unwrap();
                continue;
            }
            if handled || g.interrupts != interrupts || unsafe { is_set(completed) } {
                return g;
            }
            match self.wait(g, deadline) {
                Some(ng) => g = ng,
                None => return self.m.lock().unwrap(),
            }
        }
    }

    fn release(&self, mut g: MutexGuard<'_, EvState>) {
        g.locked = false;
        g.cycles += 1;
        drop(g);
        self.cv.notify_all();
    }
}

unsafe fn is_set(completed: *const c_int) -> bool {
    !completed.is_null() && unsafe { ptr::read_volatile(completed) } != 0
}

unsafe fn timeout_of(tv: *const libc::timeval) -> Option<Instant> {
    let tv = unsafe { tv.as_ref() }?;
    let d = Duration::from_secs(tv.tv_sec.max(0) as u64)
        + Duration::from_micros(tv.tv_usec.max(0) as u64);
    Some(Instant::now() + d)
}

unsafe fn handle_events(
    ctx: &Context,
    deadline: Option<Instant>,
    completed: *const c_int,
) -> c_int {
    let ev = &ctx.events;
    let mut g = ev.m.lock().unwrap();
    loop {
        if unsafe { is_set(completed) } {
            return LIBUSB_SUCCESS;
        }
        if !g.locked {
            g.locked = true;
            break;
        }
        // Another thread is handling events; wait for it to finish a cycle.
        let cycles = g.cycles;
        while g.locked && g.cycles == cycles {
            match ev.wait(g, deadline) {
                Some(ng) => g = ng,
                None => return LIBUSB_SUCCESS,
            }
            if unsafe { is_set(completed) } {
                return LIBUSB_SUCCESS;
            }
        }
    }
    let g = unsafe { ev.run_cycle(g, deadline, completed) };
    ev.release(g);
    LIBUSB_SUCCESS
}

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_handle_events_timeout_completed(
    ctx: *mut libusb_context,
    tv: *mut libc::timeval,
    completed: *mut c_int,
) -> c_int {
    if let Some(e) = crate::faults::check("handle_events_timeout_completed") {
        return e;
    }
    let Some(c) = (unsafe { ctx_ref(ctx) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    unsafe { handle_events(c, timeout_of(tv), completed) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_handle_events_timeout(
    ctx: *mut libusb_context,
    tv: *mut libc::timeval,
) -> c_int {
    unsafe { libusb_handle_events_timeout_completed(ctx, tv, ptr::null_mut()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_handle_events_completed(
    ctx: *mut libusb_context,
    completed: *mut c_int,
) -> c_int {
    if let Some(e) = crate::faults::check("handle_events_completed") {
        return e;
    }
    let Some(c) = (unsafe { ctx_ref(ctx) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    unsafe { handle_events(c, Some(Instant::now() + DEFAULT_TIMEOUT), completed) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_handle_events(ctx: *mut libusb_context) -> c_int {
    unsafe { libusb_handle_events_completed(ctx, ptr::null_mut()) }
}

/// The caller already holds the events lock (`libusb_lock_events`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_handle_events_locked(
    ctx: *mut libusb_context,
    tv: *mut libc::timeval,
) -> c_int {
    let Some(c) = (unsafe { ctx_ref(ctx) }) else {
        return LIBUSB_ERROR_INVALID_PARAM;
    };
    let g = c.events.m.lock().unwrap();
    drop(unsafe { c.events.run_cycle(g, timeout_of(tv), ptr::null()) });
    LIBUSB_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_try_lock_events(ctx: *mut libusb_context) -> c_int {
    let Some(c) = (unsafe { ctx_ref(ctx) }) else {
        return 1;
    };
    let mut g = c.events.m.lock().unwrap();
    if g.locked {
        return 1;
    }
    g.locked = true;
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_lock_events(ctx: *mut libusb_context) {
    let Some(c) = (unsafe { ctx_ref(ctx) }) else {
        return;
    };
    let mut g = c.events.m.lock().unwrap();
    while g.locked {
        g = c.events.cv.wait(g).unwrap();
    }
    g.locked = true;
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_unlock_events(ctx: *mut libusb_context) {
    let Some(c) = (unsafe { ctx_ref(ctx) }) else {
        return;
    };
    let g = c.events.m.lock().unwrap();
    c.events.release(g);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_event_handling_ok(_ctx: *mut libusb_context) -> c_int {
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_event_handler_active(ctx: *mut libusb_context) -> c_int {
    unsafe { ctx_ref(ctx) }.map_or(0, |c| c.events.m.lock().unwrap().locked as c_int)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_interrupt_event_handler(ctx: *mut libusb_context) {
    if let Some(c) = unsafe { ctx_ref(ctx) } {
        c.events.interrupt();
    }
}

// The waiters lock only guards libusb_wait_for_event; our condvar doesn't need it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_lock_event_waiters(_ctx: *mut libusb_context) {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_unlock_event_waiters(_ctx: *mut libusb_context) {}

/// Waits for the event handler to finish a cycle. Returns 1 on timeout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_wait_for_event(
    ctx: *mut libusb_context,
    tv: *mut libc::timeval,
) -> c_int {
    let Some(c) = (unsafe { ctx_ref(ctx) }) else {
        return 1;
    };
    let deadline = unsafe { timeout_of(tv) };
    let mut g = c.events.m.lock().unwrap();
    let cycles = g.cycles;
    while g.cycles == cycles {
        match c.events.wait(g, deadline) {
            Some(ng) => g = ng,
            None => return 1,
        }
    }
    0
}

/// Timeouts are enforced by the emulated host controller, never by the app.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_get_next_timeout(
    _ctx: *mut libusb_context,
    _tv: *mut libc::timeval,
) -> c_int {
    0
}

// ---------------------------------------------------------------------------
// Synchronous transfers

unsafe extern "C" fn sync_callback(t: *mut libusb_transfer) {
    unsafe { *((*t).user_data as *mut c_int) = 1 };
}

/// libusb's do_sync_transfer: async transfer + handle events until done.
unsafe fn sync_transfer(
    handle: *mut libusb_device_handle,
    endpoint: u8,
    type_: u8,
    buffer: *mut c_uchar,
    length: c_int,
    timeout: c_uint,
) -> Result<(c_int, c_int), c_int> {
    unsafe {
        let Some(h) = handle_ref(handle) else {
            return Err(LIBUSB_ERROR_INVALID_PARAM);
        };
        let t = libusb_alloc_transfer(0);
        if t.is_null() {
            return Err(LIBUSB_ERROR_NO_MEM);
        }
        let mut completed: c_int = 0;
        (*t).dev_handle = handle;
        (*t).endpoint = endpoint;
        (*t).type_ = type_;
        (*t).timeout = timeout;
        (*t).buffer = buffer;
        (*t).length = length;
        (*t).callback = Some(sync_callback);
        (*t).user_data = &raw mut completed as *mut c_void;
        let r = libusb_submit_transfer(t);
        if r < 0 {
            libusb_free_transfer(t);
            return Err(r);
        }
        let ctx = h.ctx as *mut libusb_context;
        while ptr::read_volatile(&completed) == 0 {
            let r = libusb_handle_events_completed(ctx, &raw mut completed);
            if r < 0 && r != LIBUSB_ERROR_INTERRUPTED {
                libusb_cancel_transfer(t);
            }
        }
        let res = ((*t).status, (*t).actual_length);
        libusb_free_transfer(t);
        Ok(res)
    }
}

fn sync_result(status: c_int) -> c_int {
    match status {
        LIBUSB_TRANSFER_COMPLETED => LIBUSB_SUCCESS,
        LIBUSB_TRANSFER_TIMED_OUT => LIBUSB_ERROR_TIMEOUT,
        LIBUSB_TRANSFER_STALL => LIBUSB_ERROR_PIPE,
        LIBUSB_TRANSFER_NO_DEVICE => LIBUSB_ERROR_NO_DEVICE,
        LIBUSB_TRANSFER_OVERFLOW => LIBUSB_ERROR_OVERFLOW,
        _ => LIBUSB_ERROR_IO,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_control_transfer(
    handle: *mut libusb_device_handle,
    request_type: u8,
    b_request: u8,
    w_value: u16,
    w_index: u16,
    data: *mut c_uchar,
    w_length: u16,
    timeout: c_uint,
) -> c_int {
    if let Some(e) = crate::faults::check("control_transfer") {
        return e;
    }
    let n = w_length as usize;
    let mut buf = vec![0u8; 8 + n];
    buf[..8].copy_from_slice(
        &SetupPacket::new(request_type, b_request, w_value, w_index, w_length).to_bytes(),
    );
    if request_type & 0x80 == 0 && n > 0 && !data.is_null() {
        unsafe { ptr::copy_nonoverlapping(data, buf.as_mut_ptr().add(8), n) };
    }
    let r = unsafe {
        sync_transfer(
            handle,
            0,
            LIBUSB_TRANSFER_TYPE_CONTROL,
            buf.as_mut_ptr(),
            buf.len() as c_int,
            timeout,
        )
    };
    match r {
        Err(e) => e,
        Ok((LIBUSB_TRANSFER_COMPLETED, actual)) => {
            if request_type & 0x80 != 0 && actual > 0 && !data.is_null() {
                unsafe { ptr::copy_nonoverlapping(buf.as_ptr().add(8), data, actual as usize) };
            }
            actual
        }
        Ok((status, _)) => sync_result(status),
    }
}

unsafe fn sync_data(
    handle: *mut libusb_device_handle,
    endpoint: u8,
    type_: u8,
    data: *mut c_uchar,
    length: c_int,
    transferred: *mut c_int,
    timeout: c_uint,
) -> c_int {
    match unsafe { sync_transfer(handle, endpoint, type_, data, length, timeout) } {
        Err(e) => e,
        Ok((status, actual)) => {
            if !transferred.is_null() {
                unsafe { *transferred = actual };
            }
            sync_result(status)
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_bulk_transfer(
    handle: *mut libusb_device_handle,
    endpoint: c_uchar,
    data: *mut c_uchar,
    length: c_int,
    transferred: *mut c_int,
    timeout: c_uint,
) -> c_int {
    if let Some(e) = crate::faults::check("bulk_transfer") {
        return e;
    }
    unsafe {
        sync_data(
            handle,
            endpoint,
            LIBUSB_TRANSFER_TYPE_BULK,
            data,
            length,
            transferred,
            timeout,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn libusb_interrupt_transfer(
    handle: *mut libusb_device_handle,
    endpoint: c_uchar,
    data: *mut c_uchar,
    length: c_int,
    transferred: *mut c_int,
    timeout: c_uint,
) -> c_int {
    if let Some(e) = crate::faults::check("interrupt_transfer") {
        return e;
    }
    unsafe {
        sync_data(
            handle,
            endpoint,
            LIBUSB_TRANSFER_TYPE_INTERRUPT,
            data,
            length,
            transferred,
            timeout,
        )
    }
}
