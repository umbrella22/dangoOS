# Independent image fixtures

Created before the N5 decoder implementation by the independent acceptance
agent with Pillow. `receipt.json` records file hashes and exact pixel inputs.
Images are synthetic orientation/color/grayscale/alpha patterns and deliberately
truncated/corrupted variants. They contain no external photographic material.
The tests compare PNG bytes exactly and check JPEG via an independent Pillow
oracle in the acceptance probe. PNG alpha uses black compositing with
`(channel * alpha + 127) / 255`.

`orientation-rgb565.bin` is an independent LVGL 9 RGB565 file with a 12-byte
header and little-endian pixels; its receipt describes the binary fields and
hash. It is used by the native LVGL album probe, separately from the Lua image
decoder. `scripts/n5-check-images.py` is the reproducible Pillow pixel oracle.
