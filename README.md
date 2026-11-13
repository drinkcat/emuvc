# emuvc

USB Video Class camera emulator, for testing libuvc and other UVC hosts
without the hardware. Devices are built from real descriptors and behave
dynamically: probe/commit negotiation, controls with real ranges, timed
isochronous/bulk streaming of stamped test frames, status interrupts — plus
configurable faults (USB errors, stalls, timeouts, disconnects, UVC quirks).

The first model is a laptop webcam (Luxvisions 30c9:0052: UVC 1.5, MJPEG and
YUY2 up to 1080p plus an IR function), reproduced byte for byte.

## Quick start

```sh
cargo build

# Any libusb program sees the emulated camera instead of real devices (no root):
target/debug/emuvc run -- lsusb -v -d 30c9:0052
target/debug/emuvc run -- ../libuvc/build/example
# (same as LD_PRELOAD=target/debug/libemuvc_libusb.so <program>)

target/debug/emuvc models               # device catalogue
target/debug/emuvc selftest             # stream every format/frame in-process, check frames
scripts/libuvc-test.sh                  # libuvc end-to-end suite (builds ../libuvc out of tree)
scripts/vm-usbip-test.sh                # USB/IP vs the real kernel (uvcvideo, libusb), in a throwaway VM
cargo test
```

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
