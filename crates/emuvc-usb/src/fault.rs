//! Generic, transport-independent fault injection.
//!
//! [`FaultInjector`] wraps any [`UsbDevice`] and rewrites its answers
//! according to [`FaultRule`]s: stall or time out matching control requests,
//! corrupt or drop isochronous packets, disconnect after a while, etc.
//! Class-aware misbehaviour (e.g. a UVC camera with a broken probe/commit)
//! belongs in the class model instead; this layer only sees USB traffic.
//!
//! Rules deserialise from TOML, e.g.:
//!
//! ```toml
//! # First GET_DEF (0x87) of brightness (selector 2) on the processing unit (entity 2) stalls.
//! [[fault]]
//! on = { control = { request = 0x87, selector = 2, entity = 2 } }
//! action = "stall"
//! count = 1
//!
//! # 0.1% of isochronous packets on 0x81 get a CRC error.
//! [[fault]]
//! on = { iso_in = { ep = 0x81 } }
//! action = { error = "protocol" }
//! probability = 0.001
//!
//! # Unplug after 5 s.
//! [[fault]]
//! on = "time"
//! after_ms = 5000
//! action = "disconnect"
//! ```

use serde::{Deserialize, Serialize};

use crate::device::{
    BusTime, ControlReply, Ctx, DeviceEvent, EndpointInfo, InReply, IsoReply, OutReply, UsbDevice,
};
use crate::status::UsbError;
use crate::types::{SetupPacket, Speed};

/// Which traffic a rule applies to. Unset fields match anything.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Match {
    Control {
        #[serde(default)]
        request_type: Option<u8>,
        #[serde(default)]
        request: Option<u8>,
        #[serde(default)]
        value: Option<u16>,
        #[serde(default)]
        index: Option<u16>,
        /// High byte of wValue (UVC control selector, descriptor type...).
        #[serde(default)]
        selector: Option<u8>,
        /// High byte of wIndex (UVC entity / unit id).
        #[serde(default)]
        entity: Option<u8>,
        /// Low byte of wIndex (interface or endpoint).
        #[serde(default)]
        interface: Option<u8>,
    },
    DataIn {
        #[serde(default)]
        ep: Option<u8>,
    },
    DataOut {
        #[serde(default)]
        ep: Option<u8>,
    },
    IsoIn {
        #[serde(default)]
        ep: Option<u8>,
    },
    IsoOut {
        #[serde(default)]
        ep: Option<u8>,
    },
    /// Fires once, when the bus time reaches `after_ms`.
    Time,
}

impl Match {
    fn control(&self, s: &SetupPacket) -> bool {
        let Match::Control {
            request_type,
            request,
            value,
            index,
            selector,
            entity,
            interface,
        } = self
        else {
            return false;
        };
        request_type.is_none_or(|v| v == s.request_type)
            && request.is_none_or(|v| v == s.request)
            && value.is_none_or(|v| v == s.value)
            && index.is_none_or(|v| v == s.index)
            && selector.is_none_or(|v| v as u16 == s.value >> 8)
            && entity.is_none_or(|v| v as u16 == s.index >> 8)
            && interface.is_none_or(|v| v as u16 == s.index & 0xff)
    }
}

/// What to do when a rule fires.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    /// STALL the request/endpoint (the device never sees the request).
    Stall,
    /// Never answer (the host's timeout fires).
    Timeout,
    /// Bulk/interrupt: pretend nothing is ready.
    Nak,
    /// Complete with this error (iso: the packet's data is lost).
    Error(UsbError),
    /// Answer this many milliseconds late (control transfers).
    DelayMs(u32),
    /// Keep only the first N bytes.
    Truncate(usize),
    /// XOR one byte (ignored if the data is shorter).
    Corrupt { offset: usize, xor: u8 },
    /// Replace the data entirely.
    Replace(Vec<u8>),
    /// Isochronous: send a zero-length packet instead (data lost).
    Zlp,
    /// Drop off the bus.
    Disconnect,
}

impl Action {
    /// Whether the wrapped device still processes the request.
    fn passes_through(&self) -> bool {
        matches!(
            self,
            Action::DelayMs(_)
                | Action::Truncate(_)
                | Action::Corrupt { .. }
                | Action::Replace(_)
                | Action::Zlp
        )
    }

    fn mangle(&self, data: &mut Vec<u8>) {
        match self {
            Action::Truncate(n) => data.truncate(*n),
            Action::Corrupt { offset, xor } => {
                if let Some(b) = data.get_mut(*offset) {
                    *b ^= xor;
                }
            }
            Action::Replace(d) => *data = d.clone(),
            Action::Zlp => data.clear(),
            _ => {}
        }
    }
}

/// One fault rule. See the module documentation for the TOML form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaultRule {
    pub on: Match,
    pub action: Action,
    /// Ignore the first N matches.
    #[serde(default)]
    pub skip: u32,
    /// Fire at most N times.
    #[serde(default)]
    pub count: Option<u32>,
    /// Only fire on every Nth (eligible) match.
    #[serde(default)]
    pub every: Option<u32>,
    #[serde(default)]
    pub probability: Option<f64>,
    /// Only active after this bus time (ms).
    #[serde(default)]
    pub after_ms: Option<u64>,
    /// Only active before this bus time (ms).
    #[serde(default)]
    pub until_ms: Option<u64>,
}

