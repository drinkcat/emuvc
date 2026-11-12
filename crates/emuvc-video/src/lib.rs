//! Synthetic UVC video frame generation.
//!
//! Produces test frames in the pixel formats UVC cameras send (YUYV, UYVY,
//! NV12, GREY8, MJPEG). Every frame carries a [`stamp`] (counter + CRC) so
//! receivers can verify integrity and ordering.

pub mod convert;
pub mod format;
pub mod jpeg;
pub mod pattern;
pub mod stamp;
pub mod threaded;

pub use format::{FrameSpec, PixelFormat};
pub use pattern::{PatternKind, TestPattern};
pub use threaded::Threaded;

/// A producer of video frames.
pub trait FrameSource: Send {
    /// Render frame `index` (0-based count since streaming started) captured at `time_ns`.
    /// Returns the complete frame in `spec.format` (MJPEG: a complete JPEG, SOI..EOI).
    fn render(&mut self, spec: &FrameSpec, index: u64, time_ns: u64) -> Vec<u8>;

    /// Hint: `render` will probably be called for this frame next. Sources
    /// that can work ahead (see [`Threaded`]) start on it now.
    fn prefetch(&mut self, _spec: &FrameSpec, _index: u64, _time_ns: u64) {}
}
