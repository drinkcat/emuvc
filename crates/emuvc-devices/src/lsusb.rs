//! `lsusb -v` importer: turns the text back into descriptor bytes.
//!
//! Bug reports about cameras usually carry `lsusb -v` output rather than
//! binary descriptors. [`parse`] rebuilds the exact descriptor bytes from it so
//! any reported camera can be emulated (model `lsusb`, see [`build`]).
//!
//! Reconstruction rules:
//! - Descriptors are rebuilt in the order lsusb prints them (which is the
//!   order of the bytes), field by field, from the *printed* values:
//!   `bLength`, `wTotalLength`, `bNumFormats`... are never recomputed, so
//!   buggy descriptors stay buggy. A field lsusb didn't print (e.g. because
//!   `bLength` is too short: "Warning: Descriptor too short") is absent from
//!   the bytes: each descriptor is truncated or zero padded to its `bLength`.
//! - `** UNRECOGNIZED: xx xx ...` lines are copied verbatim.
//! - lsusb only prints up to 4 bytes of `bmControls` for terminals/units and
//!   `bmaControls` of video streaming headers; longer bitmaps are zero padded.
//! - Descriptors lsusb decodes but this module can't rebuild (e.g. MIDI, CDC,
//!   audio class 2.0) are an error naming the descriptor.
//! - Strings come from `iProduct 2 Foo` style lines (language 0x0409); empty
//!   strings (lsusb couldn't read them) are left unset.
//! - "Device Qualifier", hub descriptors and status lines are ignored.

use std::path::Path;

use emuvc_usb::desc::{ConfigView, dtype};
use emuvc_usb::{CompositeDevice, Descriptors, Speed};
use emuvc_uvc::topology::find_video_control_interfaces;

use crate::config::DeviceConfig;

pub const NAME: &str = "lsusb";
pub const DESCRIPTION: &str = "any device from `lsusb -v` text (descriptors = \"path\", index = N)";

/// One device of an `lsusb -v` listing.
#[derive(Clone, Debug)]
pub struct LsusbDevice {
    pub bus: u8,
    pub address: u8,
    /// From "Negotiated speed:" when present (recent lsusb only).
    pub speed: Option<Speed>,
    pub descriptors: Descriptors,
}

impl LsusbDevice {
    /// Raw dump: device descriptor followed by the configuration descriptors
    /// (the sysfs `descriptors` format, usable with `generic:FILE`).
    pub fn raw_dump(&self) -> Vec<u8> {
        let mut out = self.descriptors.device.clone();
        self.descriptors
            .configs
            .iter()
            .for_each(|c| out.extend_from_slice(c));
        out
    }

    /// True if the first configuration has a VideoControl interface.
    pub fn is_uvc(&self) -> bool {
        self.descriptors
            .configs
            .first()
            .and_then(|c| ConfigView::parse(c).ok())
            .is_some_and(|v| !find_video_control_interfaces(&v).is_empty())
    }
}

/// Reads and parses an lsusb text file (invalid UTF-8 in strings is replaced).
pub fn parse_file(path: &Path) -> Result<Vec<LsusbDevice>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&String::from_utf8_lossy(&bytes)).map_err(|e| format!("{}: {e}", path.display()))
}

/// Picks the device `index`, or the first UVC device (else the first device).
pub fn select(devices: &[LsusbDevice], index: Option<usize>) -> Result<&LsusbDevice, String> {
    match index {
        Some(i) => devices.get(i).ok_or_else(|| {
            format!(
                "device index {i} out of range ({} devices in the lsusb text)",
                devices.len()
            )
        }),
        None => devices
            .iter()
            .find(|d| d.is_uvc())
            .or(devices.first())
            .ok_or_else(|| "no device in lsusb text".to_string()),
    }
}

/// Model `lsusb`: emulates a device from `lsusb -v` text.
pub fn build(cfg: &DeviceConfig) -> Result<CompositeDevice, String> {
    let path = cfg
        .descriptors
        .as_ref()
        .ok_or("lsusb device needs `descriptors` (path of the lsusb -v text)")?;
    let devices = parse_file(path)?;
    let dev = select(&devices, cfg.index)?;
    crate::generic::from_descriptors(dev.descriptors.clone(), dev.speed, cfg)
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

/// A header line ("Interface Descriptor:") and the field lines below it.
#[derive(Debug, Default)]
struct Block {
    header: String,
    line: usize,
    /// Normalised (lowercase, no spaces inside brackets) name, rest of line.
    fields: Vec<(String, String)>,
    /// Bytes of a `** UNRECOGNIZED:` line.
    raw: Option<Vec<u8>>,
    /// GUIDs are printed as real GUIDs (modern lsusb) rather than in byte order.
    guid_swapped: bool,
}

#[derive(Debug, Default)]
struct RawDevice {
    bus: u8,
    address: u8,
    speed: Option<Speed>,
    blocks: Vec<Block>,
}

fn normalise_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Splits "bmaControls( 0)   27" into ("bmacontrols(0)", "27").
fn split_field(line: &str) -> (String, &str) {
    let mut depth = 0;
    let mut end = line.len();
    for (i, c) in line.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            c if c.is_whitespace() && depth == 0 => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    (normalise_name(&line[..end]), line[end..].trim_start())
}

fn parse_speed(s: &str) -> Option<Speed> {
    let l = s.to_ascii_lowercase();
    if l.contains("plus") || l.contains("super speed+") || l.contains("superspeed+") {
        Some(Speed::SuperPlus)
    } else if l.contains("super") {
        Some(Speed::Super)
    } else if l.contains("high") {
        Some(Speed::High)
    } else if l.contains("full") {
        Some(Speed::Full)
    } else if l.contains("low") {
        Some(Speed::Low)
    } else {
        None
    }
}

fn parse_bus_line(line: &str) -> Option<(u8, u8)> {
    let rest = line.strip_prefix("Bus ")?;
    let (bus, rest) = rest.split_once(' ')?;
    let rest = rest.trim_start().strip_prefix("Device ")?;
    let addr: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    Some((bus.trim().parse().ok()?, addr.parse().ok()?))
}

fn parse_hex_bytes(s: &str) -> Vec<u8> {
    s.split_whitespace()
        .map_while(|t| u8::from_str_radix(t, 16).ok())
        .collect()
}

fn tokenize(text: &str) -> Vec<RawDevice> {
    let mut devices: Vec<RawDevice> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if !line.starts_with(' ')
            && let Some((bus, address)) = parse_bus_line(line)
        {
            devices.push(RawDevice {
                bus,
                address,
                ..Default::default()
            });
            continue;
        }
        // Dumps pasted without the "Bus NNN Device NNN" line (issue reports):
        // a "Device Descriptor:" header starts an anonymous device.
        if t == "Device Descriptor:"
            && devices.last().is_none_or(|d| {
                d.blocks
                    .iter()
                    .any(|b| b.header.ends_with("Device Descriptor"))
            })
        {
            devices.push(RawDevice::default());
        }
        let Some(dev) = devices.last_mut() else {
            continue;
        };
        if let Some(rest) = t.strip_prefix("** UNRECOGNIZED:") {
            dev.blocks.push(Block {
                header: "** UNRECOGNIZED".into(),
                line: n + 1,
                raw: Some(parse_hex_bytes(rest)),
                ..Default::default()
            });
        } else if t.starts_with("**") {
            // "** UNAVAILABLE" etc.
        } else if let Some(rest) = t.strip_prefix("Negotiated speed:") {
            dev.speed = parse_speed(rest);
        } else if let Some(head) = t.strip_suffix(':') {
            let indent = line.len() - line.trim_start().len();
            dev.blocks.push(Block {
                header: format!("{}{}", if indent == 0 { "^" } else { "" }, head),
                line: n + 1,
                ..Default::default()
            });
        } else if let Some(b) = dev.blocks.last_mut() {
            let (name, value) = split_field(t);
            b.fields.push((name, value.to_string()));
        }
    }
    // Old lsusb prints the standard format GUIDs as {32595559-0000-1000-...}
    // (bytes in order), new ones as {32595559-0000-0010-...} (real GUID).
    for dev in &mut devices {
        let guid = dev
            .blocks
            .iter()
            .find_map(|b| b.fields.iter().find(|(n, _)| n == "guidformat"))
            .map(|(_, v)| v.to_ascii_lowercase());
        let swapped = guid.is_none_or(|g| !g.contains("-0000-1000-8000-"));
        dev.blocks.iter_mut().for_each(|b| b.guid_swapped = swapped);
    }
    devices
}

