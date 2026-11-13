<p align="center"><img src="docs/logo.svg" alt="emuvc logo" width="256"></p>

# emuvc

An **emu**lator for **U**SB **V**ideo **C**lass cameras, for testing libuvc
and other UVC hosts without the hardware.

Devices are built from real descriptors and behave dynamically: probe/commit
negotiation, controls with real ranges, timed isochronous/bulk streaming of
stamped test frames, status interrupts — plus configurable faults (USB errors,
stalls, timeouts, disconnects, UVC quirks).

The first model is a laptop webcam (Luxvisions 30c9:0052: UVC 1.5, MJPEG and
YUY2 up to 1080p plus an IR function), reproduced byte for byte.

## Quick start

```sh
cargo build

# Any libusb program sees the emulated camera instead of real devices (no root):
target/debug/emuvc run -- lsusb -v -d 30c9:0052
target/debug/emuvc run -- ../libuvc/build/example
# (same as LD_PRELOAD=target/debug/libemuvc_libusb.so <program>)

target/debug/emuvc models               # device catalogue (incl. "synthetic", "lsusb")
target/debug/emuvc run -d lsusb:bug-report.txt -- ./app   # emulate a camera from `lsusb -v` text
target/debug/emuvc selftest             # stream every format/frame in-process, check frames
scripts/libuvc-test.sh                  # libuvc end-to-end suite (upstream master; LIBUVC_SRC=DIR: a checkout)
scripts/vm-usbip-test.sh                # USB/IP vs the real kernel (uvcvideo, libusb), in a throwaway VM
scripts/libuvc-coverage.sh              # libuvc coverage of the suite (gcov)
cargo test
```

CI (`.github/workflows/ci.yml`) runs all of these on every push and pull request, and weekly
since libuvc master moves on its own. It also requires `cargo fmt --check` and
`cargo clippy --workspace --all-targets -- -D warnings` to pass. The libuvc coverage table is in
the job summary and the annotated sources in the `libuvc-coverage` artifact. Run it by hand
(Actions, "Run workflow") to test another libuvc repository or ref, e.g. `pull/336/head`.

The Rust version is pinned in `rust-toolchain.toml`, which rustup also uses for local builds, so
a new Rust release changes nothing until its update is merged. Dependabot proposes the updates:
new clippy lints show up on those pull requests.

## Configuration

Environment: `EMUVC_DEVICE=model[,model...]` (or `generic:/sys/bus/usb/devices/X/descriptors`
to emulate any device from a descriptor dump), `EMUVC_CLOCK=virtual` (stream
as fast as the host reads), `EMUVC_SEED`, `EMUVC_LOG=info|debug|trace`, or a
TOML file in `EMUVC_CONFIG`:

```toml
clock = "realtime"

[[device]]
model = "luxvisions-30c9-0052"
strip_dht = true                         # MJPEG without Huffman tables
uvc = { no_eof = true }                  # UVC quirks: see StreamQuirks

[[device.fault]]                         # generic USB faults: see emuvc_usb::fault
on = { iso_in = { ep = 0x81 } }
action = { error = "protocol" }
probability = 0.01

[[device.patch]]                         # malformed descriptors: see emuvc_devices::patch
target = { iface = "vs", subtype = 0x06 }
set = [[4, 4]]                           # first MJPEG format: bNumFrameDescriptors = 4
```

## Layout

| crate | role |
| --- | --- |
| `emuvc-usb` | device model (`UsbDevice`, `CompositeDevice`), virtual host controller (`Host`), fault injection |
| `emuvc-uvc` | UVC descriptors/topology, controls, probe/commit, payload streaming, quirks |
| `emuvc-video` | test patterns, pixel formats, MJPEG, frame stamps |
| `emuvc-devices` | device catalogue and configuration |
| `emuvc-libusb` | transport: LD_PRELOAD-able libusb-1.0 replacement |
| `emuvc-usbip` | transport: USB/IP server (test the real kernel, in a VM) |
| `emuvc` | CLI |

Design: [docs/DESIGN.md](docs/DESIGN.md). Kernel-side testing in a VM:
[docs/VM.md](docs/VM.md). Tools: `tools/uvc-xu-info.py` (extension unit sizes
of a real camera, for new models).

## License

BSD 3-Clause, see [LICENSE](LICENSE). The UVC specifications used while
writing this (USB-IF PDFs) are not included.
