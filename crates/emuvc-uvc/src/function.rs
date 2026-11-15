//! The UVC function: one VideoControl interface plus its VideoStreaming
//! interfaces, built from (parsed) descriptors.
//!
//! Behaviour derives from the descriptors (entities, supported controls,
//! formats, alternate settings); device models then tune control ranges,
//! extension unit sizes, frame sources and quirks.

use std::collections::{BTreeMap, VecDeque};

use log::debug;
use serde::{Deserialize, Serialize};

use emuvc_usb::{
    BusTime, ControlReply, Ctx, Function, InReply, IsoReply, SetupPacket, Speed, UsbError,
};
use emuvc_video::FrameSource;

use crate::consts::*;
use crate::control::{self, Control, Unit};
use crate::stream::{StreamQuirks, VsInterface};
use crate::topology::{EntityKind, Transfer, UvcFunctionDesc, VideoControl, VideoStreaming};

/// Builds a frame source for a streaming interface.
pub type SourceFactory<'a> = &'a mut dyn FnMut(&VideoStreaming) -> Box<dyn FrameSource>;

pub struct UvcFunction {
    name: String,
    pub vc: VideoControl,
    controls: BTreeMap<(u8, u8), Control>,
    request_error: u8,
    status: VecDeque<Vec<u8>>,
    pub vs: Vec<VsInterface>,
    events: Vec<ScheduledEvent>,
    /// Asynchronous controls: (delay, failure code) by (entity, selector).
    async_cfg: BTreeMap<(u8, u8), (u64, Option<u8>)>,
    /// Asynchronous SET_CURs not completed yet.
    pending: Vec<PendingSet>,
    /// VC_VIDEO_POWER_MODE_CONTROL's bDevicePowerMode; `None`: no such control.
    power_mode: Option<u8>,
}

/// Per-control behaviour:
///
/// ```toml
/// [[device.control]]
/// entity = 3
/// selector = 0x02      # PU_BRIGHTNESS_CONTROL
/// async_ms = 200       # asynchronous (GET_INFO D4): SET_CUR completes 200 ms later
/// fail = 0x02          # ... with a Control Failure Change ("wrong state") instead of the value
///
/// [[device.control]]   # an extension unit control: 33 bytes, read-only, no GET_RES
/// entity = 5
/// selector = 1
/// len = 33
/// info = 0x01
/// res = []
///
/// [[device.control]]   # VC_VIDEO_POWER_MODE_CONTROL: battery powered, vendor mode supported
/// entity = 0
/// selector = 1
/// cur = [0x50]
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ControlSpec {
    pub entity: u8,
    pub selector: u8,
    pub async_ms: Option<u64>,
    pub fail: Option<u8>,
    /// GET_INFO capability bits (D0 GET, D1 SET, D3 autoupdate, D4 async).
    pub info: Option<u8>,
    /// Extension unit controls: GET_LEN. The control starts over as an opaque one of that
    /// size (MIN all 0, MAX all 0xff), keeping its GET_INFO.
    pub len: Option<u16>,
    /// Extension unit controls: GET_MIN/MAX/RES/DEF answers; `[]` makes the request STALL.
    /// GET_DEF is also the value after a reset.
    pub min: Option<Vec<u8>>,
    pub max: Option<Vec<u8>>,
    pub res: Option<Vec<u8>>,
    pub def: Option<Vec<u8>>,
    /// Entity 0 selector 1 only: bDevicePowerMode, which adds the power mode control.
    pub cur: Option<Vec<u8>>,
}

struct PendingSet {
    entity: u8,
    selector: u8,
    value: Vec<u8>,
    due: BusTime,
    fail: Option<u8>,
}

/// Completion delay of asynchronous controls without `async_ms` (they take over 10 ms).
const DEFAULT_ASYNC_MS: u64 = 20;