// ---------------------------------------------------------------------------
// Field access
// ---------------------------------------------------------------------------

fn parse_num(s: &str) -> Option<u64> {
    let s = s.trim_start();
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        // Old lsusb prints "0x 9" (padded after the prefix).
        let h = h.trim_start();
        let end = h.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(h.len());
        return u64::from_str_radix(&h[..end], 16).ok();
    }
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    s[..end].parse().ok()
}

/// "2.01" -> 0x0201 (lsusb prints each byte in hex: `%2x.%02x`).
fn parse_bcd(s: &str) -> Option<u64> {
    let tok = s.split_whitespace().next()?;
    let (hi, lo) = tok.split_once('.')?;
    Some(u64::from_str_radix(hi, 16).ok()? << 8 | u64::from_str_radix(lo, 16).ok()?)
}

/// "{26b8105a-0713-...}" -> 16 bytes in the order printed.
fn parse_uuid(s: &str) -> Option<[u8; 16]> {
    let hex: Vec<u8> = s
        .bytes()
        .take_while(|b| *b != b'}')
        .filter(|b| b.is_ascii_hexdigit())
        .collect();
    if hex.len() < 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(std::str::from_utf8(&hex[2 * i..2 * i + 2]).ok()?, 16).ok()?;
    }
    Some(out)
}

/// UVC GUIDs: modern lsusb prints real GUIDs (first three fields little
/// endian in the descriptor), old versions print the bytes in order.
fn parse_guid(s: &str, swapped: bool) -> Option<[u8; 16]> {
    let mut b = parse_uuid(s)?;
    if !swapped {
        return Some(b);
    }
    b[0..4].reverse();
    b[4..6].reverse();
    b[6..8].reverse();
    Some(b)
}

impl Block {
    fn val(&self, names: &[&str]) -> Option<&str> {
        names.iter().find_map(|n| {
            self.fields
                .iter()
                .find(|(f, _)| f == &n.to_ascii_lowercase())
                .map(|(_, v)| v.as_str())
        })
    }

    fn has(&self, names: &[&str]) -> bool {
        self.val(names).is_some()
    }

    fn n(&self, names: &[&str]) -> u64 {
        self.val(names).and_then(parse_num).unwrap_or(0)
    }

    fn bcd(&self, names: &[&str]) -> u64 {
        self.val(names).and_then(parse_bcd).unwrap_or(0)
    }

    /// All values of a name that repeats (HID descriptor lists).
    fn all(&self, name: &str) -> Vec<&str> {
        let name = name.to_ascii_lowercase();
        self.fields
            .iter()
            .filter(|(f, _)| *f == name)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    /// Values of `prefix(0)`, `prefix(1)`... (or `prefix[0]`), in index order.
    fn indexed(&self, prefix: &str) -> Vec<&str> {
        let prefix = prefix.to_ascii_lowercase();
        let mut v: Vec<(usize, &str)> = self
            .fields
            .iter()
            .filter_map(|(f, val)| {
                let rest = f.strip_prefix(&prefix)?;
                let idx = rest.strip_prefix('(').or_else(|| rest.strip_prefix('['))?;
                let idx: usize = idx.trim_end_matches([')', ']']).parse().ok()?;
                Some((idx, val.as_str()))
            })
            .collect();
        v.sort_by_key(|(i, _)| *i);
        v.into_iter().map(|(_, v)| v).collect()
    }

    fn subtype(&self) -> u64 {
        self.n(&["bDescriptorSubtype"])
    }

    fn subtype_name(&self) -> String {
        let s = self.val(&["bDescriptorSubtype"]).unwrap_or("?");
        s.to_string()
    }

    /// Bitmap of `size` bytes: `name(0)`... bytes if printed per byte, else
    /// the printed integer (lsusb shows at most 4 bytes), little endian.
    fn bitmap(&self, name: &str, size: usize) -> Vec<u8> {
        let per_byte = self.indexed(name);
        let mut v: Vec<u8> = if !per_byte.is_empty() {
            per_byte
                .iter()
                .map(|s| parse_num(s).unwrap_or(0) as u8)
                .collect()
        } else {
            self.n(&[name]).to_le_bytes().to_vec()
        };
        v.resize(size, 0);
        v
    }

    fn err(&self, what: &str) -> String {
        format!(
            "line {}: {}: {what}",
            self.line,
            self.header.trim_start_matches('^')
        )
    }
}

#[derive(Default)]
struct W(Vec<u8>);

impl W {
    /// Starts a descriptor; `bLength` is patched by [`W::finish`].
    fn new(b: &Block, default_type: u8) -> W {
        W(vec![
            0,
            b.val(&["bDescriptorType"])
                .and_then(parse_num)
                .map_or(default_type, |t| t as u8),
        ])
    }

