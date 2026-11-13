# emuvc design

emuvc emulates USB Video Class cameras so libuvc (and other UVC hosts) can be
tested without the hardware: realistic devices built from real descriptors,
behaving dynamically (probe/commit negotiation, controls, timed streaming,
status interrupts) rather than replaying captures, with injectable faults.

## Layers

```
 transports        libusb shim (LD_PRELOAD)    USB/IP server     (future: usbfs shim,
                   emuvc-libusb                emuvc-usbip        raw-gadget, FunctionFS)
                          │  URBs / completions       │
 host engine       ───────┴──────── emuvc_usb::Host ──┴─────────  virtual host controller:
                                                                  bus time, scheduling,
                                                                  enumeration, cancel, timeouts
 device            UsbDevice trait ── FaultInjector (generic USB faults, optional)
                          │
                   CompositeDevice (chapter 9: descriptors, SET_CONFIGURATION/INTERFACE, halt)
                     ├── UvcFunction (emuvc-uvc): controls, probe/commit, payloads, status, quirks
                     │        └── FrameSource (emuvc-video): test patterns + frame stamps
                     └── other functions (DFU stub, ...)
 catalogue         emuvc-devices: models (descriptors + tuning), config (TOML/env), generic-from-dump
```

Each layer only knows the one below. Device models never see a transport;
transports never see UVC.

### Device side (`emuvc-usb`)
- `UsbDevice` is a **passive state machine**: `control`, `data_in/out` (bulk,
  interrupt; may NAK), `iso_in/out` (one service interval), `endpoint` (what
  the current alt settings expose), `next_wakeup`/`tick` (autonomous events).
  No threads, no sleeping, no wall clock: every call carries the bus time
  (`Ctx`, microframes) and a seeded RNG. Models are deterministic and testable.
- `CompositeDevice` implements USB chapter 9 once (descriptors, strings,
  addressing, configurations, alt settings, endpoint halt) and routes
  class/vendor requests and endpoint traffic to `Function`s by interface.
  A device model is "descriptors + functions".
- Descriptors stay **raw bytes** (typed builders/parsers exist, `desc`,
  `emuvc_uvc::desc`): real devices are reproduced byte for byte, and broken
  descriptors are just different bytes.

### Host engine (`emuvc_usb::Host`)
One engine thread owns the devices of a virtual bus. Transports call
`submit(Urb, callback)` / `cancel` — the model of usbfs, libusb's async API and
USB/IP alike, so each transport is a thin adapter. The engine:
- enumerates devices like Linux (descriptor reads, SET_ADDRESS, strings,
  SET_CONFIGURATION) and caches the results (`DeviceInfo`, the "sysfs" view);
- serialises control transfers on EP0 (with device latency and host timeouts);
- schedules isochronous packets in consecutive service intervals (ISO_ASAP
  style), polls interrupt endpoints at their interval, retries NAKed bulk;
- flushes URBs of endpoints removed by SET_INTERFACE, completes everything
  with `NoDevice` on unplug (also device-initiated, for fault scenarios).

**Time.** Devices only see bus time. `Realtime`: bus time follows the wall
clock; isochronous URBs complete at hardware pace and a slow host loses data
exactly as with a real camera. `Virtual`: when idle, the engine jumps to the
next deadline, so streams run as fast as the host consumes them (≈10x faster
for libuvc tests) and timeouts cost nothing. Virtual time only advances
through work the host queued (or device wakeups/timeouts, after a short grace
period), which keeps runs reproducible as long as the host keeps URBs queued.

### UVC (`emuvc-uvc`)
- `topology` parses a function from descriptors; `UvcFunction::new` derives
  everything else from it: one `Control` per advertised selector (spec
  defaults, `control::standard`), probe/commit for every format/frame/interval,
  iso alt settings, status endpoint. Models then override ranges, sizes, info
  bits.
- Controls: GET_CUR/MIN/MAX/RES/DEF/LEN/INFO and SET_CUR with validation, the
  request error code control, auto-mode dependencies (e.g. WB auto disables WB
  temperature, reported via status interrupts).
- Streaming: frame `k` is captured at `start + k * dwFrameInterval` and sent
  as payloads (12-byte headers with PTS/SCR by default, FID/EOF framing), one
  per iso interval or bulk transfer. A stream that isn't read fast enough
  skips stale frames. Alt-setting choice follows from dwMaxPayloadTransferSize
  (smallest alt carrying the bandwidth), as hosts expect.
- Still images: methods 2 (in the video stream, STI bit) and 3 (dedicated
  bulk endpoint), with still probe/commit and the trigger control.
- Device-initiated events (`[[device.event]]`): button presses, control
  changes, arbitrary (malformed) status packets, at times or periodically.
