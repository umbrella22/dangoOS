# Skin compatibility fixture

`legacy-default-v4.bin` is the accepted 2026-11-11 articulated default skin before the image 2.6 artwork refresh. It is a stable input for the test-only conversion to legacy v1 row RLE and the v1 whole-package zlib wrapper. The converted form must remain within the historical 1 MiB decoded limit; current artwork can legitimately require more bytes in this obsolete encoding. Current v4 packages remain covered by the shipped-skin validation and rotation/rendering tests.

SHA256: `24b00baf37262145918a89c1251e642abbc500784d4e054d77243567e5d7401e`. Size: 494192 bytes. This fixture is not bundled into firmware or device skin packages.