/// A device-initiated event, e.g. from a test scenario:
///
/// ```toml
/// [[device.event]]
/// every_ms = 500        # first at at_ms (default 0), then periodically
/// kind = "button"
///
/// [[device.event]]
/// at_ms = 200
/// kind = { control = { entity = 1, selector = 0x11, value = [1] } }  # privacy shutter closed
///
/// [[device.event]]
/// kind = { control = { entity = 3, selector = 0x02, attribute = 4, value = [100, 0] } }  # new GET_MAX
///
/// [[device.event]]
/// kind = { raw = [1, 2] }   # arbitrary (here: truncated) status packet
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventSpec {
    #[serde(default)]
    pub at_ms: u64,
    #[serde(default)]
    pub every_ms: Option<u64>,
    /// Stop after this many occurrences.
    #[serde(default)]
    pub count: Option<u32>,
    pub kind: EventKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum EventKind {
    /// Still image button press then release (first streaming interface).
    Button,
    /// Control Change interrupt from the device. With `attribute` 0 (value change, the
    /// default) the control takes the new value; 1-4 (info, failure, min, max change)
    /// only report `value`.
    Control {
        entity: u8,
        selector: u8,
        value: Vec<u8>,
        #[serde(default)]
        attribute: u8,
    },
    /// An arbitrary status interrupt packet (malformed ones included).
    Raw(Vec<u8>),
}

struct ScheduledEvent {
    spec: EventSpec,
    next: Option<BusTime>,
    fired: u32,
}

/// Status packets the host hasn't read yet are dropped beyond this.
const MAX_QUEUED_STATUS: usize = 64;

fn unit_of(kind: &EntityKind) -> Option<Unit> {
    match kind {
        EntityKind::Camera => Some(Unit::Camera),
        EntityKind::Processing { .. } => Some(Unit::Processing),
        _ => None,
    }
}

fn tag_of(kind: &EntityKind) -> EntityKindTag {
    match kind {
        EntityKind::Camera
        | EntityKind::InputTerminal { .. }
        | EntityKind::OutputTerminal { .. } => EntityKindTag::Camera,
        EntityKind::Processing { .. } => EntityKindTag::Processing,
        EntityKind::Selector => EntityKindTag::Selector,
        EntityKind::Extension { .. } => EntityKindTag::Extension,
        EntityKind::Encoding => EntityKindTag::Encoding,
    }
}

fn request_name(r: u8) -> &'static str {
    match r {
        SET_CUR => "SET_CUR",
        GET_CUR => "GET_CUR",
        GET_MIN => "GET_MIN",
        GET_MAX => "GET_MAX",
        GET_RES => "GET_RES",
        GET_LEN => "GET_LEN",
        GET_INFO => "GET_INFO",
        GET_DEF => "GET_DEF",
        _ => "?",
    }
}

impl UvcFunction {
    /// Creates the function with spec defaults for every advertised control.
    /// Extension unit controls default to 4 opaque bytes.
    pub fn new(
        desc: UvcFunctionDesc,
        _speed: Speed,
        quirks: StreamQuirks,
        sources: SourceFactory,
    ) -> Self {
        let UvcFunctionDesc { vc, streaming } = desc;
        let mut controls = BTreeMap::new();
        let (max_w, max_h) = streaming
            .iter()
            .flat_map(|s| s.formats.iter().flat_map(|f| f.frames.iter()))
            .map(|f| (f.width, f.height))
            .max_by_key(|&(w, h)| w as u32 * h as u32)
            .unwrap_or((640, 480));
        for e in &vc.entities {
            match (&e.kind, unit_of(&e.kind)) {
                (_, Some(unit)) => {
                    for &cs in &e.controls {
                        let c = if unit == Unit::Camera && cs == CT_REGION_OF_INTEREST_CONTROL {
                            Some(control::region_of_interest(max_w, max_h))
                        } else {
                            control::standard(unit, cs)
                        };
                        controls.insert((e.id, cs), c.unwrap_or_else(|| Control::raw(1)));
                    }
                }
                (EntityKind::Extension { .. }, _) => {
                    for &cs in &e.controls {
                        controls.insert((e.id, cs), Control::raw(4));
                    }
                }
                (EntityKind::Selector, _) => {
                    let n = e.sources.len().max(1) as i64;
                    controls.insert(
                        (e.id, SU_INPUT_SELECT_CONTROL),
                        Control::int(1, false, 1, n, 1, 1),
                    );
                }
                _ => {}
            }
        }
        let vs = streaming
            .into_iter()
            .map(|s| {
                let src = sources(&s);
                VsInterface::new(s, vc.bcd_uvc, vc.clock_frequency, src, quirks.clone())
            })
            .collect();
        let mut f = UvcFunction {
            name: format!("VC{}", vc.interface),
            vc,
            controls,
            request_error: 0,
            status: VecDeque::new(),
            vs,
            events: Vec::new(),
            async_cfg: BTreeMap::new(),
            pending: Vec::new(),
            power_mode: None,
        };
        f.update_auto_links(false);
        f
    }

