//! Virtual host controller.
//!
//! [`Host`] owns the emulated devices of one virtual bus and runs them on a
//! dedicated engine thread. Transports (libusb shim, USB/IP server, tests)
//! submit URBs and get completions through callbacks — the same model as
//! usbfs, USB/IP and libusb's async API, so each transport is a thin adapter.
//!
//! Time: devices only see bus time ([`BusTime`], microframes). In
//! [`ClockMode::Realtime`] bus time follows the wall clock, so isochronous
//! URBs complete at the pace real hardware would. In [`ClockMode::Virtual`]
//! the engine jumps straight to the next deadline whenever it is idle,
//! running streams as fast as the host consumes them (deterministic as long
//! as the host keeps URBs queued).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle, ThreadId};
use std::time::{Duration, Instant};

use log::{debug, trace, warn};
use serde::{Deserialize, Serialize};

use crate::desc::{self, ConfigView, DeviceDescriptor, dtype};
use crate::device::{
    BusTime, Ctx, DeviceEvent, EndpointInfo, InReply, OutReply, UFRAME_NS, UsbDevice, ns_to_uframes,
};
use crate::rng::Rng;
use crate::status::UsbError;
use crate::types::{Direction, SetupPacket, Speed, TransferType, std_request};

pub type DeviceId = u32;
pub type UrbId = u64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClockMode {
    #[default]
    Realtime,
    Virtual,
}

#[derive(Clone, Debug)]
pub struct HostConfig {
    pub clock: ClockMode,
    /// Seed for the per-device random generators (fault probabilities etc.).
    pub seed: u64,
    pub bus_number: u8,
    /// Virtual clock: how long to wait for the host to submit more work before
    /// jumping to a deadline that isn't an isochronous URB completion.
    pub virtual_grace: Duration,
}

impl Default for HostConfig {
    fn default() -> Self {
        HostConfig {
            clock: ClockMode::Realtime,
            seed: 0,
            bus_number: 1,
            virtual_grace: Duration::from_millis(2),
        }
    }
}

/// What the host learnt about a device when enumerating it — the equivalent
/// of the kernel's cached descriptors in sysfs.
#[derive(Debug)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub name: String,
    pub bus_number: u8,
    pub address: u8,
    /// Root hub port (1-based).
    pub port: u8,
    pub speed: Speed,
    pub device_descriptor: Vec<u8>,
    /// Configuration descriptors as read during enumeration (may be truncated).
    pub configs: Vec<Vec<u8>>,
    pub bos: Option<Vec<u8>>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
    live: Mutex<Live>,
}

#[derive(Debug, Default)]
struct Live {
    connected: bool,
    active_config: u8,
    endpoints: BTreeMap<u8, EndpointInfo>,
}

impl DeviceInfo {
    pub fn descriptor(&self) -> Option<DeviceDescriptor> {
        DeviceDescriptor::parse(&self.device_descriptor)
    }

    pub fn connected(&self) -> bool {
        self.live.lock().unwrap().connected
    }

    /// bConfigurationValue of the active configuration (0 = unconfigured).
    pub fn active_config(&self) -> u8 {
        self.live.lock().unwrap().active_config
    }

