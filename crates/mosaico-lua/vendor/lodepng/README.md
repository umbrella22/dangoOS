# LodePNG

Source: `https://github.com/lvandeve/lodepng`, commit
`e584a8490db1b76b81bf298c11c09d3b55d9c270`.

`lodepng.c` and `lodepng.h` retain the upstream zlib license and source notices.
The build privately prefixes exported symbols to coexist with LVGL's own
LodePNG and redirects allocations to the owning Lua VM's capped image ledger.
PNG decoding uses a bounded output allocation. RGBA is composited over black
with rounded integer arithmetic before exposing RGB888 frames.