    /// All interfaces of the function (VideoControl first).
    pub fn interfaces(&self) -> Vec<u8> {
        let mut v = vec![self.vc.interface];
        v.extend(self.vs.iter().map(|s| s.interface()));
        v
    }

    pub fn control(&self, entity: u8, selector: u8) -> Option<&Control> {
        self.controls.get(&(entity, selector))
    }

    pub fn control_mut(&mut self, entity: u8, selector: u8) -> Option<&mut Control> {
        self.controls.get_mut(&(entity, selector))
    }

    /// Adds or replaces a control (e.g. to give an XU control its real size).
    pub fn set_control(&mut self, entity: u8, selector: u8, c: Control) {
        self.controls.insert((entity, selector), c);
        self.update_auto_links(false);
    }

    /// Entity ids of the given kind (e.g. to find the processing unit).
    pub fn entities(&self, pred: impl Fn(&EntityKind) -> bool) -> Vec<u8> {
        self.vc
            .entities
            .iter()
            .filter(|e| pred(&e.kind))
            .map(|e| e.id)
            .collect()
    }

    /// Changes a control's value from the device side (autoupdate, privacy
    /// shutter...) and queues a control change interrupt.
    pub fn device_set(&mut self, entity: u8, selector: u8, value: &[u8]) {
        if let Some(c) = self.controls.get_mut(&(entity, selector)) {
            c.cur = value.to_vec();
            self.push_control_change(entity, selector, STATUS_ATTR_VALUE, value.to_vec());
        }
    }

    /// Adds VC_VIDEO_POWER_MODE_CONTROL (UVC 1.5 4.2.1.1), starting with
    /// `bDevicePowerMode` (D7..4: power source and vendor mode support).
    pub fn set_power_mode(&mut self, mode: u8) {
        self.power_mode = Some(mode);
    }

    /// Applies per-control settings (ranges, sizes, asynchronous completion).
    pub fn configure_controls(&mut self, specs: &[ControlSpec]) -> Result<(), String> {
        for s in specs {
            if (s.entity, s.selector) == (0, VC_VIDEO_POWER_MODE_CONTROL) {
                match s.cur.as_deref() {
                    Some(&[m]) => self.power_mode = Some(m),
                    _ => return Err(format!("{}: power mode needs cur = [byte]", self.name)),
                }
                continue;
            }
            let xu = matches!(
                self.vc.entity(s.entity).map(|e| &e.kind),
                Some(EntityKind::Extension { .. })
            );
            let c = self
                .controls
                .get_mut(&(s.entity, s.selector))
                .ok_or_else(|| {
                    format!("{}: no control {}.{:#04x}", self.name, s.entity, s.selector)
                })?;
            let what = format!("{}: control {}.{:#04x}", self.name, s.entity, s.selector);
            let ranges = [&s.min, &s.max, &s.res, &s.def];
            if !xu && (s.len.is_some() || ranges.iter().any(|r| r.is_some())) {
                return Err(format!(
                    "{what}: len/min/max/res/def are for extension units"
                ));
            }
            if s.cur.is_some() {
                return Err(format!(
                    "{what}: cur is for the power mode; def is the value after reset"
                ));
            }
            if let Some(len) = s.len {
                *c = Control::raw(len).with_info(c.info);
            }
            let len = c.len as usize;
            for (field, v) in [
                (&mut c.min, &s.min),
                (&mut c.max, &s.max),
                (&mut c.res, &s.res),
                (&mut c.def, &s.def),
            ] {
                match v.as_deref() {
                    None => {}
                    Some([]) => *field = None,
                    Some(b) if b.len() == len => *field = Some(b.to_vec()),
                    Some(b) => return Err(format!("{what}: {} bytes, GET_LEN is {len}", b.len())),
                }
            }
            c.reset();
            if let Some(info) = s.info {
                c.info = info;
            }
            if s.async_ms.is_some() || s.fail.is_some() {
                c.info |= INFO_ASYNC;
                self.async_cfg.insert(
                    (s.entity, s.selector),
                    (s.async_ms.unwrap_or(DEFAULT_ASYNC_MS), s.fail),
                );
            }
        }
        Ok(())
    }