struct RuleState {
    rule: FaultRule,
    seen: u32,
    fired: u32,
}

const UFRAMES_PER_MS: BusTime = 8;

impl RuleState {
    fn active(&self, now: BusTime) -> bool {
        let ms = now / UFRAMES_PER_MS;
        self.rule.after_ms.is_none_or(|a| ms >= a)
            && self.rule.until_ms.is_none_or(|u| ms < u)
            && self.rule.count.is_none_or(|c| self.fired < c)
    }

    /// Counts a match and decides whether the rule fires.
    fn fire(&mut self, cx: &mut Ctx) -> bool {
        if !self.active(cx.now()) {
            return false;
        }
        self.seen += 1;
        if self.seen <= self.rule.skip {
            return false;
        }
        let n = self.seen - self.rule.skip;
        if self
            .rule
            .every
            .is_some_and(|e| e > 1 && !n.is_multiple_of(e))
        {
            return false;
        }
        if let Some(p) = self.rule.probability
            && !cx.rng().chance(p)
        {
            return false;
        }
        self.fired += 1;
        true
    }
}

/// Wraps a device and applies fault rules to its traffic.
pub struct FaultInjector<D> {
    inner: D,
    rules: Vec<RuleState>,
}

impl<D: UsbDevice> FaultInjector<D> {
    pub fn new(inner: D, rules: Vec<FaultRule>) -> Self {
        FaultInjector {
            inner,
            rules: rules
                .into_iter()
                .map(|rule| RuleState {
                    rule,
                    seen: 0,
                    fired: 0,
                })
                .collect(),
        }
    }

    pub fn inner(&self) -> &D {
        &self.inner
    }

    fn pick(&mut self, cx: &mut Ctx, pred: impl Fn(&Match) -> bool) -> Option<Action> {
        for r in &mut self.rules {
            if pred(&r.rule.on) && r.fire(cx) {
                log::info!("fault: {:?} on {:?}", r.rule.action, r.rule.on);
                return Some(r.rule.action.clone());
            }
        }
        None
    }
}

fn ep_match(want: &Option<u8>, ep: u8) -> bool {
    want.is_none_or(|w| w == ep)
}

impl<D: UsbDevice> UsbDevice for FaultInjector<D> {
    fn name(&self) -> String {
        self.inner.name()
    }

    fn speed(&self) -> Speed {
        self.inner.speed()
    }

    fn reset(&mut self, cx: &mut Ctx) {
        self.inner.reset(cx)
    }

    fn control(&mut self, cx: &mut Ctx, setup: &SetupPacket, data: &[u8]) -> ControlReply {
        let Some(action) = self.pick(cx, |m| m.control(setup)) else {
            return self.inner.control(cx, setup, data);
        };
        if action == Action::Disconnect {
            cx.emit(DeviceEvent::Disconnect);
        }
        if !action.passes_through() {
            return ControlReply::error(match action {
                Action::Timeout | Action::Nak => UsbError::Timeout,
                Action::Error(e) => e,
                Action::Disconnect => UsbError::NoDevice,
                _ => UsbError::Stall,
            });
        }
        let mut reply = self.inner.control(cx, setup, data);
        if let Action::DelayMs(ms) = action {
            reply.latency_us = reply.latency_us.saturating_add(ms.saturating_mul(1000));
        } else if let Ok(d) = &mut reply.result {
            action.mangle(d);
        }
        reply
    }

    fn endpoint(&self, address: u8) -> Option<EndpointInfo> {
        self.inner.endpoint(address)
    }

    fn data_in(&mut self, cx: &mut Ctx, ep: u8, max_len: usize) -> InReply {
        let Some(action) = self.pick(
            cx,
            |m| matches!(m, Match::DataIn { ep: e } if ep_match(e, ep)),
        ) else {
            return self.inner.data_in(cx, ep, max_len);
        };
        match action {
            Action::Nak | Action::Timeout => InReply::Nak,
            Action::Stall => InReply::Error(UsbError::Stall),
            Action::Error(e) => InReply::Error(e),
            Action::Disconnect => {
                cx.emit(DeviceEvent::Disconnect);
                InReply::Error(UsbError::NoDevice)
            }
            a => match self.inner.data_in(cx, ep, max_len) {
                InReply::Data(mut d) => {
                    a.mangle(&mut d);
                    InReply::Data(d)
                }
                r => r,
            },
        }
    }

