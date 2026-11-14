//! Malformed or unusual descriptors, on top of any model.
//!
//! A patch edits the configuration descriptor the host reads. The device keeps
//! working from its original descriptors, so a test shows how the host copes
//! with descriptors that disagree with the device. Patches target a video
//! class-specific (CS_INTERFACE) descriptor by interface kind, subtype and
//! occurrence:
//!
//! ```toml
//! [[device.patch]]                 # the first MJPEG format claims one frame more
//! target = { iface = "vs", subtype = 0x06 }
//! set = [[4, 4]]                   # [offset, byte]: bNumFrameDescriptors = 4
//!
//! [[device.patch]]                 # unknown subtype before the second format
//! target = { iface = "vs", subtype = 0x04, nth = 1 }
//! insert_before = [5, 0x24, 0x20, 0, 0]
//! ```
//!
//! Operations run in this order:
//! 1. structural changes (`insert_before`, `duplicate`, `move_before`, `remove`), patch by patch;
//! 2. `wTotalLength` fix-ups (configuration, VC header, VS input/output headers) by the bytes
//!    added or removed, which keeps lengths that were already wrong just as wrong;
//! 3. `set` byte edits, so a patch can still break a length on purpose.

use serde::{Deserialize, Serialize};

use emuvc_usb::desc::dtype;

const CS_INTERFACE: u8 = 0x24;
const CC_VIDEO: u8 = 0x0e;
const SC_VIDEOCONTROL: u8 = 0x01;
const SC_VIDEOSTREAMING: u8 = 0x02;
const VC_HEADER: u8 = 0x01;
const VS_INPUT_HEADER: u8 = 0x01;
const VS_OUTPUT_HEADER: u8 = 0x02;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IfaceKind {
    Vc,
    Vs,
}

/// The `nth` (0-based) CS_INTERFACE descriptor of `subtype` in VideoControl
/// or VideoStreaming interfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub iface: IfaceKind,
    pub subtype: u8,
    #[serde(default)]
    pub nth: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DescriptorPatch {
    pub target: Option<Target>,
    /// Byte edits `[offset, value]` within the target (offset 0 is bLength).
    pub set: Vec<(usize, u8)>,
    /// Raw descriptor bytes inserted before the target.
    pub insert_before: Option<Vec<u8>>,
    /// A copy of the target right after it.
    pub duplicate: bool,
    /// Moves the target before another descriptor.
    pub move_before: Option<Target>,
    pub remove: bool,
}

/// One descriptor of a configuration, with the video interface it belongs to.
#[derive(Clone)]
struct Desc {
    bytes: Vec<u8>,
    iface: Option<IfaceKind>,
    /// Index (in the original configuration) of the interface descriptor it follows.
    owner: usize,
    /// Byte edits, applied after the wTotalLength fix-ups.
    set: Vec<(usize, u8)>,
}

fn split(config: &[u8]) -> Result<Vec<Desc>, String> {
    let (mut out, mut pos, mut iface, mut owner) = (Vec::new(), 0, None, 0);
    while pos < config.len() {
        let len = config[pos] as usize;
        if len < 2 || pos + len > config.len() {
            return Err(format!("descriptor at offset {pos}: bad bLength {len}"));
        }
        let b = &config[pos..pos + len];
        if b[1] == dtype::INTERFACE && len >= 9 {
            iface = match (b[5], b[6]) {
                (CC_VIDEO, SC_VIDEOCONTROL) => Some(IfaceKind::Vc),
                (CC_VIDEO, SC_VIDEOSTREAMING) => Some(IfaceKind::Vs),
                _ => None,
            };
            owner = out.len();
        }
        out.push(Desc {
            bytes: b.to_vec(),
            iface,
            owner,
            set: Vec::new(),
        });
        pos += len;
    }
    Ok(out)
}

fn is_cs(d: &Desc) -> bool {
    d.iface.is_some() && d.bytes.len() >= 3 && d.bytes[1] == CS_INTERFACE
}

fn find(descs: &[Desc], t: &Target) -> Result<usize, String> {
    descs
        .iter()
        .enumerate()
        .filter(|(_, d)| is_cs(d) && d.iface == Some(t.iface) && d.bytes[2] == t.subtype)
        .nth(t.nth)
        .map(|(i, _)| i)
        .ok_or_else(|| format!("patch target not found: {t:?}"))
}

