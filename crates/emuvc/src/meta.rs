//! Checks what uvcvideo's metadata node handed over (`v4l2-ctl --stream-to=FILE` on it), for a camera
//! whose metadata format copies whole payload headers (`V4L2_META_FMT_D4XX`): back-to-back
//! `struct uvc_meta_buf { u64 ns; u16 sof; u8 length; u8 flags; u8 buf[length - 2]; }` (packed), one
//! per payload. `length` and `flags` are bHeaderLength and bmHeaderInfo; `buf` holds the rest of the
//! header: PTS, SCR, then the metadata, which emuvc starts with the frame index.

const FID: u8 = 0x01;
const EOF: u8 = 0x02;
const PTS: u8 = 0x04;
const SCR: u8 = 0x08;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block<'a> {
    /// Host time (ns) when uvcvideo saw the payload.
    pub ns: u64,
    /// bmHeaderInfo.
    pub flags: u8,
    /// The header after bHeaderLength and bmHeaderInfo.
    pub buf: &'a [u8],
}

impl Block<'_> {
    pub fn pts(&self) -> Option<u32> {
        (self.flags & PTS != 0 && self.buf.len() >= 4)
            .then(|| u32::from_le_bytes(self.buf[..4].try_into().unwrap()))
    }

    /// The SCR: source clock and 11-bit SOF counter.
    pub fn scr(&self) -> Option<(u32, u16)> {
        let o = if self.flags & PTS != 0 { 4 } else { 0 };
        (self.flags & SCR != 0 && self.buf.len() >= o + 6).then(|| {
            let stc = u32::from_le_bytes(self.buf[o..o + 4].try_into().unwrap());
            (
                stc,
                u16::from_le_bytes([self.buf[o + 4], self.buf[o + 5]]) & 0x7ff,
            )
        })
    }

    /// The bytes after PTS and SCR.
    pub fn metadata(&self) -> &[u8] {
        let o =
            if self.flags & PTS != 0 { 4 } else { 0 } + if self.flags & SCR != 0 { 6 } else { 0 };
        self.buf.get(o..).unwrap_or(&[])
    }
}

pub fn parse(data: &[u8]) -> Result<Vec<Block<'_>>, String> {
    let mut blocks = Vec::new();
    let mut off = 0;
    while off < data.len() {
        if data.len() - off < 12 {
            return Err(format!("truncated block at byte {off}"));
        }
        let length = data[off + 10];
        if length < 2 {
            return Err(format!("bHeaderLength {length} at byte {off}"));
        }
        let end = off + 10 + length as usize;
        if end > data.len() {
            return Err(format!("the block at byte {off} runs past the end"));
        }
        blocks.push(Block {
            ns: u64::from_le_bytes(data[off..off + 8].try_into().unwrap()),
            flags: data[off + 11],
            buf: &data[off + 12..end],
        });
        off = end;
    }
    Ok(blocks)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub blocks: usize,
    /// Whole frames: the first and last runs of a FID value may be cut by the capture.
    pub frames: usize,
    pub with_metadata: usize,
    pub without_metadata: usize,
    pub errors: Vec<String>,
}

impl Report {
    fn err(&mut self, e: String) {
        if self.errors.len() < 20 {
            self.errors.push(e);
        }
    }
}

