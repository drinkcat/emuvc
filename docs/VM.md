# Disposable test VM

`scripts/vm-run.sh` runs a command as root inside a throwaway QEMU/KVM VM that
boots an installed kernel (the running one by default) with the host root
shared read-only. Purpose: USB/IP tests need `vhci-hcd`, `usbip-core`, `uvcvideo`,
usbfs etc.; those modules must never be loaded on the host, only in the VM.

## How it works
- Initramfs (cached in `target/vm/`, rebuilt when kernel/busybox/init change):
  static busybox + decompressed `netfs`, `9pnet`, `9pnet_virtio`, `9p` modules
  + `scripts/vm/init`.
- QEMU: KVM, 2 CPUs, 1 GiB, serial console on stdio, no network devices,
  `-virtfs local,path=/,readonly=on`, `-no-reboot`, `panic=-1`.
- Init mounts the host root over 9p (ro), puts tmpfs on `/tmp`, `/run`,
  `/var/tmp`, fresh proc/sysfs/devtmpfs/devpts/debugfs, brings up `lo`, then
  chroots into it and runs the command (same cwd, HOME=/tmp), prints
  `@@EMUVC_RC=<n>`, and powers off. The wrapper strips that marker and exits
  with that code (124 on timeout/crash, 125 on init failure).

## Usage
    scripts/vm-run.sh [-m MEM_MB] [-t TIMEOUT_S] [-v] -- COMMAND [ARGS...]
    scripts/vm-run.sh -- sh -c 'modprobe vhci-hcd && cat /sys/devices/platform/vhci_hcd.0/status'
Default timeout 120 s. `-v` shows kernel messages; on non-zero exit the last
dmesg lines are always printed. A run of `true` takes well under a second.

## Limitations
- The host filesystem is read-only; write only to `/tmp`, `/run`, `/var/tmp`
  (all lost at exit). Build artifacts must be built on the host beforehand.
- No network except loopback; no stdin; command line must fit in ~1 KB (it is
  passed base64 on the kernel cmdline). stdout/stderr are merged (serial).
- Boots kernel `KVER` (default `uname -r`, modules in `/lib/modules/$KVER`) from
  `KERNEL` (default `/boot/vmlinuz-$KVER`, else Arch's `/boot/vmlinuz-linux`),
  which must be that version: reboot after a kernel upgrade, or set `KVER`. Any
  installed kernel works; CI boots a generic Ubuntu kernel. Needs a static
  busybox (`BUSYBOX` overrides the one on `PATH`).

## USB/IP test
`scripts/vm-usbip-test.sh` boots the VM with `scripts/vm/usbip-guest.sh`:
`emuvc usbip` on loopback, `emuvc usbip-attach 1-1` into vhci-hcd, then
lsusb, V4L2 capture through uvcvideo (frames checked with `emuvc
check-frames`), and libuvc's `stream_test` through the real libusb (allowing two skipped
frames: over USB/IP frames can arrive in bursts, and libuvc then skips all but the newest).