- Frames come from `FrameSource` (`emuvc-video`), rendered one frame ahead on
  a worker thread (`Threaded`, like a camera's ISP pipeline) so rendering
  never stalls the bus: test patterns in every format libuvc knows (YUYV,
  UYVY, NV12/21, I420, P010, GREY 8/16, RGB/BGR, 8-bit Bayer), MJPEG
  (optionally without DHT) and H.264 (a fake bitstream: hosts don't decode
  it), each carrying a **stamp** (frame
  counter + CRC-8 in a band at the top, documented in `stamp.rs`) so tests
  detect dropped, reordered or corrupted frames end to end, even through
  libuvc and JPEG.

### Fault injection
Two layers, both configurable from TOML:
- **Generic** (`emuvc_usb::fault::FaultInjector`, wraps any device): rules =
  match (control setup fields incl. UVC selector/entity, endpoint traffic,
  time) + action (stall, timeout, error status, delay, truncate, corrupt,
  replace, ZLP, disconnect) + firing policy (skip, count, every Nth,
  probability with the seeded RNG, time window).
- **UVC** (`StreamQuirks`): misbehaviour that needs class knowledge — no EOF,
  stuck FID, ERR bit, dropped/truncated frames, bogus
  dwMaxPayloadTransferSize, short probe structures, ignoring the requested
  format, startup delay, ZLPs vs header-only idle payloads.

- **Transport** (libusb shim only): `EMUVC_LIBUSB_FAIL="open=access,submit_transfer=no_mem@5"`
  makes chosen libusb calls fail, for host error paths no device can reach.

Rule of thumb: if it can be described on the wire, it's a generic rule; if it
needs to know what a frame or a probe structure is, it's a quirk.

## Device models
- Real devices, byte for byte (`luxvisions_30c9_0052.rs`).
- Any device from a descriptor dump (`generic:` binary dump, `lsusb:` the
  text of `lsusb -v`, as found in bug reports; `lsusb.rs` rebuilds the exact
  bytes, including broken descriptors). Default control values.
- `synthetic`: a generator covering most of the spec — UVC 1.0/1.1/1.5,
  full/high/SuperSpeed, iso or bulk, every control and unit, all formats,
  discrete or continuous intervals, several streaming interfaces, still
  methods 1-3 — plus known-bad variants from libuvc's bug tracker
  (`tests/libuvc/configs/issue-*.toml`).

## Transports

**libusb shim** (`emuvc-libusb`, cdylib). Exports all 104 libusb 1.0.30
symbols, so `LD_PRELOAD` (or a `libusb-1.0.so.0` on `LD_LIBRARY_PATH`) puts any
libusb program — libuvc, lsusb — in front of the emulator, without root.
Config comes from the environment (`EMUVC_DEVICE`, `EMUVC_CONFIG`,
`EMUVC_CLOCK`, `EMUVC_SEED`, `EMUVC_LOG`). Fidelity choices:
- descriptor structs are built by an exact port of libusb's parser (same
  `extra` placement), from the descriptors the engine read at enumeration;
- event handling follows libusb's event-lock protocol, sync transfers are
  async + `handle_events` like in libusb (callbacks may run inside
  `libusb_control_transfer`), `libusb_close` discards in-flight transfers;
- a kernel driver (uvcvideo) is emulated as bound to video interfaces, so
  detach/attach paths run (`kernel_driver = false` to disable).
Not supported: hotplug callbacks, pollfd integration (empty list; timeouts
are enforced by the engine), `libusb_wrap_sys_device`, root hubs
(`libusb_get_parent` is NULL), bulk streams. `libusb_set_option` is variadic
in C; the shim relies on the x86-64/AArch64 Linux ABI passing its single
argument like a named one. Tools reading sysfs directly (lsusb's device
strings) don't see emulated devices there.

The trade-off: the real libusb is not exercised. Two ways to keep it in the
loop: **USB/IP** (the real kernel + libusb + uvcvideo, inside a VM — see
`docs/VM.md`; never load vhci-hcd on the host), or a future **usbfs shim**
(LD_PRELOAD intercepting the syscalls libusb makes on `/dev/bus/usb` and
sysfs, as umockdev does, but backed by the live engine).

**USB/IP** (`emuvc-usbip`): `emuvc usbip` serves the configured devices
(OP_REQ_DEVLIST/IMPORT, CMD_SUBMIT/UNLINK mapped 1:1 onto `Host`; loopback
only by default, the protocol has no authentication); `emuvc usbip-attach`
replaces `usbip attach`. `scripts/vm-usbip-test.sh` runs the whole real stack
in a throwaway VM: kernel enumeration through vhci-hcd, uvcvideo streaming
(V4L2), and libuvc over the real libusb/usbfs.

## Adding a device model
1. Capture: `/sys/bus/usb/devices/<dev>/descriptors` (+ `bos_descriptors`),
   control ranges (`v4l2-ctl -L`), extension unit sizes
   (`tools/uvc-xu-info.py`). Never needs streaming from the real camera.
2. Quick start: `EMUVC_DEVICE=generic:/path/to/descriptors` or
   `EMUVC_DEVICE=lsusb:/path/to/lsusb-v.txt` already emulates it with default
   control values (`emuvc import-lsusb FILE --raw OUT` converts the text).
3. Proper model: a module in `emuvc-devices` building the descriptors with the
   builders (byte-compare against the dump in a test), tuning controls, and an
   entry in `MODELS`. See `luxvisions_30c9_0052.rs`.
4. `emuvc selftest MODEL` streams every format/frame and checks the frames.

## Testing
- `cargo test`: unit tests, host engine tests, in-process end-to-end tests of
  the model on a virtual clock (`crates/emuvc-devices/tests`).
- `scripts/libuvc-test.sh`: builds libuvc out of tree and runs
  `tests/libuvc/stream_test.c` under the shim: formats, frame integrity,
  controls, multiple devices, fault scenarios.
- `scripts/vm-usbip-test.sh`: USB/IP against the real kernel, in a VM.
- `scripts/libuvc-coverage.sh`: the libuvc suite against a gcov-instrumented
  libuvc, with per-file/function coverage (`scripts/gcov-summary.py`).
- `emuvc selftest`; `emuvc check-frames` (raw captures, e.g. from
  `v4l2-ctl --stream-to`); `emuvc run -- lsusb -v` (diff against the real device).

## Future work
- More real models (from bug reports via the lsusb importer).
- Richer scenarios (re-enumeration, timed fault phases).
- usbmon-format pcap of engine traffic (Wireshark dissects UVC).
- Real encoders for frame-based formats; UVC 1.5 H.264/VP8 format descriptors.