/// Every whole frame carries its own index as metadata, the same in all its payloads, or none. With
/// `metadata_every` N, frames whose index is N - 1 modulo N carry none (emuvc's `no_metadata_every`),
/// and the others do. A frame's payloads (up to the one with EOF) share their PTS, which grows from
/// frame to frame. With `clock_hz`, the SCR's clock agrees with its SOF counter (1 ms each), and a
/// frame's PTS precedes the SCR of its first payload by less than a second.
pub fn check(blocks: &[Block], metadata_every: Option<u32>, clock_hz: Option<u32>) -> Report {
    let mut r = Report {
        blocks: blocks.len(),
        ..Default::default()
    };
    for (i, w) in blocks.windows(2).enumerate() {
        if w[1].ns < w[0].ns {
            r.err(format!("block {}: host time goes back", i + 1));
        }
        if let (Some(hz), Some((a, sa)), Some((b, sb))) = (clock_hz, w[0].scr(), w[1].scr()) {
            let stc_ms = b.wrapping_sub(a) as f64 * 1000.0 / hz as f64;
            let sof_ms = sb.wrapping_sub(sa) & 0x7ff;
            if (stc_ms - sof_ms as f64).abs() > 1.0 {
                r.err(format!(
                    "block {}: SCR clock +{stc_ms:.3} ms, SOF +{sof_ms} ms",
                    i + 1
                ));
            }
        }
    }
    // Runs of one FID value: a frame's payloads up to EOF, then idle ones.
    let mut runs: Vec<&[Block]> = blocks
        .chunk_by(|a, b| (a.flags ^ b.flags) & FID == 0)
        .collect();
    if runs.len() < 3 {
        r.err(format!("only {} FID runs", runs.len()));
        return r;
    }
    runs.remove(0);
    runs.pop();
    let mut prev: Option<(u32, Option<u32>)> = None; // (PTS, index) of the last frame
    let mut skipped = 0; // frames without metadata since the last one with
    for run in runs {
        let n = run.len();
        let Some(end) = run.iter().position(|b| b.flags & EOF != 0) else {
            r.err(format!("a frame of {n} payloads without EOF"));
            continue;
        };
        let data = &run[..=end];
        r.frames += 1;
        let (pts, meta) = (data[0].pts(), data[0].metadata());
        if data.iter().any(|b| b.pts() != pts || b.metadata() != meta) {
            r.err(format!(
                "frame {}: PTS or metadata change within it",
                r.frames
            ));
            continue;
        }
        let index = (meta.len() >= 4).then(|| u32::from_le_bytes(meta[..4].try_into().unwrap()));
        match index {
            Some(_) => r.with_metadata += 1,
            None => r.without_metadata += 1,
        }
        if let (Some(hz), Some(p), Some((stc, _))) = (clock_hz, pts, data[0].scr()) {
            let ahead = stc.wrapping_sub(p) as i32;
            if ahead < 0 || ahead as f64 > hz as f64 {
                r.err(format!(
                    "frame {}: PTS {p} vs SCR clock {stc} of its first payload",
                    r.frames
                ));
            }
        }
        if let (Some((pp, _)), Some(p)) = (prev, pts)
            && p.wrapping_sub(pp) as i32 <= 0
        {
            r.err(format!("frame {}: PTS {p} after {pp}", r.frames));
        }
        if let Some(n) = metadata_every.filter(|&n| n > 0) {
            match index {
                Some(i) if i % n == n - 1 => {
                    r.err(format!("frame {}: index {i} carries metadata", r.frames))
                }
                Some(i) => {
                    if let Some((_, Some(last))) = prev {
                        let due = (last + 1..i).filter(|k| k % n == n - 1).count();
                        if i <= last || skipped > due {
                            let f = r.frames;
                            r.err(format!(
                                "frame {f}: index {i}, {skipped} frames without metadata since {last}"
                            ));
                        }
                    }
                    skipped = 0;
                }
                None => skipped += 1,
            }
        }
        prev = Some((pts.unwrap_or(0), index.or(prev.and_then(|p| p.1))));
    }
    if metadata_every.is_some() && (r.with_metadata == 0 || r.without_metadata == 0) {
        r.err(format!(
            "{} frames with metadata, {} without",
            r.with_metadata, r.without_metadata
        ));
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    const HZ: u32 = 48_000_000;

    /// One payload's block: emuvc's header at microframe `uf` (STC and SOF from it).
    fn block(out: &mut Vec<u8>, uf: u64, fid: bool, eof: bool, pts: u32, meta: Option<u32>) {
        let mut hdr = vec![
            0,
            0x80 | PTS | SCR | if fid { FID } else { 0 } | if eof { EOF } else { 0 },
        ];
        hdr.extend_from_slice(&pts.to_le_bytes());
        hdr.extend_from_slice(&((uf * 6000) as u32).to_le_bytes()); // 48 MHz: 6000 ticks per microframe
        hdr.extend_from_slice(&(((uf / 8) & 0x7ff) as u16).to_le_bytes());
        if let Some(i) = meta {
            hdr.extend_from_slice(&i.to_le_bytes());
            hdr.extend_from_slice(&[0xa5; 4]);
        }
        hdr[0] = hdr.len() as u8;
        out.extend_from_slice(&(uf * 125_000).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&hdr);
    }

    /// Frames `first..first + n`, 3 payloads and 2 idle ones each, metadata on the frames `meta` picks.
    fn stream(first: u32, n: u32, meta: impl Fn(u32) -> bool) -> Vec<u8> {
        let mut out = Vec::new();
        let mut uf = 1000;
        for i in first..first + n {
            let pts = (uf * 6000 - 100) as u32;
            for k in 0..5 {
                let m = (k < 3 && meta(i)).then_some(i);
                block(
                    &mut out,
                    uf,
                    i % 2 == 1,
                    k == 2,
                    if k < 3 { pts } else { 0 },
                    m,
                );
                uf += 1;
            }
        }
        out
    }

    #[test]
    fn every_other_frame() {
        let data = stream(10, 8, |i| i % 2 == 0);
        let b = parse(&data).unwrap();
        assert_eq!(b.len(), 40);
        assert_eq!(b[0].metadata(), [10, 0, 0, 0, 0xa5, 0xa5, 0xa5, 0xa5]);
        let r = check(&b, Some(2), Some(HZ));
        assert_eq!(r.errors, Vec::<String>::new());
        assert_eq!((r.frames, r.with_metadata, r.without_metadata), (6, 3, 3));
    }

    #[test]
    fn stale_or_missing_metadata() {
        // Every frame with metadata, though odd ones must have none.
        let data = stream(10, 8, |_| true);
        let r = check(&parse(&data).unwrap(), Some(2), Some(HZ));
        assert!(
            r.errors.iter().any(|e| e.contains("carries metadata")),
            "{r:?}"
        );
        // Two frames in a row without, where one was due.
        let data = stream(10, 8, |i| i % 4 == 0);
        let r = check(&parse(&data).unwrap(), Some(2), Some(HZ));
        assert!(
            r.errors
                .iter()
                .any(|e| e.contains("without metadata since")),
            "{r:?}"
        );
    }

    #[test]
    fn clock_and_layout() {
        let mut data = stream(0, 6, |_| false);
        let b = parse(&data).unwrap();
        assert_eq!(check(&b, None, Some(HZ)).errors, Vec::<String>::new());
        // An SCR clock that stops: 3 ms of SOF later, the same STC.
        let mut buf = b[7].buf.to_vec();
        buf[4..8].copy_from_slice(&b[6].buf[4..8]);
        buf[8..10].copy_from_slice(&(b[6].scr().unwrap().1 + 3).to_le_bytes());
        let mut stuck = b.clone();
        stuck[7].buf = &buf;
        let r = check(&stuck, None, Some(HZ));
        assert!(r.errors.iter().any(|e| e.contains("SCR clock")), "{r:?}");
        // A cut block.
        data.pop();
        assert!(parse(&data).is_err());
    }
}
