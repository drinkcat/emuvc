`67-05a3_8830.txt`: ELP 05a3:8830 bulk-streaming camera, output of a patched
lsusb pasted in libuvc pull request #67 (no "Bus NNN Device NNN" line).
`302-0001_c002.txt`: "1081" stereo camera, a Linux USB gadget (kernel 4.19) with two UVC 1.1
functions (one per sensor) and an ADB interface: `lsusb -v -v` attached to libuvc issue #302
(line endings converted to LF; the host's root hubs are in it too).
