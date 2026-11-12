#!/usr/bin/env python3
"""Print GET_LEN/GET_INFO of every extension unit control of a real UVC camera.

Helps writing emuvc device models. Read-only metadata queries through
uvcvideo's UVCIOC_CTRL_QUERY; no streaming, no GET_CUR.

usage: uvc-xu-info.py /dev/videoN UNIT:NUM_CONTROLS [UNIT:NUM_CONTROLS...]
  e.g. uvc-xu-info.py /dev/video0 3:2 4:26 5:3
"""
import ctypes, fcntl, os, sys

class Query(ctypes.Structure):
    _fields_ = [("unit", ctypes.c_uint8), ("selector", ctypes.c_uint8), ("query", ctypes.c_uint8),
                ("size", ctypes.c_uint16), ("data", ctypes.POINTER(ctypes.c_uint8))]

UVCIOC_CTRL_QUERY = 0xC0000000 | (ctypes.sizeof(Query) << 16) | (ord('u') << 8) | 0x21
GET_LEN, GET_INFO = 0x85, 0x86

def query(fd, unit, sel, q, size):
    buf = (ctypes.c_uint8 * size)()
    arg = Query(unit, sel, q, size, ctypes.cast(buf, ctypes.POINTER(ctypes.c_uint8)))
    fcntl.ioctl(fd, UVCIOC_CTRL_QUERY, arg)
    return bytes(buf)

fd = os.open(sys.argv[1], os.O_RDWR)
for spec in sys.argv[2:]:
    unit, n = map(int, spec.split(":"))
    for sel in range(1, n + 1):
        try:
            length = int.from_bytes(query(fd, unit, sel, GET_LEN, 2), "little")
            info = query(fd, unit, sel, GET_INFO, 1)[0]
            print(f"unit {unit} selector {sel}: len {length} info 0x{info:02x}")
        except OSError as e:
            print(f"unit {unit} selector {sel}: {e.strerror}")