    /// Endpoint of the current configuration / alternate settings.
    pub fn endpoint(&self, address: u8) -> Option<EndpointInfo> {
        self.live.lock().unwrap().endpoints.get(&address).copied()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrbKind {
    Control(SetupPacket),
    Bulk,
    Interrupt,
    /// Isochronous, with the requested length of each packet.
    Iso(Vec<usize>),
}

/// A transfer request.
#[derive(Clone, Debug)]
pub struct Urb {
    pub device: DeviceId,
    /// Endpoint address including the direction bit (0x00/0x80 for control).
    pub endpoint: u8,
    pub kind: UrbKind,
    /// OUT payload (control OUT data stage, bulk/interrupt/iso OUT data).
    pub data: Vec<u8>,
    /// IN capacity in bytes (control IN: wLength; iso: sum of packet lengths).
    pub length: usize,
    pub timeout: Option<Duration>,
    /// Fail IN transfers shorter than `length` with [`UsbError::ShortPacket`].
    pub short_not_ok: bool,
}

impl Urb {
    pub fn control(device: DeviceId, setup: SetupPacket, data: Vec<u8>) -> Self {
        let ep = if setup.direction() == Direction::In {
            0x80
        } else {
            0
        };
        let length = setup.length as usize;
        Urb {
            device,
            endpoint: ep,
            kind: UrbKind::Control(setup),
            data,
            length,
            timeout: None,
            short_not_ok: false,
        }
    }

    pub fn bulk_in(device: DeviceId, endpoint: u8, length: usize) -> Self {
        Urb {
            device,
            endpoint,
            kind: UrbKind::Bulk,
            data: Vec::new(),
            length,
            timeout: None,
            short_not_ok: false,
        }
    }

    pub fn interrupt_in(device: DeviceId, endpoint: u8, length: usize) -> Self {
        Urb {
            kind: UrbKind::Interrupt,
            ..Self::bulk_in(device, endpoint, length)
        }
    }

    pub fn iso_in(device: DeviceId, endpoint: u8, packets: Vec<usize>) -> Self {
        let length = packets.iter().sum();
        Urb {
            device,
            endpoint,
            kind: UrbKind::Iso(packets),
            data: Vec::new(),
            length,
            timeout: None,
            short_not_ok: false,
        }
    }

    pub fn with_timeout(mut self, t: Option<Duration>) -> Self {
        self.timeout = t;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoPacketResult {
    pub status: Result<(), UsbError>,
    /// IN data (OUT: empty).
    pub data: Vec<u8>,
    pub actual_length: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub id: UrbId,
    /// Overall status. Isochronous URBs complete `Ok` with per-packet errors.
    pub status: Result<(), UsbError>,
    /// IN data for control/bulk/interrupt; OUT: empty.
    pub data: Vec<u8>,
    /// Bytes transferred (OUT: bytes accepted).
    pub actual_length: usize,
    pub iso: Vec<IsoPacketResult>,
}

/// Completion callback. Runs on the engine thread: it must not block, and
/// must not call blocking `Host` methods (`attach`, `control`, `reset_device`).
pub type CompletionFn = Box<dyn FnOnce(Completion) + Send>;

enum Cmd {
    Attach(
        Box<dyn UsbDevice>,
        mpsc::Sender<Result<Arc<DeviceInfo>, UsbError>>,
    ),
    Detach(DeviceId),
    Reset(DeviceId, mpsc::Sender<Result<(), UsbError>>),
    Submit(UrbId, Urb, CompletionFn),
    Cancel(UrbId),
    Shutdown,
}

struct Shared {
    tx: Mutex<Sender<Cmd>>,
    devices: RwLock<BTreeMap<DeviceId, Arc<DeviceInfo>>>,
    next_urb: AtomicU64,
    now: AtomicU64,
    engine_thread: Mutex<Option<ThreadId>>,
}

/// Handle to a virtual bus. See the module documentation.
pub struct Host {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Host {
    pub fn new(cfg: HostConfig) -> Host {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            tx: Mutex::new(tx),
            devices: RwLock::new(BTreeMap::new()),
            next_urb: AtomicU64::new(1),
            now: AtomicU64::new(0),
            engine_thread: Mutex::new(None),
        });
        let engine = Engine::new(cfg, shared.clone(), rx);
        let thread = thread::Builder::new()
            .name("emuvc-host".into())
            .spawn(move || engine.run())
            .expect("spawn host engine");
        *shared.engine_thread.lock().unwrap() = Some(thread.thread().id());
        Host {
            shared,
            thread: Some(thread),
        }
    }

    fn send(&self, cmd: Cmd) {
        let _ = self.shared.tx.lock().unwrap().send(cmd);
    }

    fn assert_not_engine(&self) {
        debug_assert_ne!(
            *self.shared.engine_thread.lock().unwrap(),
            Some(thread::current().id()),
            "blocking Host call from a completion callback"
        );
    }

    /// Plugs a device in and enumerates it (blocking).
    pub fn attach(&self, dev: Box<dyn UsbDevice>) -> Result<Arc<DeviceInfo>, UsbError> {
        self.assert_not_engine();
        let (tx, rx) = mpsc::channel();
        self.send(Cmd::Attach(dev, tx));
        rx.recv().unwrap_or(Err(UsbError::NoDevice))
    }

    /// Unplugs a device: pending URBs complete with [`UsbError::NoDevice`].
    pub fn detach(&self, id: DeviceId) {
        self.send(Cmd::Detach(id));
    }

    /// Port reset and re-enumeration (blocking). Pending URBs are cancelled.
    pub fn reset_device(&self, id: DeviceId) -> Result<(), UsbError> {
        self.assert_not_engine();
        let (tx, rx) = mpsc::channel();
        self.send(Cmd::Reset(id, tx));
        rx.recv().unwrap_or(Err(UsbError::NoDevice))
    }

    /// Connected devices.
    pub fn devices(&self) -> Vec<Arc<DeviceInfo>> {
        self.shared
            .devices
            .read()
            .unwrap()
            .values()
            .filter(|d| d.connected())
            .cloned()
            .collect()
    }

    pub fn device(&self, id: DeviceId) -> Option<Arc<DeviceInfo>> {
        self.shared.devices.read().unwrap().get(&id).cloned()
    }

    /// Current bus time.
    pub fn now(&self) -> BusTime {
        self.shared.now.load(Ordering::Relaxed)
    }

    /// Queues a URB. Obvious errors (unknown device/endpoint) are reported
    /// synchronously; everything else through `done`.
    pub fn submit(&self, urb: Urb, done: CompletionFn) -> Result<UrbId, UsbError> {
        let info = self.device(urb.device).ok_or(UsbError::NoDevice)?;
        if !info.connected() {
            return Err(UsbError::NoDevice);
        }
        validate(&info, &urb)?;
        let id = self.shared.next_urb.fetch_add(1, Ordering::Relaxed);
        self.send(Cmd::Submit(id, urb, done));
        Ok(id)
    }

    /// Requests cancellation; the URB then completes with
    /// [`UsbError::Cancelled`] unless it already completed.
    pub fn cancel(&self, id: UrbId) {
        self.send(Cmd::Cancel(id));
    }

    /// Submits and waits (blocking).
    pub fn transfer(&self, urb: Urb) -> Completion {
        self.assert_not_engine();
        let (tx, rx) = mpsc::channel();
        match self.submit(urb, Box::new(move |c| drop(tx.send(c)))) {
            Ok(_) => rx.recv().unwrap(),
            Err(e) => Completion {
                id: 0,
                status: Err(e),
                data: Vec::new(),
                actual_length: 0,
                iso: Vec::new(),
            },
        }
    }

    /// Synchronous control transfer helper.
    pub fn control(
        &self,
        device: DeviceId,
        setup: SetupPacket,
        data: &[u8],
        timeout: Option<Duration>,
    ) -> Result<Vec<u8>, UsbError> {
        let c = self.transfer(Urb::control(device, setup, data.to_vec()).with_timeout(timeout));
        c.status.map(|_| c.data)
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.send(Cmd::Shutdown);
        if let Some(t) = self.thread.take()
            && t.thread().id() != thread::current().id()
        {
            let _ = t.join();
        }
    }
}

fn validate(info: &DeviceInfo, urb: &Urb) -> Result<(), UsbError> {
    if urb.endpoint & 0x7f == 0 {
        return match urb.kind {
            UrbKind::Control(_) => Ok(()),
            _ => Err(UsbError::Invalid),
        };
    }
    let ep = info.endpoint(urb.endpoint).ok_or(UsbError::Invalid)?;
    let ok = matches!(
        (&urb.kind, ep.transfer_type),
        (UrbKind::Bulk, TransferType::Bulk)
            | (UrbKind::Interrupt, TransferType::Interrupt)
            | (UrbKind::Iso(_), TransferType::Isochronous)
    );
    match &urb.kind {
        UrbKind::Iso(p) if p.is_empty() => Err(UsbError::Invalid),
        _ if ok => Ok(()),
        _ => Err(UsbError::Invalid),
    }
}

// ---------------------------------------------------------------------------
// Engine

struct Pending {
    id: UrbId,
    urb: Urb,
    done: CompletionFn,
    deadline: Option<BusTime>,
    /// Isochronous: slot of packet 0, and results so far.
    iso_start: BusTime,
    iso: Vec<IsoPacketResult>,
}

#[derive(Default)]
struct EpQueue {
    urbs: VecDeque<Pending>,
    /// Isochronous: first free service interval after the queued URBs.
    next_slot: BusTime,
    /// Bulk/interrupt: when to try the head URB again.
    next_poll: BusTime,
    /// The device NAKed the last attempt (polls then don't drive virtual time).
    nak: bool,
}

struct Dev {
    info: Arc<DeviceInfo>,
    dev: Box<dyn UsbDevice>,
    rng: Rng,
    events: Vec<DeviceEvent>,
    ep0: VecDeque<Pending>,
    /// Reply for the head of `ep0`, available at the given time.
    ep0_reply: Option<(BusTime, Result<Vec<u8>, UsbError>)>,
    eps: BTreeMap<u8, EpQueue>,
    /// Last time `tick` ran, to consume wakeups.
    last_tick: Option<BusTime>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    IsoPacket(DeviceId, u8),
    ControlDone(DeviceId),
    Timeout(DeviceId, u8, UrbId),
    Wakeup(DeviceId),
    Poll(DeviceId, u8),
}

struct Engine {
    cfg: HostConfig,
    shared: Arc<Shared>,
    rx: Receiver<Cmd>,
    start: Instant,
    now: BusTime,
    devs: BTreeMap<DeviceId, Dev>,
    urbs: HashMap<UrbId, (DeviceId, u8)>,
    next_dev: DeviceId,
    done: Vec<(CompletionFn, Completion)>,
}

fn completion(id: UrbId, status: Result<(), UsbError>) -> Completion {
    Completion {
        id,
        status,
        data: Vec::new(),
        actual_length: 0,
        iso: Vec::new(),
    }
}

impl Engine {
    fn new(cfg: HostConfig, shared: Arc<Shared>, rx: Receiver<Cmd>) -> Self {
        Engine {
            cfg,
            shared,
            rx,
            start: Instant::now(),
            now: 0,
            devs: BTreeMap::new(),
            urbs: HashMap::new(),
            next_dev: 1,
            done: Vec::new(),
        }
    }

    fn realtime(&self) -> bool {
        self.cfg.clock == ClockMode::Realtime
    }

    fn wall_now(&self) -> BusTime {
        (self.start.elapsed().as_nanos() / UFRAME_NS as u128) as BusTime
    }

    fn run(mut self) {
        loop {
            if self.realtime() {
                self.advance_to(self.wall_now());
            }
            loop {
                match self.rx.try_recv() {
                    Ok(cmd) => {
                        if !self.command(cmd) {
                            return;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }
            self.deliver();
            let next = self.next_deadline();
            let cmd = if self.realtime() {
                let wait = match next {
                    Some((t, _)) => {
                        Duration::from_nanos(t.saturating_sub(self.wall_now()) * UFRAME_NS)
                    }
                    None => Duration::from_millis(50),
                };
                self.rx.recv_timeout(wait.min(Duration::from_millis(50)))
            } else {
                match next {
                    // Iso URBs already queued fix the future: jump right away.
                    Some((t, true)) => {
                        self.advance_to(t);
                        continue;
                    }
                    Some((t, false)) => match self.rx.recv_timeout(self.cfg.virtual_grace) {
                        Err(RecvTimeoutError::Timeout) => {
                            self.advance_to(t);
                            continue;
                        }
                        r => r,
                    },
                    None => self.rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                }
            };
            match cmd {
                Ok(cmd) => {
                    if self.realtime() {
                        self.advance_to(self.wall_now());
                    }
                    if !self.command(cmd) {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn deliver(&mut self) {
        for (f, c) in self.done.drain(..) {
            f(c);
        }
    }

    fn finish(&mut self, p: Pending, c: Completion) {
        trace!(
            "urb {} done: {:?} ({} bytes)",
            p.id, c.status, c.actual_length
        );
        self.urbs.remove(&p.id);
        self.done.push((p.done, c));
    }

    // -- commands -----------------------------------------------------------

    fn command(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::Attach(dev, reply) => {
                let r = self.attach(dev);
                let _ = reply.send(r);
            }
            Cmd::Detach(id) => self.detach(id),
            Cmd::Reset(id, reply) => {
                let r = self.reset(id);
                let _ = reply.send(r);
            }
            Cmd::Submit(id, urb, done) => self.submit(id, urb, done),
            Cmd::Cancel(id) => self.cancel(id),
            Cmd::Shutdown => {
                for id in self.devs.keys().copied().collect::<Vec<_>>() {
                    self.detach(id);
                }
                self.deliver();
                return false;
            }
        }
        true
    }

    fn attach(&mut self, dev: Box<dyn UsbDevice>) -> Result<Arc<DeviceInfo>, UsbError> {
        let id = self.next_dev;
        self.next_dev += 1;
        let address = (id % 127) as u8 + 1;
        let mut d = Dev {
            info: Arc::new(placeholder_info(id)),
            dev,
            rng: Rng::new(self.cfg.seed ^ (id as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)),
            events: Vec::new(),
            ep0: VecDeque::new(),
            ep0_reply: None,
            eps: BTreeMap::new(),
            last_tick: None,
        };
        let info = enumerate(&mut d, self.now, id, address, self.cfg.bus_number)?;
        debug!(
            "attached {} as {:03}:{:03} (id {id})",
            info.name, info.bus_number, info.address
        );
        d.info = Arc::new(info);
        refresh_live(&mut d);
        let info = d.info.clone();
        self.shared
            .devices
            .write()
            .unwrap()
            .insert(id, info.clone());
        self.devs.insert(id, d);
        Ok(info)
    }

    fn detach(&mut self, id: DeviceId) {
        let Some(mut d) = self.devs.remove(&id) else {
            return;
        };
        debug!("detached {}", d.info.name);
        d.info.live.lock().unwrap().connected = false;
        let mut all: Vec<Pending> = d.ep0.drain(..).collect();
        for q in d.eps.values_mut() {
            all.extend(q.urbs.drain(..));
        }
        for p in all {
            let c = completion(p.id, Err(UsbError::NoDevice));
            self.finish(p, c);
        }
        self.shared.devices.write().unwrap().remove(&id);
    }

    fn reset(&mut self, id: DeviceId) -> Result<(), UsbError> {
        let now = self.now;
        let d = self.devs.get_mut(&id).ok_or(UsbError::NoDevice)?;
        let mut all: Vec<Pending> = d.ep0.drain(..).collect();
        d.ep0_reply = None;
        for q in d.eps.values_mut() {
            all.extend(q.urbs.drain(..));
        }
        let (bus, address) = (d.info.bus_number, d.info.address);
        let fresh = enumerate(d, now, id, address, bus);
        let r = match fresh {
            Ok(info)
                if info.device_descriptor == d.info.device_descriptor
                    && info.configs == d.info.configs =>
            {
                d.info.live.lock().unwrap().active_config = info.active_config();
                refresh_live(d);
                Ok(())
            }
            Ok(_) => Err(UsbError::NoDevice), // descriptors changed: libusb reports NOT_FOUND
            Err(e) => Err(e),
        };
        for p in all {
            let c = completion(p.id, Err(UsbError::Cancelled));
            self.finish(p, c);
        }
        if r.is_err() {
            self.detach(id);
        }
        r
    }

    fn submit(&mut self, id: UrbId, urb: Urb, done: CompletionFn) {
        let now = self.now;
        let Some(d) = self.devs.get_mut(&urb.device) else {
            self.done
                .push((done, completion(id, Err(UsbError::NoDevice))));
            return;
        };
        if let Err(e) = validate(&d.info, &urb) {
            self.done.push((done, completion(id, Err(e))));
            return;
        }
        trace!(
            "urb {id} submit {:?} ep {:02x} len {}",
            urb.kind, urb.endpoint, urb.length
        );
        let deadline = urb
            .timeout
            .filter(|t| !t.is_zero())
            .map(|t| now + ns_to_uframes(t.as_nanos() as u64));
        let mut p = Pending {
            id,
            urb,
            done,
            deadline,
            iso_start: 0,
            iso: Vec::new(),
        };
        let ep = p.urb.endpoint;
        self.urbs.insert(
            id,
            (
                p.urb.device,
                if matches!(p.urb.kind, UrbKind::Control(_)) {
                    0
                } else {
                    ep
                },
            ),
        );
        let dev_id = p.urb.device;
        match &p.urb.kind {
            UrbKind::Control(_) => {
                d.ep0.push_back(p);
                if d.ep0.len() == 1 {
                    self.start_control(dev_id);
                }
            }
            UrbKind::Iso(packets) => {
                let info = d.info.endpoint(ep).unwrap();
                let interval = info.interval.max(1) as BusTime;
                let q = d.eps.entry(ep).or_default();
                let earliest = (now + 1).div_ceil(interval) * interval;
                p.iso_start = q.next_slot.max(earliest);
                q.next_slot = p.iso_start + packets.len() as BusTime * interval;
                q.urbs.push_back(p);
            }
            UrbKind::Bulk | UrbKind::Interrupt => {
                let q = d.eps.entry(ep).or_default();
                q.urbs.push_back(p);
                if q.urbs.len() == 1 {
                    q.next_poll = q.next_poll.max(now);
                    q.nak = false;
                    if q.next_poll <= now {
                        self.poll(dev_id, ep);
                    }
                }
            }
        }
    }

    fn cancel(&mut self, id: UrbId) {
        let Some(&(dev_id, ep)) = self.urbs.get(&id) else {
            return;
        };
        let Some(d) = self.devs.get_mut(&dev_id) else {
            return;
        };
        let (p, was_head) = if ep == 0 {
            let Some(pos) = d.ep0.iter().position(|p| p.id == id) else {
                return;
            };
            if pos == 0 {
                d.ep0_reply = None;
            }
            (d.ep0.remove(pos).unwrap(), pos == 0)
        } else {
            let Some(q) = d.eps.get_mut(&ep) else { return };
            let Some(pos) = q.urbs.iter().position(|p| p.id == id) else {
                return;
            };
            (q.urbs.remove(pos).unwrap(), pos == 0)
        };
        let mut c = completion(id, Err(UsbError::Cancelled));
        if let UrbKind::Iso(packets) = &p.urb.kind {
            c.iso = p.iso.clone();
            c.iso.resize(
                packets.len(),
                IsoPacketResult {
                    status: Err(UsbError::Cancelled),
                    data: Vec::new(),
                    actual_length: 0,
                },
            );
        }
        self.finish(p, c);
        if ep == 0 && was_head {
            self.start_control(dev_id);
        }
    }

    // -- control transfers --------------------------------------------------

    /// Runs the device side of the control transfer at the head of EP0's queue.
    fn start_control(&mut self, dev_id: DeviceId) {
        let now = self.now;
        let Some(d) = self.devs.get_mut(&dev_id) else {
            return;
        };
        if d.ep0_reply.is_some() {
            return;
        }
        let Some(p) = d.ep0.front() else { return };
        let UrbKind::Control(setup) = p.urb.kind else {
            unreachable!()
        };
        let out: &[u8] = if setup.direction() == Direction::Out {
            &p.urb.data
        } else {
            &[]
        };
        let mut cx = Ctx::new(now, &mut d.rng, &mut d.events);
        let reply = d.dev.control(&mut cx, &setup, out);
        debug!(
            "{}: {:?} -> {:?}{}",
            d.info.name,
            setup,
            reply.result.as_ref().map(|v| HexBytes(v)),
            if reply.latency_us > 0 {
                format!(" after {}us", reply.latency_us)
            } else {
                String::new()
            }
        );
        if setup.request_type == 0x00
            && setup.request == std_request::SET_CONFIGURATION
            && reply.result.is_ok()
        {
            d.info.live.lock().unwrap().active_config = setup.value as u8;
        }
        let ready = now + ns_to_uframes(reply.latency_us as u64 * 1000);
        d.ep0_reply = Some((ready, reply.result));
        let std_ctl = setup.request_type & 0x60 == 0
            && matches!(
                setup.request,
                std_request::SET_INTERFACE
                    | std_request::SET_CONFIGURATION
                    | std_request::CLEAR_FEATURE
            );
        if std_ctl {
            self.endpoints_changed(dev_id);
        }
        self.after_device_call(dev_id, true);
    }

    fn finish_control(&mut self, dev_id: DeviceId) {
        let Some(d) = self.devs.get_mut(&dev_id) else {
            return;
        };
        let Some((_, result)) = d.ep0_reply.take() else {
            return;
        };
        let Some(p) = d.ep0.pop_front() else { return };
        let c = match result {
            // A device that never answers only completes through the URB timeout.
            Err(UsbError::Timeout) => {
                d.ep0.push_front(p);
                d.ep0_reply = Some((BusTime::MAX, Err(UsbError::Timeout)));
                return;
            }
            Ok(mut data) => {
                if p.urb.endpoint & 0x80 != 0 {
                    data.truncate(p.urb.length);
                    let n = data.len();
                    let status = if p.urb.short_not_ok && n < p.urb.length {
                        Err(UsbError::ShortPacket)
                    } else {
                        Ok(())
                    };
                    Completion {
                        id: p.id,
                        status,
                        data,
                        actual_length: n,
                        iso: Vec::new(),
                    }
                } else {
                    Completion {
                        id: p.id,
                        status: Ok(()),
                        data: Vec::new(),
                        actual_length: p.urb.data.len(),
                        iso: Vec::new(),
                    }
                }
            }
            Err(e) => completion(p.id, Err(e)),
        };
        self.finish(p, c);
        self.start_control(dev_id);
    }

    /// The current alternate settings may have changed: refresh what the
    /// host sees and flush URBs queued on endpoints that went away.
    fn endpoints_changed(&mut self, dev_id: DeviceId) {
        let Some(d) = self.devs.get_mut(&dev_id) else {
            return;
        };
        refresh_live(d);
        let mut flushed = Vec::new();
        for (&ep, q) in d.eps.iter_mut() {
            if d.dev.endpoint(ep).is_none() {
                flushed.extend(q.urbs.drain(..));
                q.next_slot = 0;
            }
        }
        for p in flushed {
            let mut c = completion(p.id, Err(UsbError::Shutdown));
            if let UrbKind::Iso(packets) = &p.urb.kind {
                c.iso = p.iso.clone();
                c.iso.resize(
                    packets.len(),
                    IsoPacketResult {
                        status: Err(UsbError::Shutdown),
                        data: Vec::new(),
                        actual_length: 0,
                    },
                );
            }
            self.finish(p, c);
        }
    }

    /// After a device call: handle its events and, if the call may have
    /// changed device state (`kick`), let NAKed transfers retry right away.
    fn after_device_call(&mut self, dev_id: DeviceId, kick: bool) {
        let now = self.now;
        let Some(d) = self.devs.get_mut(&dev_id) else {
            return;
        };
        let events = std::mem::take(&mut d.events);
        for q in d.eps.values_mut().filter(|q| kick && q.nak) {
            q.nak = false;
            q.next_poll = q.next_poll.min(now);
        }
        if events.contains(&DeviceEvent::Disconnect) {
            warn!("{} disconnected itself", d.info.name);
            self.detach(dev_id);
        }
    }

    // -- time ---------------------------------------------------------------

    /// Earliest pending event, if at or before `limit`.
    fn next_event(&self, limit: BusTime) -> Option<(BusTime, Event)> {
        let mut best: Option<(BusTime, Event)> = None;
        let mut consider = |t: BusTime, e: Event| {
            if t <= limit && best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, e));
            }
        };
        for (&id, d) in &self.devs {
            if let Some((t, _)) = d.ep0_reply {
                consider(t, Event::ControlDone(id));
            }
            if let Some(p) = d.ep0.front()
                && let Some(dl) = p.deadline
            {
                consider(dl, Event::Timeout(id, 0, p.id));
            }
            if let Some(t) = d.dev.next_wakeup()
                && d.last_tick.is_none_or(|lt| t > lt)
            {
                consider(t.max(self.now), Event::Wakeup(id));
            }
            for (&ep, q) in &d.eps {
                let Some(head) = q.urbs.front() else { continue };
                match &head.urb.kind {
                    UrbKind::Iso(_) => {
                        let interval =
                            d.info.endpoint(ep).map_or(1, |e| e.interval.max(1)) as BusTime;
                        consider(
                            head.iso_start + head.iso.len() as BusTime * interval,
                            Event::IsoPacket(id, ep),
                        );
                    }
                    _ => consider(q.next_poll, Event::Poll(id, ep)),
                }
                for p in &q.urbs {
                    if let Some(dl) = p.deadline {
                        consider(dl, Event::Timeout(id, ep, p.id));
                    }
                }
            }
        }
        best
    }

    /// When the engine must run next, and whether that is an isochronous URB
    /// completion (see the virtual clock policy in `run`).
    fn next_deadline(&self) -> Option<(BusTime, bool)> {
        let mut best: Option<(BusTime, bool)> = None;
        let mut consider = |t: BusTime, iso: bool| {
            if best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, iso));
            }
        };
        for d in self.devs.values() {
            if let Some((t, _)) = d.ep0_reply
                && t != BusTime::MAX
            {
                consider(t, false);
            }
            if let Some(dl) = d.ep0.front().and_then(|p| p.deadline) {
                consider(dl, false);
            }
            if let Some(t) = d.dev.next_wakeup()
                && d.last_tick.is_none_or(|lt| t > lt)
            {
                consider(t.max(self.now), false);
            }
            for (&ep, q) in &d.eps {
                let Some(head) = q.urbs.front() else { continue };
                match &head.urb.kind {
                    UrbKind::Iso(packets) => {
                        let interval =
                            d.info.endpoint(ep).map_or(1, |e| e.interval.max(1)) as BusTime;
                        consider(
                            head.iso_start + (packets.len() as BusTime - 1) * interval,
                            true,
                        );
                    }
                    // Data waiting for queued URBs fixes the future like iso does.
                    _ if !q.nak => consider(q.next_poll.max(self.now), true),
                    _ if self.realtime() => consider(q.next_poll.max(self.now), false),
                    _ => {}
                }
                for dl in q.urbs.iter().filter_map(|p| p.deadline) {
                    consider(dl, false);
                }
            }
        }
        best
    }

    fn advance_to(&mut self, t: BusTime) {
        while let Some((et, ev)) = self.next_event(t) {
            self.now = self.now.max(et);
            match ev {
                Event::IsoPacket(id, ep) => self.iso_packet(id, ep),
                Event::ControlDone(id) => self.finish_control(id),
                Event::Timeout(id, ep, urb) => self.timeout(id, ep, urb),
                Event::Wakeup(id) => self.wakeup(id),
                Event::Poll(id, ep) => self.poll(id, ep),
            }
        }
        self.now = self.now.max(t);
        self.shared.now.store(self.now, Ordering::Relaxed);
    }

    fn timeout(&mut self, dev_id: DeviceId, ep: u8, urb: UrbId) {
        let Some(d) = self.devs.get_mut(&dev_id) else {
            return;
        };
        let p = if ep == 0 {
            if d.ep0.front().is_some_and(|p| p.id == urb) {
                d.ep0_reply = None;
                d.ep0.pop_front()
            } else {
                None
            }
        } else {
            d.eps.get_mut(&ep).and_then(|q| {
                let pos = q.urbs.iter().position(|p| p.id == urb)?;
                q.urbs.remove(pos)
            })
        };
        if let Some(p) = p {
            debug!("{}: urb {urb} timed out", d.info.name);
            let c = completion(p.id, Err(UsbError::Timeout));
            self.finish(p, c);
            if ep == 0 {
                self.start_control(dev_id);
            }
        }
    }

    fn wakeup(&mut self, dev_id: DeviceId) {
        let now = self.now;
        let Some(d) = self.devs.get_mut(&dev_id) else {
            return;
        };
        d.last_tick = Some(now);
        let mut cx = Ctx::new(now, &mut d.rng, &mut d.events);
        d.dev.tick(&mut cx);
        self.after_device_call(dev_id, true);
    }

    fn iso_packet(&mut self, dev_id: DeviceId, ep: u8) {
        let Some(d) = self.devs.get_mut(&dev_id) else {
            return;
        };
        let interval = d.info.endpoint(ep).map_or(1, |e| e.interval.max(1)) as BusTime;
        let Some(q) = d.eps.get_mut(&ep) else { return };
        let Some(p) = q.urbs.front_mut() else { return };
        let UrbKind::Iso(packets) = &p.urb.kind else {
            return;
        };
        let k = p.iso.len();
        let slot = p.iso_start + k as BusTime * interval;
        let max = packets[k];
        let mut cx = Ctx::new(slot, &mut d.rng, &mut d.events);
        let result = if ep & 0x80 != 0 {
            let r = d.dev.iso_in(&mut cx, ep, max);
            let mut data = r.data;
            let status = if data.len() > max {
                data.truncate(max);
                Err(UsbError::Overflow)
            } else {
                r.error.map_or(Ok(()), Err)
            };
            IsoPacketResult {
                status,
                actual_length: data.len(),
                data,
            }
        } else {
            let off: usize = packets[..k].iter().sum();
            let chunk = &p.urb.data[off.min(p.urb.data.len())..(off + max).min(p.urb.data.len())];
            let r = d.dev.iso_out(&mut cx, ep, chunk);
            IsoPacketResult {
                status: r.error.map_or(Ok(()), Err),
                data: Vec::new(),
                actual_length: chunk.len(),
            }
        };
        p.iso.push(result);
        if p.iso.len() == packets.len() {
            let p = q.urbs.pop_front().unwrap();
            let total = p.iso.iter().map(|r| r.actual_length).sum();
            let c = Completion {
                id: p.id,
                status: Ok(()),
                data: Vec::new(),
                actual_length: total,
                iso: p.iso.clone(),
            };
            self.finish(p, c);
        }
        self.after_device_call(dev_id, false);
    }

    /// Tries the head bulk/interrupt URB of an endpoint.
    fn poll(&mut self, dev_id: DeviceId, ep: u8) {
        let now = self.now;
        let (finished, disconnect) = {
            let Some(d) = self.devs.get_mut(&dev_id) else {
                return;
            };
            let Some(info) = d.dev.endpoint(ep) else {
                return;
            };
            let Some(q) = d.eps.get_mut(&ep) else { return };
            let Some(p) = q.urbs.front() else { return };
            let mut cx = Ctx::new(now, &mut d.rng, &mut d.events);
            let result: Option<Completion> = if ep & 0x80 != 0 {
                match d.dev.data_in(&mut cx, ep, p.urb.length) {
                    InReply::Data(mut data) => {
                        let status = if data.len() > p.urb.length {
                            data.truncate(p.urb.length);
                            Err(UsbError::Overflow)
                        } else if p.urb.short_not_ok && data.len() < p.urb.length {
                            Err(UsbError::ShortPacket)
                        } else {
                            Ok(())
                        };
                        Some(Completion {
                            id: p.id,
                            status,
                            actual_length: data.len(),
                            data,
                            iso: Vec::new(),
                        })
                    }
                    InReply::Nak => None,
                    InReply::Error(e) => Some(completion(p.id, Err(e))),
                }
            } else {
                match d.dev.data_out(&mut cx, ep, &p.urb.data) {
                    OutReply::Ack => Some(Completion {
                        id: p.id,
                        status: Ok(()),
                        data: Vec::new(),
                        actual_length: p.urb.data.len(),
                        iso: Vec::new(),
                    }),
                    OutReply::Nak => None,
                    OutReply::Error(e) => Some(completion(p.id, Err(e))),
                }
            };
            let interval = info.interval.max(1) as BusTime;
            let finished = match result {
                Some(c) => {
                    // Bus time used: interrupt endpoints move one transfer per
                    // interval; bulk is limited to ~13 packets per microframe.
                    let busy = match info.transfer_type {
                        TransferType::Interrupt => interval,
                        _ => (c.actual_length.max(1) as BusTime)
                            .div_ceil(13 * info.max_bytes.max(1) as BusTime),
                    };
                    q.next_poll = now + busy;
                    q.nak = false;
                    Some((q.urbs.pop_front().unwrap(), c))
                }
                None => {
                    q.nak = true;
                    q.next_poll = now
                        + if info.transfer_type == TransferType::Interrupt {
                            interval
                        } else {
                            8
                        };
                    None
                }
            };
            // Not `after_device_call`: that would clear the NAK we just recorded.
            let events = std::mem::take(&mut d.events);
            (finished, events.contains(&DeviceEvent::Disconnect))
        };
        if let Some((p, c)) = finished {
            self.finish(p, c);
        }
        if disconnect {
            self.detach(dev_id);
        }
    }
}

fn placeholder_info(id: DeviceId) -> DeviceInfo {
    DeviceInfo {
        id,
        name: String::new(),
        bus_number: 0,
        address: 0,
        port: 0,
        speed: Speed::Full,
        device_descriptor: Vec::new(),
        configs: Vec::new(),
        bos: None,
        manufacturer: None,
        product: None,
        serial: None,
        live: Mutex::new(Live::default()),
    }
}

/// Re-reads the device's endpoint table into the shared `DeviceInfo`.
fn refresh_live(d: &mut Dev) {
    let mut live = d.info.live.lock().unwrap();
    live.connected = true;
    live.endpoints.clear();
    let mut addrs: Vec<u8> = Vec::new();
    for c in &d.info.configs {
        if let Ok(v) = ConfigView::parse(c) {
            for i in &v.interfaces {
                for a in &i.alts {
                    addrs.extend(a.endpoints.iter().map(|e| e.desc.address));
                }
            }
        }
    }
    for a in addrs {
        if let Some(info) = d.dev.endpoint(a) {
            live.endpoints.insert(a, info);
        }
    }
}

/// Kernel-style enumeration: reads descriptors and strings, sets the address
/// and selects the first configuration.
fn enumerate(
    d: &mut Dev,
    now: BusTime,
    id: DeviceId,
    address: u8,
    bus: u8,
) -> Result<DeviceInfo, UsbError> {
    {
        let mut cx = Ctx::new(now, &mut d.rng, &mut d.events);
        d.dev.reset(&mut cx);
    }
    let (name, speed) = (d.dev.name(), d.dev.speed());
    let mut ctl = |setup: SetupPacket| -> Result<Vec<u8>, UsbError> {
        let mut cx = Ctx::new(now, &mut d.rng, &mut d.events);
        let mut r = d.dev.control(&mut cx, &setup, &[]).result;
        if let Ok(v) = &mut r {
            v.truncate(setup.length as usize);
        }
        r
    };
    let get = |ty: u8, index: u8, lang: u16, len: u16| {
        SetupPacket::new(
            0x80,
            std_request::GET_DESCRIPTOR,
            (ty as u16) << 8 | index as u16,
            lang,
            len,
        )
    };
    let first = ctl(get(dtype::DEVICE, 0, 0, 64))?;
    if first.len() < 8 {
        return Err(UsbError::Protocol);
    }
    ctl(SetupPacket::new(
        0x00,
        std_request::SET_ADDRESS,
        address as u16,
        0,
        0,
    ))?;
    let device_descriptor = ctl(get(dtype::DEVICE, 0, 0, 18))?;
    let dd = DeviceDescriptor::parse(&device_descriptor).ok_or(UsbError::Protocol)?;
    let mut configs = Vec::new();
    for i in 0..dd.num_configurations.min(8) {
        let head = ctl(get(dtype::CONFIG, i, 0, 9))?;
        if head.len() < 4 {
            return Err(UsbError::Protocol);
        }
        let total = u16::from_le_bytes([head[2], head[3]]).max(9);
        configs.push(ctl(get(dtype::CONFIG, i, 0, total))?);
    }
    let mut bos = None;
    if dd.bcd_usb >= 0x0201
        && let Ok(head) = ctl(get(dtype::BOS, 0, 0, 5))
        && head.len() >= 4
    {
        let total = u16::from_le_bytes([head[2], head[3]]).max(5);
        bos = ctl(get(dtype::BOS, 0, 0, total)).ok();
    }
    let lang = ctl(get(dtype::STRING, 0, 0, 255))
        .ok()
        .filter(|l| l.len() >= 4)
        .map_or(0x0409, |l| u16::from_le_bytes([l[2], l[3]]));
    let mut string = |i: u8| -> Option<String> {
        if i == 0 {
            return None;
        }
        ctl(get(dtype::STRING, i, lang, 255))
            .ok()
            .and_then(|s| desc::parse_string_descriptor(&s))
    };
    let (manufacturer, product, serial) = (
        string(dd.i_manufacturer),
        string(dd.i_product),
        string(dd.i_serial),
    );
    let active = configs
        .first()
        .and_then(|c| desc::ConfigHeader::parse(c))
        .map_or(0, |h| h.value);
    let active = match ctl(SetupPacket::new(
        0x00,
        std_request::SET_CONFIGURATION,
        active as u16,
        0,
        0,
    )) {
        Ok(_) => active,
        Err(e) => {
            warn!("{name}: SET_CONFIGURATION failed: {e}");
            0
        }
    };
    d.events.clear();
    Ok(DeviceInfo {
        id,
        name,
        bus_number: bus,
        address,
        port: id as u8,
        speed,
        device_descriptor,
        configs,
        bos,
        manufacturer,
        product,
        serial,
        live: Mutex::new(Live {
            connected: true,
            active_config: active,
            endpoints: BTreeMap::new(),
        }),
    })
}

struct HexBytes<'a>(&'a [u8]);

impl std::fmt::Debug for HexBytes<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, b) in self.0.iter().take(32).enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{b:02x}")?;
        }
        if self.0.len() > 32 {
            write!(f, " ... ({} bytes)", self.0.len())?;
        }
        Ok(())
    }
}