    fn data_out(&mut self, cx: &mut Ctx, ep: u8, data: &[u8]) -> OutReply {
        let Some(action) = self.pick(
            cx,
            |m| matches!(m, Match::DataOut { ep: e } if ep_match(e, ep)),
        ) else {
            return self.inner.data_out(cx, ep, data);
        };
        match action {
            Action::Nak | Action::Timeout => OutReply::Nak,
            Action::Stall => OutReply::Error(UsbError::Stall),
            Action::Error(e) => OutReply::Error(e),
            Action::Disconnect => {
                cx.emit(DeviceEvent::Disconnect);
                OutReply::Error(UsbError::NoDevice)
            }
            a => {
                let mut d = data.to_vec();
                a.mangle(&mut d);
                self.inner.data_out(cx, ep, &d)
            }
        }
    }

    fn iso_in(&mut self, cx: &mut Ctx, ep: u8, max_len: usize) -> IsoReply {
        let action = self.pick(
            cx,
            |m| matches!(m, Match::IsoIn { ep: e } if ep_match(e, ep)),
        );
        // Isochronous data is generated (and lost) whatever happens on the wire.
        let mut r = self.inner.iso_in(cx, ep, max_len);
        match action {
            None => {}
            Some(Action::Error(e)) => {
                r.data.clear();
                r.error = Some(e);
            }
            Some(Action::Stall | Action::Timeout | Action::Nak) => {
                r.data.clear();
                r.error = Some(UsbError::Protocol);
            }
            Some(Action::Disconnect) => {
                cx.emit(DeviceEvent::Disconnect);
                r.data.clear();
                r.error = Some(UsbError::NoDevice);
            }
            Some(a) => a.mangle(&mut r.data),
        }
        r
    }

    fn iso_out(&mut self, cx: &mut Ctx, ep: u8, data: &[u8]) -> IsoReply {
        match self.pick(
            cx,
            |m| matches!(m, Match::IsoOut { ep: e } if ep_match(e, ep)),
        ) {
            None => self.inner.iso_out(cx, ep, data),
            Some(Action::Error(e)) => IsoReply {
                data: Vec::new(),
                error: Some(e),
            },
            Some(Action::Disconnect) => {
                cx.emit(DeviceEvent::Disconnect);
                IsoReply {
                    data: Vec::new(),
                    error: Some(UsbError::NoDevice),
                }
            }
            Some(a) => {
                let mut d = data.to_vec();
                a.mangle(&mut d);
                self.inner.iso_out(cx, ep, &d)
            }
        }
    }

    fn next_wakeup(&self) -> Option<BusTime> {
        let timed = self
            .rules
            .iter()
            .filter(|r| r.rule.on == Match::Time && r.fired == 0)
            .filter_map(|r| r.rule.after_ms.map(|ms| ms * UFRAMES_PER_MS))
            .min();
        match (timed, self.inner.next_wakeup()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn tick(&mut self, cx: &mut Ctx) {
        let now = cx.now();
        for r in &mut self.rules {
            let due = r.rule.on == Match::Time
                && r.fired == 0
                && r.rule.after_ms.is_some_and(|ms| now >= ms * UFRAMES_PER_MS);
            if due {
                r.fired = 1;
                log::info!("fault: {:?} at {} ms", r.rule.action, now / UFRAMES_PER_MS);
                if r.rule.action == Action::Disconnect {
                    cx.emit(DeviceEvent::Disconnect);
                }
            }
        }
        self.inner.tick(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Cfg {
        fault: Vec<FaultRule>,
    }

    #[test]
    fn rule_counting() {
        let rule = FaultRule {
            on: Match::DataIn { ep: Some(0x81) },
            action: Action::Stall,
            skip: 1,
            count: Some(2),
            every: None,
            probability: None,
            after_ms: None,
            until_ms: None,
        };
        let mut st = RuleState {
            rule,
            seen: 0,
            fired: 0,
        };
        let (mut rng, mut ev) = (crate::rng::Rng::new(0), Vec::new());
        let mut cx = Ctx::new(0, &mut rng, &mut ev);
        let fired: Vec<bool> = (0..5).map(|_| st.fire(&mut cx)).collect();
        assert_eq!(fired, [false, true, true, false, false]);
    }

    #[test]
    fn toml_shapes() {
        let cfg: Cfg = toml::from_str(
            r#"
            [[fault]]
            on = { control = { request = 0x87, selector = 2, entity = 2 } }
            action = "stall"
            count = 1

            [[fault]]
            on = { iso_in = { ep = 0x81 } }
            action = { error = "protocol" }
            probability = 0.001

            [[fault]]
            on = "time"
            after_ms = 5000
            action = "disconnect"

            [[fault]]
            on = { data_in = {} }
            action = { corrupt = { offset = 1, xor = 0x40 } }
            "#,
        )
        .unwrap();
        assert_eq!(cfg.fault.len(), 4);
        assert!(
            cfg.fault[0]
                .on
                .control(&SetupPacket::new(0xa1, 0x87, 0x0200, 0x0200, 2))
        );
        assert!(
            !cfg.fault[0]
                .on
                .control(&SetupPacket::new(0xa1, 0x81, 0x0200, 0x0200, 2))
        );
        assert_eq!(cfg.fault[1].action, Action::Error(UsbError::Protocol));
        assert_eq!(cfg.fault[2].on, Match::Time);
    }
}
