# Application raster and fonts

Copied from the locally maintained esp-claw display and esp_painter sources in
`firmware/runtime/lua-engine/vendor/esp-claw`, upstream commit
`345073de68179542c307e303df81e38baadf77f0` (Apache-2.0). Existing local DFN1 and
font-size support is preserved. The build prefixes exported symbols to coexist
with Lua host tests. No Lua headers or VM enter this library.
