#!/usr/bin/env python3
"""winshot.py <window-id> <out.png> [scale]

Saves a screenshot of ONE window as a PNG. It does not change which window has focus.
Handy for showing Claude what the Grok Bot window looks like.

Requirements: Linux with X11, python3, the Pillow library, and the `xwd` tool.
    sudo apt install x11-apps python3-pil
Find a window id with:  wmctrl -l      (the first column, like 0x04800003)

Example:  python3 winshot.py 0x04800003 shot.png 0.5   (0.5 = half size)
"""
import struct
import subprocess
import sys

from PIL import Image


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    wid, out = sys.argv[1], sys.argv[2]
    scale = float(sys.argv[3]) if len(sys.argv) > 3 else 1.0

    raw = subprocess.run(["xwd", "-silent", "-id", wid], capture_output=True, check=True).stdout
    # The xwd header is 25 32-bit numbers. Work out the byte order from the file version field.
    h = struct.unpack(">25I", raw[:100])
    if h[0] > 100000 or h[1] != 7:
        h = struct.unpack("<25I", raw[:100])
    hsize, _ver, _fmt, _depth, w, ht, _xoff, border, _bunit, _bbo, _bpad, _bpp, bpl, _vclass = h[:14]
    ncolors = h[19]
    offset = hsize + ncolors * 12
    data = raw[offset:offset + bpl * ht]
    mode = "BGRX" if border == 0 else "XRGB"
    img = Image.frombuffer("RGB", (w, ht), data, "raw", mode, bpl, 1)
    if scale != 1.0:
        img = img.resize((int(w * scale), int(ht * scale)))
    img.save(out)
    print(out, img.size)


if __name__ == "__main__":
    main()
