//! Transport-level faults: make chosen libusb calls fail, to exercise the
//! host's error handling (what device-side fault rules can't reach, e.g.
//! `libusb_open` refusing access or `libusb_submit_transfer` running out of
//! memory).
//!
//! `EMUVC_LIBUSB_FAIL="FUNC=ERROR[@N[-M]],..."`: `FUNC` is a libusb function
//! name without the `libusb_` prefix (open, claim_interface,
//! set_interface_alt_setting, submit_transfer, control_transfer,
//! get_device_list, alloc_transfer, ...), `ERROR` a libusb error name
//! without prefix (io, access, busy, no_mem, pipe, timeout, no_device, ...),
//! `@N-M` restricts it to the Nth..Mth calls (1-based; default: every call).
//! Example: `EMUVC_LIBUSB_FAIL="submit_transfer=no_mem@5,open=access@1"`.
//!
//! `submit_transfer` counts every submission, including those of the
//! synchronous calls (`libusb_control_transfer`, ...). `submit_transfer_iso`,
//! `_bulk`, `_interrupt` and `_control` count only that type of transfer, e.g.
//! `submit_transfer_iso=no_mem@1` fails a stream's first isochronous transfer.

use std::collections::HashMap;
use std::ffi::c_int;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::ffi::*;

struct Rule {
    error: c_int,
    calls: Option<(u32, u32)>,
    count: AtomicU32,
}

fn error_code(name: &str) -> Option<c_int> {
    Some(match name {
        "io" => LIBUSB_ERROR_IO,
        "invalid_param" => LIBUSB_ERROR_INVALID_PARAM,
        "access" => LIBUSB_ERROR_ACCESS,
        "no_device" => LIBUSB_ERROR_NO_DEVICE,
        "not_found" => LIBUSB_ERROR_NOT_FOUND,
        "busy" => LIBUSB_ERROR_BUSY,
        "timeout" => LIBUSB_ERROR_TIMEOUT,
        "overflow" => LIBUSB_ERROR_OVERFLOW,
        "pipe" => LIBUSB_ERROR_PIPE,
        "interrupted" => LIBUSB_ERROR_INTERRUPTED,
        "no_mem" => LIBUSB_ERROR_NO_MEM,
        "not_supported" => LIBUSB_ERROR_NOT_SUPPORTED,
        "other" => LIBUSB_ERROR_OTHER,
        _ => return None,
    })
}

fn parse(spec: &str) -> HashMap<String, Rule> {
    let mut rules = HashMap::new();
    for item in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let Some((func, rest)) = item.split_once('=') else {
            log::error!("emuvc: EMUVC_LIBUSB_FAIL: bad item {item:?}");
            continue;
        };
        let (err, calls) = match rest.split_once('@') {
            Some((e, range)) => {
                let (a, b) = range.split_once('-').unwrap_or((range, range));
                match (a.parse(), b.parse()) {
                    (Ok(a), Ok(b)) => (e, Some((a, b))),
                    _ => {
                        log::error!("emuvc: EMUVC_LIBUSB_FAIL: bad call range in {item:?}");
                        continue;
                    }
                }
            }
            None => (rest, None),
        };
        match error_code(err) {
            Some(error) => {
                rules.insert(
                    func.trim_start_matches("libusb_").to_string(),
                    Rule {
                        error,
                        calls,
                        count: AtomicU32::new(0),
                    },
                );
            }
            None => log::error!("emuvc: EMUVC_LIBUSB_FAIL: unknown error {err:?}"),
        }
    }
    rules
}

fn rules() -> &'static HashMap<String, Rule> {
    static RULES: OnceLock<HashMap<String, Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        std::env::var("EMUVC_LIBUSB_FAIL")
            .map(|s| parse(&s))
            .unwrap_or_default()
    })
}

/// Error to return from `func` on this call, if a rule says so.
pub fn check(func: &str) -> Option<c_int> {
    let rule = rules().get(func)?;
    let n = rule.count.fetch_add(1, Ordering::Relaxed) + 1;
    let fail = rule.calls.is_none_or(|(a, b)| (a..=b).contains(&n));
    if fail {
        log::info!(
            "emuvc: injected failure: libusb_{func} call {n} -> {}",
            rule.error
        );
    }
    fail.then_some(rule.error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rules() {
        let r = parse(
            "submit_transfer=no_mem@5,libusb_open=access, claim_interface=busy@2-3,bad,x=nope",
        );
        assert_eq!(r.len(), 3);
        assert_eq!(r["submit_transfer"].calls, Some((5, 5)));
        assert_eq!(r["open"].error, LIBUSB_ERROR_ACCESS);
        assert_eq!(r["claim_interface"].calls, Some((2, 3)));
    }
}
