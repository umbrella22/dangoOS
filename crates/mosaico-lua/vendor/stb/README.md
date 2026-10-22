# stb_image_write

`stb_image_write.h` is copied unchanged from `https://github.com/nothings/stb`,
commit `a7d4c2175630e1ea5b6f8dd00ab96fed0b97b96d`.

The header includes its dual MIT/public-domain license. Only the in-memory JPEG
writer is used. All exports are private static symbols, stdio is disabled, and
allocation macros route temporary encoder buffers through the owning VM ledger.
JPEG conversion uses baseline quality 90; output is capped at 4 MiB.
