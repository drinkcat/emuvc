#!/bin/bash
# Run a command as root inside a throwaway KVM VM booting the host kernel with
# the host root shared read-only. See docs/VM.md.
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(dirname "$HERE")
OUT=$ROOT/target/vm
MEM=1024 TIMEOUT=120 VERBOSE=0
usage() { echo "usage: $0 [-m MEM_MB] [-t TIMEOUT_S] [-v] -- COMMAND [ARGS...]" >&2; exit 2; }
while getopts "m:t:vh" o; do
    case $o in m) MEM=$OPTARG;; t) TIMEOUT=$OPTARG;; v) VERBOSE=1;; *) usage;; esac
done
shift $((OPTIND-1))
[ $# -ge 1 ] || usage

KVER=$(uname -r)
KERNEL=/boot/vmlinuz-linux
[ -r "$KERNEL" ] || { echo "vm-run: cannot read $KERNEL" >&2; exit 2; }
if ! file -b "$KERNEL" | grep -q "version $KVER "; then
    echo "vm-run: $KERNEL does not match running kernel $KVER (reboot after upgrade?)" >&2
    exit 2
fi
[ -w /dev/kvm ] || { echo "vm-run: /dev/kvm not accessible" >&2; exit 2; }

# ---- initramfs (cached; rebuilt when kernel, busybox or init changes) ----
mkdir -p "$OUT"
INITRD=$OUT/initramfs.cgz
STAMP="$KVER $(stat -c '%Y %s' /usr/bin/busybox) $(stat -c '%Y' "$HERE/vm/init" "$0")"
if [ ! -f "$INITRD" ] || [ "$(cat "$OUT/initramfs.stamp" 2>/dev/null)" != "$STAMP" ]; then
    echo "vm-run: building initramfs..." >&2
    S=$(mktemp -d "$OUT/stage.XXXXXX")
    mkdir -p "$S"/{bin,sbin,proc,sys,dev,mnt,mods,tmp,usr/bin,etc}
    cp /usr/bin/busybox "$S/bin/busybox"
    for a in sh mount umount insmod ip ifconfig chroot env base64 cp mkdir hostname \
             dmesg tail cat poweroff sleep echo; do
        ln -sf busybox "$S/bin/$a"
    done
    ln -sf ../bin/busybox "$S/sbin/poweroff"
    cp "$HERE/vm/init" "$S/init"; chmod +x "$S/init"
    : > "$S/mods/list"; n=0
    while read -r _ path _; do
        n=$((n+1)); name=$(printf '%02d-%s' $n "$(basename "${path%.zst}")")
        case $path in
            *.zst) zstd -dqc "$path" > "$S/mods/$name" ;;
            *.xz)  xz -dc "$path" > "$S/mods/$name" ;;
            *)     cp "$path" "$S/mods/$name" ;;
        esac
        echo "$name" >> "$S/mods/list"
    done < <({ modprobe --show-depends 9pnet_virtio; modprobe --show-depends 9p; } | grep '^insmod' | awk '!seen[$2]++')
    (cd "$S" && find . | cpio -o -H newc -R 0:0 --quiet | gzip -1) > "$INITRD.tmp"
    mv "$INITRD.tmp" "$INITRD"; echo "$STAMP" > "$OUT/initramfs.stamp"
    rm -rf "$S"
fi

# ---- script to run in the chroot (cwd + quoted argv), passed via cmdline ----
SCRIPT="cd $(printf '%q' "$PWD") || exit 126
exec$(printf ' %q' "$@")
"
B64=$(printf '%s' "$SCRIPT" | base64 -w0)
if [ "${#B64}" -gt 1500 ]; then echo "vm-run: command too long for kernel cmdline" >&2; exit 2; fi
if [ $VERBOSE = 1 ]; then LOG="loglevel=7"; else LOG="quiet loglevel=0"; fi

rc=
QCMD=(timeout -s KILL "$TIMEOUT" qemu-system-x86_64 -enable-kvm -cpu host -smp 2 -m "$MEM"
    -kernel "$KERNEL" -initrd "$INITRD"
    -append "console=ttyS0 panic=-1 rdinit=/init $LOG emuvc_script=$B64"
    -display none -serial stdio -monitor none -no-reboot -nic none
    -virtfs "local,path=/,mount_tag=hostroot,security_model=none,readonly=on,id=hostroot")
while IFS= read -r line; do
    line=${line%$'\r'}
    case $line in
        *@@EMUVC_RC=*) pre=${line%%@@EMUVC_RC=*}; rc=${line##*@@EMUVC_RC=}
                       [ -n "$pre" ] && printf '%s\n' "$pre" ;;
        *) [ -n "$rc" ] || printf '%s\n' "$line" ;;
    esac
done < <("${QCMD[@]}" </dev/null 2>&1)
if [ -z "$rc" ]; then
    echo "vm-run: VM ended without reporting an exit code (timeout ${TIMEOUT}s, panic or crash)" >&2
    exit 124
fi
exit "$rc"
