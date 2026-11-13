//! Renders frames on a worker thread, ahead of time.
//!
//! A real camera's sensor/ISP/encoder pipeline runs in parallel with USB.
//! Rendering synchronously on the emulator's bus thread would instead stall
//! the bus (a 1080p MJPEG frame takes milliseconds) and deliver frames in
//! bursts. [`Threaded`] renders frame k+1 (after a [`FrameSource::prefetch`]
//! hint) while frame k is being sent. Output is identical either way: a
//! frame only depends on its spec and index.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use crate::{FrameSource, FrameSpec};

type Key = (FrameSpec, u64);

pub struct Threaded {
    tx: Sender<(FrameSpec, u64, u64)>,
    rx: Receiver<(Key, Vec<u8>)>,
    /// Request sent to the worker whose result hasn't been collected.
    pending: Option<Key>,
}

impl Threaded {
    pub fn new(mut inner: Box<dyn FrameSource>) -> Self {
        let (req_tx, req_rx) = channel::<(FrameSpec, u64, u64)>();
        let (res_tx, res_rx) = channel();
        thread::Builder::new()
            .name("emuvc-render".into())
            .spawn(move || {
                for (spec, index, time_ns) in req_rx {
                    let data = inner.render(&spec, index, time_ns);
                    if res_tx.send(((spec, index), data)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn render thread");
        Threaded {
            tx: req_tx,
            rx: res_rx,
            pending: None,
        }
    }

    fn request(&mut self, spec: &FrameSpec, index: u64, time_ns: u64) {
        self.tx
            .send((*spec, index, time_ns))
            .expect("render thread died");
        self.pending = Some((*spec, index));
    }

    fn collect(&mut self) -> (Key, Vec<u8>) {
        self.pending = None;
        self.rx.recv().expect("render thread died")
    }
}

impl FrameSource for Threaded {
    fn render(&mut self, spec: &FrameSpec, index: u64, time_ns: u64) -> Vec<u8> {
        if self.pending.is_some() {
            let (key, data) = self.collect();
            if key == (*spec, index) {
                return data;
            }
            // Wrong guess (frames were skipped): render what's needed.
        }
        self.request(spec, index, time_ns);
        self.collect().1
    }

    fn prefetch(&mut self, spec: &FrameSpec, index: u64, time_ns: u64) {
        if self.pending.is_none() {
            self.request(spec, index, time_ns);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PatternKind, PixelFormat, TestPattern};

    #[test]
    fn same_frames_as_inline() {
        let spec = FrameSpec {
            format: PixelFormat::Yuyv,
            width: 64,
            height: 48,
        };
        let mut inline = TestPattern::new(PatternKind::ColorBars);
        let mut threaded = Threaded::new(Box::new(TestPattern::new(PatternKind::ColorBars)));
        threaded.prefetch(&spec, 0, 0);
        assert_eq!(threaded.render(&spec, 0, 0), inline.render(&spec, 0, 0));
        threaded.prefetch(&spec, 1, 0);
        // Skipped frame 1: the prefetch is discarded.
        assert_eq!(threaded.render(&spec, 2, 0), inline.render(&spec, 2, 0));
        assert_eq!(threaded.render(&spec, 3, 0), inline.render(&spec, 3, 0));
    }
}
