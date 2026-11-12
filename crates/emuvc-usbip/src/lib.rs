//! USB/IP transport for emuvc.
//!
//! Design: [`serve`] speaks the USB/IP wire protocol as the *server* (what
//! `usbipd` + the `usbip-host` stub driver do on a real machine) in front of an
//! [`emuvc_usb::Host`]. Each accepted TCP connection gets its own thread and
//! starts in the "operation" phase (`OP_REQ_DEVLIST` / `OP_REQ_IMPORT`). A
//! successful import resets the device (so every import looks like a fresh
//! plug-in), marks it busy (one importer at a time) and switches the socket to
//! the URB phase:
//!
//! * the *reader* (the connection thread) decodes `USBIP_CMD_SUBMIT` /
//!   `USBIP_CMD_UNLINK`, translates them to [`emuvc_usb::Urb`]s and calls
//!   `Host::submit` / `Host::cancel`;
//! * completion callbacks run on the engine thread, so they only encode the
//!   reply PDU and push the bytes into a channel;
//! * the *writer* thread drains that channel into the socket.
//!
//! A per-connection table of pending sequence numbers arbitrates the
//! submit/unlink race: an unlinked URB that was still pending answers
//! `RET_UNLINK(-ECONNRESET)` and its `RET_SUBMIT` is suppressed; one that
//! already completed answers `RET_UNLINK(0)`. Malformed input closes the
//! connection (with a log line); on disconnect all pending URBs are cancelled
//! and the device is released.
//!
//! The client half ([`list`], [`import`]) is used by the tests and by
//! [`attach_vhci`], which hands an imported socket to the kernel's `vhci-hcd`
//! the way `usbip attach` does.

pub mod client;
pub mod proto;
pub mod server;
pub mod vhci;

pub use client::{RemoteDevice, import, list};
pub use server::serve;
pub use vhci::attach_vhci;