    /// Queues a VideoStreaming button press/release interrupt.
    pub fn button(&mut self, interface: u8, pressed: bool) {
        self.push_status(vec![
            STATUS_ORIGINATOR_VS,
            interface,
            STATUS_EVENT_BUTTON_PRESS,
            pressed as u8,
        ]);
    }

    /// Schedules device-initiated events (bus time origin).
    pub fn set_events(&mut self, events: Vec<EventSpec>) {
        self.events = events
            .into_iter()
            .map(|spec| ScheduledEvent {
                next: Some(spec.at_ms * 8),
                spec,
                fired: 0,
            })
            .collect();
    }

    fn push_status(&mut self, p: Vec<u8>) {
        if self.status.len() >= MAX_QUEUED_STATUS {
            self.status.pop_front();
        }
        self.status.push_back(p);
    }

    fn fire_events(&mut self, now: BusTime) {
        let mut due = Vec::new();
        for e in &mut self.events {
            while e.next.is_some_and(|t| t <= now) {
                due.push(e.spec.kind.clone());
                e.fired += 1;
                let more = e.spec.count.is_none_or(|c| e.fired < c);
                e.next = match (e.spec.every_ms, more) {
                    (Some(p), true) if p > 0 => e.next.map(|t| t + p * 8),
                    _ => None,
                };
            }
        }
        for kind in due {
            debug!("{}: event {kind:?}", self.name);
            match kind {
                EventKind::Button => {
                    let iface = self
                        .vs
                        .first()
                        .map_or(self.vc.interface + 1, |v| v.interface());
                    self.button(iface, true);
                    self.button(iface, false);
                }
                EventKind::Control {
                    entity,
                    selector,
                    value,
                    attribute: STATUS_ATTR_VALUE,
                } => self.device_set(entity, selector, &value),
                EventKind::Control {
                    entity,
                    selector,
                    value,
                    attribute,
                } => self.push_control_change(entity, selector, attribute, value),
                EventKind::Raw(p) => self.push_status(p),
            }
        }
    }

    /// Completes the asynchronous SET_CURs that are due: Control Change interrupt with the
    /// new value, or a Control Failure Change with the error code.
    fn complete_pending(&mut self, now: BusTime) {
        let (due, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|p| p.due <= now);
        self.pending = rest;
        for p in due {
            let Some(c) = self.controls.get_mut(&(p.entity, p.selector)) else {
                continue;
            };
            // The state may have changed meanwhile (auto mode): set() checks again.
            match p.fail.map_or_else(|| c.set(&p.value), Err) {
                Ok(()) => {
                    let v = c.cur.clone();
                    debug!(
                        "{}: async {}.{:#04x} done {v:02x?}",
                        self.name, p.entity, p.selector
                    );
                    self.push_control_change(p.entity, p.selector, STATUS_ATTR_VALUE, v);
                    self.update_auto_links(true);
                }
                Err(code) => {
                    debug!(
                        "{}: async {}.{:#04x} failed {code:#04x}",
                        self.name, p.entity, p.selector
                    );
                    self.push_control_change(p.entity, p.selector, STATUS_ATTR_FAILURE, vec![code]);
                }
            }
        }
    }

    fn push_control_change(&mut self, entity: u8, selector: u8, attribute: u8, value: Vec<u8>) {
        let mut p = vec![
            STATUS_ORIGINATOR_VC,
            entity,
            STATUS_EVENT_CONTROL_CHANGE,
            selector,
            attribute,
        ];
        p.extend(value);
        self.push_status(p);
    }

    /// Recomputes which controls are disabled by an "auto" control; with
    /// `notify`, queues GET_INFO change interrupts for controls that flipped.
    fn update_auto_links(&mut self, notify: bool) {
        let mut changes = Vec::new();
        for e in &self.vc.entities {
            let Some(unit) = unit_of(&e.kind) else {
                continue;
            };
            let mut disabled: BTreeMap<u8, bool> = BTreeMap::new();
            for (auto, pred, deps) in control::auto_links(unit) {
                let Some(a) = self.controls.get(&(e.id, *auto)) else {
                    continue;
                };
                let on = pred(a.value());
                for d in *deps {
                    *disabled.entry(*d).or_default() |= on;
                }
            }
            for (cs, dis) in disabled {
                if let Some(c) = self.controls.get_mut(&(e.id, cs))
                    && c.disabled != dis
                {
                    c.disabled = dis;
                    changes.push((e.id, cs, c.info | if dis { INFO_DISABLED } else { 0 }));
                }
            }
        }
        if notify {
            for (id, cs, info) in changes {
                self.push_control_change(id, cs, STATUS_ATTR_INFO, vec![info]);
            }
        }
    }