/// wTotalLength offset of a header covering its interface's class-specific descriptors.
fn total_length_offset(d: &Desc) -> Option<usize> {
    if !is_cs(d) {
        return None;
    }
    match (d.iface?, d.bytes[2]) {
        (IfaceKind::Vc, VC_HEADER) if d.bytes.len() >= 7 => Some(5),
        (IfaceKind::Vs, VS_INPUT_HEADER | VS_OUTPUT_HEADER) if d.bytes.len() >= 6 => Some(4),
        _ => None,
    }
}

/// Bytes of class-specific descriptors in the interface (alternate setting) `owner`.
fn cs_bytes(descs: &[Desc], owner: usize) -> usize {
    descs
        .iter()
        .filter(|d| d.owner == owner && is_cs(d))
        .map(|d| d.bytes.len())
        .sum()
}

/// Applies `patches` to one configuration descriptor.
pub fn apply(config: &mut Vec<u8>, patches: &[DescriptorPatch]) -> Result<(), String> {
    if patches.is_empty() {
        return Ok(());
    }
    let mut descs = split(config)?;
    let mut owners: Vec<usize> = descs.iter().map(|d| d.owner).collect();
    owners.dedup();
    let before: Vec<usize> = owners.iter().map(|&o| cs_bytes(&descs, o)).collect();

    for p in patches {
        let t = p.target.as_ref().ok_or("patch without target")?;
        let mut i = find(&descs, t)?;
        if let Some(raw) = &p.insert_before {
            let d = Desc {
                bytes: raw.clone(),
                set: Vec::new(),
                ..descs[i].clone()
            };
            descs.insert(i, d);
            i += 1;
        }
        if p.duplicate {
            let d = descs[i].clone();
            descs.insert(i + 1, d);
        }
        if let Some(to) = &p.move_before {
            let d = descs.remove(i);
            i = find(&descs, to)?;
            let (iface, owner) = (descs[i].iface, descs[i].owner);
            descs.insert(i, Desc { iface, owner, ..d });
        }
        if p.remove {
            descs.remove(i);
            continue;
        }
        if let Some(&(off, _)) = p.set.iter().find(|(off, _)| *off >= descs[i].bytes.len()) {
            return Err(format!("patch {t:?}: offset {off} past the descriptor"));
        }
        descs[i].set.extend(&p.set);
    }

    // Keep each header's wTotalLength as (in)consistent as it was.
    for (&owner, &was) in owners.iter().zip(&before) {
        let delta = cs_bytes(&descs, owner) as isize - was as isize;
        if delta == 0 {
            continue;
        }
        if let Some((d, off)) = descs
            .iter_mut()
            .filter(|d| d.owner == owner)
            .find_map(|d| total_length_offset(d).map(|off| (d, off)))
        {
            let v = u16::from_le_bytes([d.bytes[off], d.bytes[off + 1]]) as isize + delta;
            d.bytes[off..off + 2].copy_from_slice(&(v.clamp(0, 0xffff) as u16).to_le_bytes());
        }
    }

    let mut out = Vec::with_capacity(config.len());
    for d in &mut descs {
        for &(off, v) in &d.set {
            d.bytes[off] = v;
        }
        out.extend_from_slice(&d.bytes);
    }
    if out.len() >= 4 && out[1] == dtype::CONFIG {
        let total = out.len().min(0xffff) as u16;
        out[2..4].copy_from_slice(&total.to_le_bytes());
    }
    *config = out;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Config; VC interface, header (wTotalLength 13), input terminal; VS interface,
    /// input header (wTotalLength 20), format, 2 frames.
    fn sample() -> Vec<u8> {
        let mut c = vec![9, dtype::CONFIG, 0, 0, 2, 1, 0, 0x80, 50];
        c.extend([
            9,
            dtype::INTERFACE,
            0,
            0,
            0,
            CC_VIDEO,
            SC_VIDEOCONTROL,
            0,
            0,
        ]);
        c.extend([8, CS_INTERFACE, VC_HEADER, 0x10, 0x01, 13, 0, 0]);
        c.extend([5, CS_INTERFACE, 0x02, 1, 0]);
        c.extend([
            9,
            dtype::INTERFACE,
            1,
            0,
            0,
            CC_VIDEO,
            SC_VIDEOSTREAMING,
            0,
            0,
        ]);
        c.extend([6, CS_INTERFACE, VS_INPUT_HEADER, 1, 20, 0]);
        c.extend([4, CS_INTERFACE, 0x06, 1]);
        c.extend([5, CS_INTERFACE, 0x07, 1, 0xaa]);
        c.extend([5, CS_INTERFACE, 0x07, 2, 0xbb]);
        let n = c.len() as u16;
        c[2..4].copy_from_slice(&n.to_le_bytes());
        c
    }

    fn patch(src: &str) -> Vec<u8> {
        #[derive(Deserialize)]
        struct P {
            patch: Vec<DescriptorPatch>,
        }
        let p: P = toml::from_str(src).unwrap();
        let mut c = sample();
        apply(&mut c, &p.patch).unwrap();
        c
    }

    fn vs_subtypes(c: &[u8]) -> Vec<u8> {
        split(c)
            .unwrap()
            .iter()
            .filter(|d| d.iface == Some(IfaceKind::Vs) && is_cs(d))
            .map(|d| d.bytes[2])
            .collect()
    }

    const VC_TOTAL: usize = 9 + 9 + 5;
    const VS_TOTAL: usize = 9 + 9 + 8 + 5 + 9 + 4;

    fn total(c: &[u8]) -> usize {
        u16::from_le_bytes([c[2], c[3]]) as usize
    }

    #[test]
    fn unchanged_without_patches() {
        let mut c = sample();
        apply(&mut c, &[]).unwrap();
        assert_eq!(c, sample());
        let c = patch("[[patch]]\ntarget = { iface = \"vs\", subtype = 6 }");
        assert_eq!(c, sample());
    }

    #[test]
    fn set_targets_nth() {
        let c = patch(
            "[[patch]]\ntarget = { iface = \"vs\", subtype = 7, nth = 1 }\nset = [[4, 0xcc]]",
        );
        assert_eq!(&c[c.len() - 5..], &[5, CS_INTERFACE, 7, 2, 0xcc]);
        assert_eq!(c[VS_TOTAL], 20);
        assert_eq!(total(&c), c.len());
    }

    #[test]
    fn structural_changes_fix_lengths() {
        let c = patch(
            "[[patch]]\ntarget = { iface = \"vs\", subtype = 6 }\ninsert_before = [5, 0x24, 0x20, 0, 0]\n\
             [[patch]]\ntarget = { iface = \"vs\", subtype = 7 }\nduplicate = true\n\
             [[patch]]\ntarget = { iface = \"vc\", subtype = 2 }\nremove = true",
        );
        assert_eq!(vs_subtypes(&c), [1, 0x20, 6, 7, 7, 7]);
        assert_eq!(c[VC_TOTAL], 13 - 5);
        assert_eq!(c[VS_TOTAL - 5], 20 + 5 + 5);
        assert_eq!(c.len(), sample().len() + 5);
        assert_eq!(total(&c), c.len());
    }

    #[test]
    fn move_then_explicit_total_length() {
        let c = patch(
            "[[patch]]\ntarget = { iface = \"vs\", subtype = 7, nth = 1 }\nmove_before = { iface = \"vs\", subtype = 6 }\n\
             [[patch]]\ntarget = { iface = \"vs\", subtype = 1 }\nset = [[4, 0xff], [5, 0]]",
        );
        assert_eq!(vs_subtypes(&c), [1, 7, 6, 7]);
        assert_eq!(c[VS_TOTAL], 0xff);
    }

    #[test]
    fn errors() {
        let mut c = sample();
        let target = Some(Target {
            iface: IfaceKind::Vs,
            subtype: 0x10,
            nth: 0,
        });
        let p = DescriptorPatch {
            target,
            ..Default::default()
        };
        assert!(apply(&mut c, &[p]).is_err());
        let target = Some(Target {
            iface: IfaceKind::Vs,
            subtype: 6,
            nth: 0,
        });
        let p = DescriptorPatch {
            target,
            set: vec![(4, 0)],
            ..Default::default()
        };
        assert!(apply(&mut c, &[p]).is_err());
        assert!(apply(&mut c, &[DescriptorPatch::default()]).is_err());
        assert_eq!(c, sample());
    }
}