    fn u8(&mut self, x: u64) -> &mut W {
        self.0.push(x as u8);
        self
    }

    fn u16(&mut self, x: u64) -> &mut W {
        self.0.extend_from_slice(&(x as u16).to_le_bytes());
        self
    }

    fn u24(&mut self, x: u64) -> &mut W {
        self.0.extend_from_slice(&(x as u32).to_le_bytes()[..3]);
        self
    }

    fn u32(&mut self, x: u64) -> &mut W {
        self.0.extend_from_slice(&(x as u32).to_le_bytes());
        self
    }

    fn bytes(&mut self, b: &[u8]) -> &mut W {
        self.0.extend_from_slice(b);
        self
    }

    /// Applies the printed `bLength`: truncates or zero pads.
    fn finish(mut self, b: &Block) -> Vec<u8> {
        let len = match b.val(&["bLength"]).and_then(parse_num) {
            Some(l) => (l as usize).min(255),
            None => self.0.len(),
        };
        self.0.resize(len, 0);
        if let Some(first) = self.0.first_mut() {
            *first = len as u8;
        }
        self.0
    }
}

/// `bmControls`-like value printed with a bControlSize.
fn clock_frequency(b: &Block) -> u64 {
    let Some(v) = b.val(&["dwClockFrequency"]) else {
        return 0;
    };
    let tok = v.split_whitespace().next().unwrap_or("");
    if let Some(mhz) = tok.strip_suffix("MHz") {
        return (mhz.parse::<f64>().unwrap_or(0.0) * 1e6).round() as u64;
    }
    parse_num(v).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Descriptor builders
// ---------------------------------------------------------------------------

fn device_descriptor(b: &Block) -> Vec<u8> {
    let mut w = W::new(b, dtype::DEVICE);
    w.u16(b.bcd(&["bcdUSB"]))
        .u8(b.n(&["bDeviceClass"]))
        .u8(b.n(&["bDeviceSubClass"]))
        .u8(b.n(&["bDeviceProtocol"]))
        .u8(b.n(&["bMaxPacketSize0"]))
        .u16(b.n(&["idVendor"]))
        .u16(b.n(&["idProduct"]))
        .u16(b.bcd(&["bcdDevice"]))
        .u8(b.n(&["iManufacturer"]))
        .u8(b.n(&["iProduct"]))
        .u8(b.n(&["iSerial", "iSerialNumber"]))
        .u8(b.n(&["bNumConfigurations"]));
    w.finish(b)
}

fn config_header(b: &Block, bcd_usb: u64) -> Vec<u8> {
    let mut w = W::new(b, dtype::CONFIG);
    // lsusb prints bMaxPower in mA: 2 mA units, 8 mA for SuperSpeed devices.
    let unit = if bcd_usb >= 0x0300 { 8 } else { 2 };
    w.u16(b.n(&["wTotalLength"]))
        .u8(b.n(&["bNumInterfaces"]))
        .u8(b.n(&["bConfigurationValue"]))
        .u8(b.n(&["iConfiguration"]))
        .u8(b.n(&["bmAttributes"]))
        .u8(b.n(&["MaxPower", "bMaxPower"]) / unit);
    w.finish(b)
}

fn iad(b: &Block) -> Vec<u8> {
    let mut w = W::new(b, dtype::INTERFACE_ASSOCIATION);
    w.u8(b.n(&["bFirstInterface"]))
        .u8(b.n(&["bInterfaceCount"]))
        .u8(b.n(&["bFunctionClass"]))
        .u8(b.n(&["bFunctionSubClass"]))
        .u8(b.n(&["bFunctionProtocol"]))
        .u8(b.n(&["iFunction"]));
    w.finish(b)
}

fn interface(b: &Block) -> Vec<u8> {
    let mut w = W::new(b, dtype::INTERFACE);
    w.u8(b.n(&["bInterfaceNumber"]))
        .u8(b.n(&["bAlternateSetting"]))
        .u8(b.n(&["bNumEndpoints"]))
        .u8(b.n(&["bInterfaceClass"]))
        .u8(b.n(&["bInterfaceSubClass"]))
        .u8(b.n(&["bInterfaceProtocol"]))
        .u8(b.n(&["iInterface"]));
    w.finish(b)
}

fn endpoint(b: &Block) -> Vec<u8> {
    let mut w = W::new(b, dtype::ENDPOINT);
    w.u8(b.n(&["bEndpointAddress", "bEndPointAddress"]))
        .u8(b.n(&["bmAttributes"]))
        .u16(b.n(&["wMaxPacketSize"]))
        .u8(b.n(&["bInterval"]));
    if b.has(&["bRefresh"]) || b.has(&["bSynchAddress"]) {
        w.u8(b.n(&["bRefresh"])).u8(b.n(&["bSynchAddress"]));
    }
    w.finish(b)
}

fn ss_companion(b: &Block, ep_attributes: u8) -> Vec<u8> {
    let mut w = W::new(b, dtype::SS_ENDPOINT_COMPANION);
    let attributes = match b.val(&["bmAttributes"]) {
        Some(v) => parse_num(v).unwrap_or(0),
        None => match ep_attributes & 3 {
            // Bulk: MaxStreams is printed as 2^n (0 when unsupported).
            2 => {
                let s = b.n(&["MaxStreams"]);
                if s > 1 { s.trailing_zeros() as u64 } else { 0 }
            }
            1 => b.n(&["Mult"]) & 3,
            _ => 0,
        },
    };
    w.u8(b.n(&["bMaxBurst"]))
        .u8(attributes)
        .u16(b.n(&["wBytesPerInterval"]));
    w.finish(b)
}

fn ssp_companion(b: &Block) -> Vec<u8> {
    let mut w = W::new(b, dtype::SSP_ISOC_ENDPOINT_COMPANION);
    w.u16(b.n(&["wReserved"])).u32(b.n(&["dwBytesPerInterval"]));
    w.finish(b)
}

fn dfu(b: &Block) -> Vec<u8> {
    let mut w = W::new(b, dtype::CS_DEVICE);
    w.u8(b.n(&["bmAttributes"]))
        .u16(b.n(&["wDetachTimeout"]))
        .u16(b.n(&["wTransferSize"]))
        .u16(b.bcd(&["bcdDFUVersion"]));
    w.finish(b)
}

fn hid(b: &Block) -> Vec<u8> {
    let mut w = W::new(b, dtype::CS_DEVICE);
    w.u16(b.bcd(&["bcdHID"]))
        .u8(b.n(&["bCountryCode"]))
        .u8(b.n(&["bNumDescriptors"]));
    // bDescriptorType repeats: the first is the HID descriptor's own.
    let types = b.all("bDescriptorType");
    let lens = b.all("wDescriptorLength");
    for (t, l) in types.iter().skip(1).zip(&lens) {
        w.u8(parse_num(t).unwrap_or(0))
            .u16(parse_num(l).unwrap_or(0));
    }
    w.finish(b)
}

fn vc_interrupt_endpoint(b: &Block) -> Vec<u8> {
    let mut w = W::new(b, dtype::CS_ENDPOINT);
    w.u8(b.subtype()).u16(b.n(&["wMaxTransferSize"]));
    w.finish(b)
}

fn video_control(b: &Block) -> Result<Vec<u8>, String> {
    let mut w = W::new(b, dtype::CS_INTERFACE);
    w.u8(b.subtype());
    match b.subtype() {
        1 => {
            w.u16(b.bcd(&["bcdUVC"]))
                .u16(b.n(&["wTotalLength"]))
                .u32(clock_frequency(b))
                .u8(b.n(&["bInCollection"]));
            for v in b.indexed("baInterfaceNr") {
                w.u8(parse_num(v).unwrap_or(0));
            }
        }
        2 => {
            w.u8(b.n(&["bTerminalID"]))
                .u16(b.n(&["wTerminalType"]))
                .u8(b.n(&["bAssocTerminal"]))
                .u8(b.n(&["iTerminal"]));
            if b.has(&["wObjectiveFocalLengthMin"]) {
                w.u16(b.n(&["wObjectiveFocalLengthMin"]))
                    .u16(b.n(&["wObjectiveFocalLengthMax"]))
                    .u16(b.n(&["wOcularFocalLength"]));
            }
            if b.has(&["bControlSize"]) {
                let size = b.n(&["bControlSize"]) as usize;
                w.u8(size as u64).bytes(&b.bitmap("bmControls", size));
                if b.has(&["bTransportModeSize"]) {
                    let ms = b.n(&["bTransportModeSize"]) as usize;
                    w.u8(ms as u64).bytes(&b.bitmap("bmTransportModes", ms));
                }
            }
        }
        3 => {
            w.u8(b.n(&["bTerminalID"]))
                .u16(b.n(&["wTerminalType"]))
                .u8(b.n(&["bAssocTerminal"]))
                .u8(b.n(&["bSourceID"]))
                .u8(b.n(&["iTerminal"]));
        }
        4 => {
            let sources = b.indexed("baSourceID");
            w.u8(b.n(&["bUnitID"])).u8(b.n(&["bNrInPins", "bNrPins"]));
            for s in sources {
                w.u8(parse_num(s).unwrap_or(0));
            }
            w.u8(b.n(&["iSelector"]));
        }
        5 => {
            let size = b.n(&["bControlSize"]) as usize;
            w.u8(b.n(&["bUnitID"]))
                .u8(b.n(&["bSourceID"]))
                .u16(b.n(&["wMaxMultiplier"]))
                .u8(size as u64);
            w.bytes(&b.bitmap("bmControls", size))
                .u8(b.n(&["iProcessing"]))
                .u8(b.n(&["bmVideoStandards"]));
        }
        6 => {
            let size = b.n(&["bControlSize"]) as usize;
            w.u8(b.n(&["bUnitID"]));
            w.bytes(
                &b.val(&["guidExtensionCode"])
                    .and_then(|v| parse_guid(v, b.guid_swapped))
                    .unwrap_or([0; 16]),
            );
            w.u8(b.n(&["bNumControls", "bNumControl"]))
                .u8(b.n(&["bNrInPins", "bNrPins"]));
            for s in b.indexed("baSourceID") {
                w.u8(parse_num(s).unwrap_or(0));
            }
            w.u8(size as u64)
                .bytes(&b.bitmap("bmControls", size))
                .u8(b.n(&["iExtension"]));
        }
        7 => {
            let size = b.n(&["bControlSize"]) as usize;
            w.u8(b.n(&["bUnitID"]))
                .u8(b.n(&["bSourceID"]))
                .u8(b.n(&["iEncoding"]))
                .u8(size as u64);
            w.bytes(&b.bitmap("bmControls", size))
                .bytes(&b.bitmap("bmControlsRuntime", size));
        }
        _ => {
            return Err(b.err(&format!(
                "unsupported VideoControl descriptor subtype {}",
                b.subtype_name()
            )));
        }
    }
    Ok(w.finish(b))
}

fn push_intervals(w: &mut W, b: &Block) {
    let discrete = b.indexed("dwFrameInterval");
    if !discrete.is_empty() {
        for v in discrete {
            w.u32(parse_num(v).unwrap_or(0));
        }
    } else if b.has(&["dwMinFrameInterval"]) {
        w.u32(b.n(&["dwMinFrameInterval"]))
            .u32(b.n(&["dwMaxFrameInterval"]))
            .u32(b.n(&["dwFrameIntervalStep"]));
    }
}

fn video_streaming(b: &Block) -> Result<Vec<u8>, String> {
    let mut w = W::new(b, dtype::CS_INTERFACE);
    w.u8(b.subtype());
    let ep = ["bEndpointAddress", "bEndPointAddress"];
    let format_common = |w: &mut W| {
        w.u8(b.n(&["bDefaultFrameIndex"]))
            .u8(b.n(&["bAspectRatioX"]))
            .u8(b.n(&["bAspectRatioY"]))
            .u8(b.n(&["bmInterlaceFlags"]))
            .u8(b.n(&["bCopyProtect"]));
    };
    let guid = |w: &mut W| {
        w.bytes(
            &b.val(&["guidFormat"])
                .and_then(|v| parse_guid(v, b.guid_swapped))
                .unwrap_or([0; 16]),
        );
    };
    match b.subtype() {
        1 | 2 => {
            let input = b.subtype() == 1;
            let size = b.n(&["bControlSize"]) as usize;
            w.u8(b.n(&["bNumFormats"]))
                .u16(b.n(&["wTotalLength"]))
                .u8(b.n(&ep));
            if input {
                w.u8(b.n(&["bmInfo"]));
            }
            w.u8(b.n(&["bTerminalLink"]));
            if input {
                w.u8(b.n(&["bStillCaptureMethod"]))
                    .u8(b.n(&["bTriggerSupport"]))
                    .u8(b.n(&["bTriggerUsage"]));
            }
            w.u8(size as u64);
            for v in b.indexed("bmaControls") {
                let mut c = parse_num(v).unwrap_or(0).to_le_bytes().to_vec();
                c.resize(size, 0);
                w.bytes(&c);
            }
        }
        3 => {
            w.u8(b.n(&ep)).u8(b.n(&["bNumImageSizePatterns"]));
            let (ws, hs) = (b.indexed("wWidth"), b.indexed("wHeight"));
            for (x, y) in ws.iter().zip(&hs) {
                w.u16(parse_num(x).unwrap_or(0))
                    .u16(parse_num(y).unwrap_or(0));
            }
            w.u8(b.n(&["bNumCompressionPatterns"]));
            for c in b.indexed("bCompression") {
                w.u8(parse_num(c).unwrap_or(0));
            }
        }
        4 => {
            w.u8(b.n(&["bFormatIndex"]))
                .u8(b.n(&["bNumFrameDescriptors"]));
            guid(&mut w);
            w.u8(b.n(&["bBitsPerPixel"]));
            format_common(&mut w);
        }
        5 | 7 => {
            w.u8(b.n(&["bFrameIndex"]))
                .u8(b.n(&["bmCapabilities"]))
                .u16(b.n(&["wWidth"]))
                .u16(b.n(&["wHeight"]));
            w.u32(b.n(&["dwMinBitRate"]))
                .u32(b.n(&["dwMaxBitRate"]))
                .u32(b.n(&["dwMaxVideoFrameBufferSize"]));
            w.u32(b.n(&["dwDefaultFrameInterval"]))
                .u8(b.n(&["bFrameIntervalType"]));
            push_intervals(&mut w, b);
        }
        6 => {
            w.u8(b.n(&["bFormatIndex"]))
                .u8(b.n(&["bNumFrameDescriptors"]))
                .u8(b.n(&["bFlags"]));
            format_common(&mut w);
        }
        10 => {
            w.u8(b.n(&["bFormatIndex"]))
                .u8(b.n(&["bDataOffset"]))
                .u8(b.n(&["bPacketLength"]))
                .u8(b.n(&["bStrideLength"]));
            w.bytes(
                &b.val(&["guidStrideFormat"])
                    .and_then(|v| parse_guid(v, b.guid_swapped))
                    .unwrap_or([0; 16]),
            );
        }
        13 => {
            w.u8(b.n(&["bColorPrimaries"]))
                .u8(b.n(&["bTransferCharacteristics"]))
                .u8(b.n(&["bMatrixCoefficients"]));
        }
        16 => {
            w.u8(b.n(&["bFormatIndex"]))
                .u8(b.n(&["bNumFrameDescriptors"]));
            guid(&mut w);
            w.u8(b.n(&["bBitsPerPixel"]));
            format_common(&mut w);
            w.u8(b.n(&["bVariableSize"]));
        }
        17 => {
            w.u8(b.n(&["bFrameIndex"]))
                .u8(b.n(&["bmCapabilities"]))
                .u16(b.n(&["wWidth"]))
                .u16(b.n(&["wHeight"]));
            w.u32(b.n(&["dwMinBitRate"]))
                .u32(b.n(&["dwMaxBitRate"]))
                .u32(b.n(&["dwDefaultFrameInterval"]));
            w.u8(b.n(&["bFrameIntervalType"]))
                .u32(b.n(&["dwBytesPerLine"]));
            push_intervals(&mut w, b);
        }
        _ => {
            return Err(b.err(&format!(
                "unsupported VideoStreaming descriptor subtype {}",
                b.subtype_name()
            )));
        }
    }
    Ok(w.finish(b))
}

/// Audio class 1.0 (what UVC webcams with microphones carry).
fn audio_control(b: &Block) -> Result<Vec<u8>, String> {
    let mut w = W::new(b, dtype::CS_INTERFACE);
    w.u8(b.subtype());
    match b.subtype() {
        1 => {
            if b.bcd(&["bcdADC"]) >= 0x0200 {
                return Err(b.err("audio class 2.0 descriptors are not supported"));
            }
            w.u16(b.bcd(&["bcdADC"]))
                .u16(b.n(&["wTotalLength"]))
                .u8(b.n(&["bInCollection"]));
            for v in b.indexed("baInterfaceNr") {
                w.u8(parse_num(v).unwrap_or(0));
            }
        }
        2 => {
            w.u8(b.n(&["bTerminalID"]))
                .u16(b.n(&["wTerminalType"]))
                .u8(b.n(&["bAssocTerminal"]))
                .u8(b.n(&["bNrChannels"]));
            w.u16(b.n(&["wChannelConfig"]))
                .u8(b.n(&["iChannelNames"]))
                .u8(b.n(&["iTerminal"]));
        }
        3 => {
            w.u8(b.n(&["bTerminalID"]))
                .u16(b.n(&["wTerminalType"]))
                .u8(b.n(&["bAssocTerminal"]))
                .u8(b.n(&["bSourceID"]))
                .u8(b.n(&["iTerminal"]));
        }
        6 => {
            let size = b.n(&["bControlSize"]);
            w.u8(b.n(&["bUnitID"])).u8(b.n(&["bSourceID"])).u8(size);
            for v in b.indexed("bmaControls") {
                let mut c = parse_num(v).unwrap_or(0).to_le_bytes().to_vec();
                c.resize(size as usize, 0);
                w.bytes(&c);
            }
            w.u8(b.n(&["iFeature"]));
        }
        _ => {
            return Err(b.err(&format!(
                "unsupported AudioControl descriptor subtype {}",
                b.subtype_name()
            )));
        }
    }
    Ok(w.finish(b))
}

fn audio_streaming(b: &Block) -> Result<Vec<u8>, String> {
    let mut w = W::new(b, dtype::CS_INTERFACE);
    w.u8(b.subtype());
    match b.subtype() {
        1 => {
            w.u8(b.n(&["bTerminalLink"]))
                .u8(b.n(&["bDelay"]))
                .u16(b.n(&["wFormatTag"]));
        }
        2 => {
            if b.n(&["bFormatType"]) != 1 {
                return Err(b.err(&format!(
                    "unsupported audio format type {}",
                    b.val(&["bFormatType"]).unwrap_or("?")
                )));
            }
            w.u8(b.n(&["bFormatType"]))
                .u8(b.n(&["bNrChannels"]))
                .u8(b.n(&["bSubframeSize"]))
                .u8(b.n(&["bBitResolution"]));
            w.u8(b.n(&["bSamFreqType"]));
            if b.has(&["tLowerSamFreq"]) {
                w.u24(b.n(&["tLowerSamFreq"])).u24(b.n(&["tUpperSamFreq"]));
            }
            for v in b.indexed("tSamFreq") {
                w.u24(parse_num(v).unwrap_or(0));
            }
        }
        _ => {
            return Err(b.err(&format!(
                "unsupported AudioStreaming descriptor subtype {}",
                b.subtype_name()
            )));
        }
    }
    Ok(w.finish(b))
}

fn audio_endpoint(b: &Block) -> Vec<u8> {
    let mut w = W::new(b, dtype::CS_ENDPOINT);
    w.u8(b.subtype())
        .u8(b.n(&["bmAttributes"]))
        .u8(b.n(&["bLockDelayUnits"]))
        .u16(b.n(&["wLockDelay"]));
    w.finish(b)
}

// BOS capabilities --------------------------------------------------------

fn capability(b: &Block, kind: &str) -> Option<Vec<u8>> {
    let mut w = W::new(b, dtype::DEVICE_CAPABILITY);
    let ty = b.n(&["bDevCapabilityType"]);
    w.u8(ty);
    match kind {
        "usb2ext" => {
            w.u32(b.n(&["bmAttributes"]));
        }
        "ss" => {
            w.u8(b.n(&["bmAttributes"]))
                .u16(b.n(&["wSpeedsSupported"]))
                .u8(b.n(&["bFunctionalitySupport"]));
            w.u8(b.n(&["bU1DevExitLat"]))
                .u16(b.n(&["wU2DevExitLat", "bU2DevExitLat"]));
        }
        "ssp" => {
            w.u8(b.n(&["bReserved"]))
                .u32(b.n(&["bmAttributes"]))
                .u16(b.n(&["wFunctionalitySupport", "bFunctionalitySupport"]))
                .u16(b.n(&["wReserved"]));
            for v in b.indexed("bmSublinkSpeedAttr") {
                w.u32(parse_num(v).unwrap_or(0));
            }
        }
        "container" => {
            w.u8(b.n(&["bReserved"]));
            w.bytes(
                &b.val(&["ContainerID", "ContainerUUID"])
                    .and_then(|v| parse_guid(v, b.guid_swapped))
                    .unwrap_or([0; 16]),
            );
        }
        "platform" => {
            w.u8(b.n(&["bReserved"]));
            w.bytes(
                &b.val(&["PlatformCapabilityUUID"])
                    .and_then(|v| parse_guid(v, b.guid_swapped))
                    .unwrap_or([0; 16]),
            );
            for v in b.indexed("CapabilityData") {
                w.u8(parse_num(v).unwrap_or(0));
            }
        }
        "fwstatus" => {
            w.u8(b.n(&["bcdDescriptorVersion"]))
                .u32(b.n(&["bmAttributes"]));
        }
        _ => return None,
    }
    Some(w.finish(b))
}

fn capability_kind(header: &str) -> Option<&'static str> {
    let h = header.to_ascii_lowercase();
    Some(if h.starts_with("usb 2.0 extension") {
        "usb2ext"
    } else if h.starts_with("superspeedplus")
        || h.starts_with("superspeed plus")
        || h.starts_with("superspeed+")
    {
        "ssp"
    } else if h.starts_with("superspeed") {
        "ss"
    } else if h.starts_with("container id") {
        "container"
    } else if h.starts_with("platform") {
        "platform"
    } else if h.starts_with("fwstatus") {
        "fwstatus"
    } else {
        return None;
    })
}

// ---------------------------------------------------------------------------
// Device assembly
// ---------------------------------------------------------------------------

const STRING_FIELDS: &[&str] = &[
    "imanufacturer",
    "iproduct",
    "iserial",
    "iserialnumber",
    "iconfiguration",
    "iinterface",
    "ifunction",
    "iterminal",
    "iprocessing",
    "iextension",
    "iselector",
    "iencoding",
    "ifeature",
    "ichannelnames",
];

#[derive(PartialEq, Clone, Copy)]
enum Scope {
    None,
    Config,
    Bos,
    Ignored,
}

fn collect_strings(b: &Block, d: &mut Descriptors) {
    for (name, value) in &b.fields {
        if !STRING_FIELDS.contains(&name.as_str()) {
            continue;
        }
        let Some(index) = parse_num(value).filter(|i| (1..=255).contains(i)) else {
            continue;
        };
        let text = value
            .trim_start()
            .split_once(char::is_whitespace)
            .map_or("", |(_, t)| t.strip_prefix(' ').unwrap_or(t));
        if !text.is_empty() {
            d.strings
                .entry(index as u8)
                .or_insert_with(|| text.to_string());
        }
    }
}

fn assemble(raw: RawDevice) -> Result<Option<LsusbDevice>, String> {
    let mut d = Descriptors {
        langids: vec![0x0409],
        ..Default::default()
    };
    let mut scope = Scope::None;
    let mut bcd_usb = 0;
    let mut have_device = false;
    let mut cur: Option<Vec<u8>> = None;
    let mut bos: Option<Vec<u8>> = None;
    let mut ep_attributes = 0u8;
    let mut pending_unknown: Option<String> = None;

    fn flush(cur: &mut Option<Vec<u8>>, d: &mut Descriptors) {
        if let Some(c) = cur.take() {
            d.configs.push(c);
        }
    }

    for b in &raw.blocks {
        if let Some(bytes) = &b.raw {
            pending_unknown = None;
            match scope {
                Scope::Config => cur
                    .as_mut()
                    .ok_or_else(|| b.err("descriptor outside a configuration"))?
                    .extend_from_slice(bytes),
                Scope::Bos => bos
                    .as_mut()
                    .ok_or_else(|| b.err("descriptor outside the BOS"))?
                    .extend_from_slice(bytes),
                _ => {}
            }
            continue;
        }
        if let Some(h) = pending_unknown.take() {
            return Err(format!("{h}: unsupported device capability (no hex dump)"));
        }
        let header = b.header.as_str();
        let top = header.starts_with('^');
        let name = header.trim_start_matches('^');
        // Scope switches.
        match name {
            "Device Descriptor" => {
                flush(&mut cur, &mut d);
                scope = Scope::None;
                d.device = device_descriptor(b);
                bcd_usb = b.bcd(&["bcdUSB"]);
                have_device = true;
                collect_strings(b, &mut d);
                continue;
            }
            "Configuration Descriptor" => {
                flush(&mut cur, &mut d);
                scope = Scope::Config;
                cur = Some(config_header(b, bcd_usb));
                collect_strings(b, &mut d);
                continue;
            }
            "Binary Object Store Descriptor" => {
                flush(&mut cur, &mut d);
                scope = Scope::Bos;
                let mut w = W::new(b, dtype::BOS);
                w.u16(b.n(&["wTotalLength"])).u8(b.n(&["bNumDeviceCaps"]));
                bos = Some(w.finish(b));
                continue;
            }
            _ => {}
        }
        if top
            || name.starts_with("Device Qualifier")
            || name.starts_with("Other Speed")
            || name.starts_with("Hub Descriptor")
        {
            flush(&mut cur, &mut d);
            scope = Scope::Ignored;
            continue;
        }
        if name == "Report Descriptors" {
            continue;
        }
        match scope {
            Scope::None | Scope::Ignored => continue,
            Scope::Bos => {
                let out = bos
                    .as_mut()
                    .ok_or_else(|| b.err("capability outside the BOS"))?;
                match capability_kind(name).and_then(|k| capability(b, k)) {
                    Some(bytes) => out.extend_from_slice(&bytes),
                    None => pending_unknown = Some(format!("line {}: {name}", b.line)),
                }
            }
            Scope::Config => {
                collect_strings(b, &mut d);
                let bytes = match name {
                    "Interface Association" => iad(b),
                    "Interface Descriptor" => interface(b),
                    "Endpoint Descriptor" => {
                        let e = endpoint(b);
                        ep_attributes = e.get(3).copied().unwrap_or(0);
                        e
                    }
                    "SuperSpeed Endpoint Companion Descriptor" => ss_companion(b, ep_attributes),
                    "SuperSpeedPlus Isochronous Endpoint Companion Descriptor" => ssp_companion(b),
                    "VideoControl Interface Descriptor" => video_control(b)?,
                    "VideoStreaming Interface Descriptor" => video_streaming(b)?,
                    "VideoControl Endpoint Descriptor" => vc_interrupt_endpoint(b),
                    "AudioControl Interface Descriptor" => audio_control(b)?,
                    "AudioStreaming Interface Descriptor" => audio_streaming(b)?,
                    "AudioControl Endpoint Descriptor" => audio_endpoint(b),
                    "Device Firmware Upgrade Interface Descriptor" => dfu(b),
                    "HID Device Descriptor" => hid(b),
                    _ => return Err(b.err("unsupported descriptor (lsusb decodes it but it can't be rebuilt; no hex dump)")),
                };
                cur.as_mut()
                    .ok_or_else(|| b.err("descriptor outside a configuration"))?
                    .extend_from_slice(&bytes);
            }
        }
    }
    if let Some(h) = pending_unknown {
        return Err(format!("{h}: unsupported device capability (no hex dump)"));
    }
    flush(&mut cur, &mut d);
    if !have_device {
        return Ok(None);
    }
    d.bos = bos;
    Ok(Some(LsusbDevice {
        bus: raw.bus,
        address: raw.address,
        speed: raw.speed,
        descriptors: d,
    }))
}

/// Parses `lsusb -v` text (any number of devices; devices without a
/// "Device Descriptor:" section are skipped).
pub fn parse(text: &str) -> Result<Vec<LsusbDevice>, String> {
    let mut out = Vec::new();
    for raw in tokenize(text) {
        let (bus, address) = (raw.bus, raw.address);
        if let Some(dev) =
            assemble(raw).map_err(|e| format!("Bus {bus:03} Device {address:03}: {e}"))?
        {
            out.push(dev);
        }
    }
    if out.is_empty() {
        return Err("no device found (expected `lsusb -v` output: \"Bus NNN Device NNN: ID ...\" followed by \"Device Descriptor:\")".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wraps a configuration body (indented like lsusb) into a minimal device.
    fn config_bytes(body: &str) -> Vec<u8> {
        let text = format!(
            "\nBus 001 Device 002: ID 1234:5678 Test\nDevice Descriptor:\n  bLength 18\n  bDescriptorType 1\n  bcdUSB 2.00\n  bNumConfigurations 1\n  Configuration Descriptor:\n    bLength 9\n    bDescriptorType 2\n    wTotalLength 100\n    bNumInterfaces 1\n    bConfigurationValue 1\n    iConfiguration 0\n    bmAttributes 0x80\n    MaxPower 100mA\n{body}"
        );
        let devs = parse(&text).unwrap();
        devs[0].descriptors.configs[0][9..].to_vec()
    }

    #[test]
    fn processing_unit_layouts() {
        // UVC 1.1+: bmVideoStandards present (bLength = 10 + size).
        let v11 = config_bytes(
            "      VideoControl Interface Descriptor:\n        bLength 13\n        bDescriptorType 36\n        bDescriptorSubtype 5 (PROCESSING_UNIT)\n        bUnitID 2\n        bSourceID 1\n        wMaxMultiplier 16384\n        bControlSize 3\n        bmControls 0x0000157f\n          Brightness\n        iProcessing 0\n        bmVideoStandards 0x1f\n",
        );
        assert_eq!(
            v11,
            [13, 0x24, 5, 2, 1, 0x00, 0x40, 3, 0x7f, 0x15, 0x00, 0, 0x1f]
        );
        // UVC 1.0 (bLength = 9 + size): lsusb warns and prints a bogus
        // bmVideoStandards from beyond the descriptor; it must not be emitted.
        let v10 = config_bytes(
            "      VideoControl Interface Descriptor:\n        bLength 11\n        bDescriptorType 36\n        bDescriptorSubtype 5 (PROCESSING_UNIT)\n      Warning: Descriptor too short\n        bUnitID 2\n        bSourceID 1\n        wMaxMultiplier 0\n        bControlSize 2\n        bmControls 0x00000039\n          Brightness\n        iProcessing 0\n        bmVideoStandards 0x 9\n          None\n",
        );
        assert_eq!(v10, [11, 0x24, 5, 2, 1, 0, 0, 2, 0x39, 0x00, 0]);
    }

    #[test]
    fn frame_intervals_discrete_and_continuous() {
        let frame = |tail: &str, len: u32| {
            config_bytes(&format!(
                "      VideoStreaming Interface Descriptor:\n        bLength {len}\n        bDescriptorType 36\n        bDescriptorSubtype 7 (FRAME_MJPEG)\n        bFrameIndex 1\n        bmCapabilities 0x01\n        wWidth 640\n        wHeight 480\n        dwMinBitRate 1\n        dwMaxBitRate 2\n        dwMaxVideoFrameBufferSize 614400\n        dwDefaultFrameInterval 333333\n{tail}"
            ))
        };
        let discrete = frame(
            "        bFrameIntervalType 2\n        dwFrameInterval( 0) 333333\n        dwFrameInterval( 1) 666666\n",
            34,
        );
        assert_eq!(discrete.len(), 34);
        assert_eq!(&discrete[25..30], &[2, 0x15, 0x16, 0x05, 0x00]);
        assert_eq!(&discrete[30..], &666666u32.to_le_bytes());
        let continuous = frame(
            "        bFrameIntervalType 0\n        dwMinFrameInterval 333333\n        dwMaxFrameInterval 2000000\n        dwFrameIntervalStep 1000\n",
            38,
        );
        assert_eq!(continuous.len(), 38);
        assert_eq!(continuous[25], 0);
        assert_eq!(&continuous[26..30], &333333u32.to_le_bytes());
        assert_eq!(&continuous[30..34], &2000000u32.to_le_bytes());
        assert_eq!(&continuous[34..], &1000u32.to_le_bytes());
    }

    #[test]
    fn frame_based_has_bytes_per_line() {
        let b = config_bytes(
            "      VideoStreaming Interface Descriptor:\n        bLength 30\n        bDescriptorType 36\n        bDescriptorSubtype 17 (FRAME_FRAME_BASED)\n        bFrameIndex 3\n        bmCapabilities 0x00\n        wWidth 160\n        wHeight 90\n        dwMinBitRate 10\n        dwMaxBitRate 20\n        dwDefaultFrameInterval 333333\n        bFrameIntervalType 1\n        dwBytesPerLine 320\n        dwFrameInterval( 0) 333333\n",
        );
        assert_eq!(b.len(), 30);
        assert_eq!(&b[21..26], &[1, 0x40, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn unrecognized_hex_blocks_are_copied_in_place() {
        let b = config_bytes(
            "      ** UNRECOGNIZED:  04 41 01 02\n      Endpoint Descriptor:\n        bLength 7\n        bDescriptorType 5\n        bEndpointAddress 0x81  EP 1 IN\n        bmAttributes 3\n        wMaxPacketSize 0x0008  1x 8 bytes\n        bInterval 10\n        ** UNRECOGNIZED:  02 ff\n",
        );
        assert_eq!(b, [4, 0x41, 1, 2, 7, 5, 0x81, 3, 8, 0, 10, 2, 0xff]);
    }

    #[test]
    fn printed_lengths_win_over_computed_ones() {
        // bNumFormats says 5, only one bmaControls is printed, wTotalLength is bogus.
        let b = config_bytes(
            "      VideoStreaming Interface Descriptor:\n        bLength 14\n        bDescriptorType 36\n        bDescriptorSubtype 1 (INPUT_HEADER)\n        bNumFormats 5\n        wTotalLength 9999\n        bEndPointAddress 130\n        bmInfo 0\n        bTerminalLink 3\n        bStillCaptureMethod 0\n        bTriggerSupport 0\n        bTriggerUsage 0\n        bControlSize 1\n        bmaControls( 0) 27\n",
        );
        assert_eq!(b, [14, 0x24, 1, 5, 0x0f, 0x27, 0x82, 0, 3, 0, 0, 0, 1, 27]);
    }

    #[test]
    fn unsupported_descriptor_is_an_error() {
        let text = "Bus 001 Device 002: ID 1234:5678 x\nDevice Descriptor:\n  bLength 18\n  Configuration Descriptor:\n    bLength 9\n    Interface Descriptor:\n      bLength 9\n      MIDIStreaming Interface Descriptor:\n        bLength 7\n";
        let e = parse(text).unwrap_err();
        assert!(e.contains("MIDIStreaming Interface Descriptor"), "{e}");
        let text = "Bus 001 Device 002: ID 1234:5678 x\nDevice Descriptor:\n  bLength 18\n  Configuration Descriptor:\n    bLength 9\n    Interface Descriptor:\n      bLength 9\n      VideoControl Interface Descriptor:\n        bLength 5\n        bDescriptorSubtype 9 (WEIRD)\n";
        assert!(parse(text).unwrap_err().contains("subtype 9 (WEIRD)"));
        assert!(parse("nothing here").is_err());
    }

    #[test]
    fn bcd_hex_and_strings() {
        let text = "Bus 003 Device 009: ID 1234:5678 x\nNegotiated speed: SuperSpeed Plus (10Gbps)\nDevice Descriptor:\n  bLength 18\n  bDescriptorType 1\n  bcdUSB 3.10\n  bcdDevice 11.09\n  iManufacturer 1 Foo Inc\n  iProduct 2 \n  iSerial 3 01.00\n  bNumConfigurations 0\n";
        let d = &parse(text).unwrap()[0];
        assert_eq!((d.bus, d.address, d.speed), (3, 9, Some(Speed::SuperPlus)));
        assert_eq!(&d.descriptors.device[2..4], &[0x10, 0x03]);
        assert_eq!(&d.descriptors.device[12..14], &[0x09, 0x11]);
        assert_eq!(
            d.descriptors.strings.get(&1).map(String::as_str),
            Some("Foo Inc")
        );
        assert_eq!(d.descriptors.strings.get(&2), None);
        assert_eq!(
            d.descriptors.strings.get(&3).map(String::as_str),
            Some("01.00")
        );
    }
}