    /// VC_VIDEO_POWER_MODE_CONTROL (4.2.1.1): D3..0 is the mode (0 full power, 1 vendor
    /// dependent, if D4 says it is supported; others reserved), D7..4 are read-only.
    fn power_request(&mut self, pm: u8, req: u8, data: &[u8]) -> Result<Vec<u8>, u8> {
        match req {
            GET_CUR => Ok(vec![pm]),
            GET_INFO => Ok(vec![INFO_GET | INFO_SET]),
            SET_CUR => match *data {
                [v] if v & 0x0f == 0 || (v & 0x0f == 1 && pm & 0x10 != 0) => {
                    debug!("{}: power mode {:#04x}", self.name, v & 0x0f);
                    self.power_mode = Some(pm & 0xf0 | v & 0x0f);
                    Ok(Vec::new())
                }
                [_] => Err(ERR_OUT_OF_RANGE),
                _ => Err(ERR_UNKNOWN),
            },
            _ => Err(ERR_INVALID_REQUEST),
        }
    }

    fn vs_mut(&mut self, iface: u8) -> Option<&mut VsInterface> {
        self.vs.iter_mut().find(|v| v.interface() == iface)
    }

    fn vc_request(
        &mut self,
        now: BusTime,
        setup: &SetupPacket,
        data: &[u8],
    ) -> Result<Vec<u8>, u8> {
        let (entity, cs, req) = (
            (setup.index >> 8) as u8,
            (setup.value >> 8) as u8,
            setup.request,
        );
        if entity == 0 {
            return match (cs, req, self.power_mode) {
                (VC_REQUEST_ERROR_CODE_CONTROL, GET_CUR, _) => Ok(vec![self.request_error]),
                (VC_REQUEST_ERROR_CODE_CONTROL, GET_INFO, _) => Ok(vec![INFO_GET]),
                (VC_REQUEST_ERROR_CODE_CONTROL, _, _) => Err(ERR_INVALID_REQUEST),
                (VC_VIDEO_POWER_MODE_CONTROL, _, Some(pm)) => self.power_request(pm, req, data),
                _ => Err(ERR_INVALID_CONTROL),
            };
        }
        let Some(kind) = self.vc.entity(entity).map(|e| e.kind.clone()) else {
            return Err(ERR_INVALID_UNIT);
        };
        let Some(c) = self.controls.get_mut(&(entity, cs)) else {
            return Err(ERR_INVALID_CONTROL);
        };
        let busy = self
            .pending
            .iter()
            .any(|p| (p.entity, p.selector) == (entity, cs));
        let (is_async, autoupdate) = (c.info & INFO_ASYNC != 0, c.info & INFO_AUTOUPDATE != 0);
        let r = match req {
            // 2.4.4 (figure 2-21): the control is busy until its Control Change interrupt.
            GET_CUR | SET_CUR if busy => Err(ERR_NOT_READY),
            SET_CUR if is_async => c.check(data).map(|_| {
                let (ms, fail) = self
                    .async_cfg
                    .get(&(entity, cs))
                    .copied()
                    .unwrap_or((DEFAULT_ASYNC_MS, None));
                self.pending.push(PendingSet {
                    entity,
                    selector: cs,
                    value: data.to_vec(),
                    due: now + ms * 8,
                    fail,
                });
                Vec::new()
            }),
            SET_CUR => c.set(data).map(|_| Vec::new()),
            _ => c.get(req),
        };
        debug!(
            "{}: {} {}({entity}).{} {:02x?} -> {:02x?}",
            self.name,
            request_name(req),
            match kind {
                EntityKind::Camera => "CT",
                EntityKind::Processing { .. } => "PU",
                EntityKind::Extension { .. } => "XU",
                EntityKind::Selector => "SU",
                _ => "??",
            },
            control_name(tag_of(&kind), cs),
            data,
            r
        );
        if req == SET_CUR && r.is_ok() && !is_async {
            // 2.4.2.2: controls with Control Change events report every SET_CUR.
            if autoupdate && let Some(c) = self.controls.get(&(entity, cs)) {
                self.push_control_change(entity, cs, STATUS_ATTR_VALUE, c.cur.clone());
            }
            if unit_of(&kind).is_some() {
                self.update_auto_links(true);
            }
        }
        r
    }
}

