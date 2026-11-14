//! USB Video Class model: constants, descriptors, controls and streaming.
//!
//! - [`consts`], [`desc`], [`topology`]: UVC vocabulary, descriptor builders and parsing.
//! - [`control`]: control values, validation and spec defaults.
//! - [`stream`]: probe/commit negotiation and payload generation.
//! - [`function::UvcFunction`]: an `emuvc_usb::Function` tying it together.

pub mod consts;
pub mod control;
pub mod desc;
pub mod function;
pub mod host;
pub mod stream;
pub mod topology;

pub use function::{ControlSpec, EventKind, EventSpec, UvcFunction};
pub use stream::{IdlePayload, StreamQuirks};
