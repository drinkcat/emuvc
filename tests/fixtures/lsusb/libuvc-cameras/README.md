These `lsusb -v` dumps come from libuvc (https://github.com/libuvc/libuvc,
`cameras/*.txt`, BSD licence, Copyright (C) 2010-2015 Ken Tossell).
They were produced by old lsusb versions (2010s), which is the point: the
importer must read them. `quickcampro9000_builtin_ctrls.txt` and
`quickcampro9000_extra_ctrls.txt` are not lsusb output (lists of V4L2
controls) and are expected to be rejected.