impl Function for UvcFunction {
    fn control(&mut self, cx: &mut Ctx, setup: &SetupPacket, data: &[u8]) -> ControlReply {
        if setup.request_type & 0x60 != 0x20 {
            return ControlReply::stall();
        }
        let iface = setup.index as u8;
        let r = if iface == self.vc.interface {
            self.vc_request(cx.now(), setup, data)
        } else if let Some(vs) = self.vs_mut(iface) {
            vs.control(
                cx,
                setup.request,
                (setup.value >> 8) as u8,
                data,
                setup.length,
            )
        } else {
            Err(ERR_INVALID_UNIT)
        };
        match r {
            Ok(mut d) => {
                // Reading the error code doesn't reset it to "no error" before the reply.
                if !(iface == self.vc.interface && setup.index >> 8 == 0) {
                    self.request_error = ERR_NO_ERROR;
                }
                d.truncate(setup.length as usize);
                ControlReply::data(d)
            }
            Err(code) => {
                debug!(
                    "{}: {:?} failed with error code {code:#04x}",
                    self.name, setup
                );
                self.request_error = code;
                ControlReply::stall()
            }
        }
    }

    fn set_alt(&mut self, cx: &mut Ctx, iface: u8, alt: u8) -> Result<(), UsbError> {
        if let Some(vs) = self.vs_mut(iface) {
            vs.set_alt(cx, alt);
        }
        Ok(())
    }

    fn reset(&mut self, _cx: &mut Ctx) {
        for c in self.controls.values_mut() {
            c.reset();
        }
        self.update_auto_links(false);
        self.request_error = 0;
        self.status.clear();
        self.pending.clear();
        if let Some(pm) = &mut self.power_mode {
            *pm &= 0xf0;
        }
        for vs in &mut self.vs {
            vs.reset();
        }
    }

    fn data_in(&mut self, cx: &mut Ctx, ep: u8, max_len: usize) -> InReply {
        if self
            .vc
            .status_endpoint
            .as_ref()
            .is_some_and(|s| s.address == ep)
        {
            return match self.status.pop_front() {
                Some(mut p) => {
                    p.truncate(max_len);
                    InReply::Data(p)
                }
                None => InReply::Nak,
            };
        }
        if let Some(vs) = self.vs.iter_mut().find(|v| v.still_endpoint() == Some(ep)) {
            return vs.still_in(cx, max_len).map_or(InReply::Nak, InReply::Data);
        }
        match self.vs.iter_mut().find(|v| v.desc.endpoint == ep) {
            Some(vs) => vs.bulk_in(cx, max_len).map_or(InReply::Nak, InReply::Data),
            None => InReply::Error(UsbError::Stall),
        }
    }

    fn iso_in(&mut self, cx: &mut Ctx, ep: u8, max_len: usize) -> IsoReply {
        match self.vs.iter_mut().find(|v| v.desc.endpoint == ep) {
            Some(vs) => IsoReply::data(vs.iso_in(cx, max_len)),
            None => IsoReply::default(),
        }
    }

    fn clear_halt(&mut self, _cx: &mut Ctx, ep: u8) {
        for vs in &mut self.vs {
            vs.clear_halt(ep);
        }
    }

    fn next_wakeup(&self) -> Option<BusTime> {
        let bulk = self
            .vs
            .iter()
            .filter(|v| matches!(v.desc.transfer, Transfer::Bulk { .. }))
            .filter_map(|v| v.next_wakeup());
        bulk.chain(self.events.iter().filter_map(|e| e.next))
            .chain(self.pending.iter().map(|p| p.due))
            .min()
    }

    fn tick(&mut self, cx: &mut Ctx) {
        self.complete_pending(cx.now());
        self.fire_events(cx.now());
    }
}
