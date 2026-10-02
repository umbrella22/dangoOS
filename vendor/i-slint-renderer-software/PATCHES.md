# Slint software renderer 1.17.1

Source: the published `i-slint-renderer-software` 1.17.1 crate from crates.io.
The upstream package metadata, source headers and licenses are retained.
The root Cargo manifest selects this copy through `[patch.crates-io]`.

## Retained framebuffer path rendering

Upstream `RenderToBuffer::process_filled_path` and `process_stroked_path` pass the
item clip to the path rasterizer without applying the renderer's dirty region.
A path intersecting a small dirty rectangle consequently blends over pixels
outside that rectangle. Those pixels retain the previous frame's contents, so
antialiased strokes darken at overlaps on subsequent partial frames.

The local patch passes `PhysicalRegion` through both entry points. `path.rs`
intersects it with the path, item clip and target bounds, then blends only the
non-overlapping rectangles returned by `PhysicalRegion::iter()`. Each fill or
stroke generates one mask. The path coordinate system and mask rasterization
remain independent of the dirty region.

Path tests cover fills, round strokes, overlapping and separated dirty
rectangles, an empty region, and a path extending beyond the target. The
application's RGB565 integration test compares swapped buffers against full
rendering through rolling history, changing controls and publication recovery.

## Reusable path rasterization workspace

`SoftwareRenderer` retains one mask buffer and `zeno::Scratch` for framebuffer
rendering. Each path resizes and clears the active mask, then calls
`Mask::with_scratch`. This reuses edge/stroke storage and avoids rebuilding
Zeno's default 1,024-cell stack workspace for each path. Only working storage is
retained; geometry, coverage and styles are recalculated for every draw.

The path coordinate system and dirty-region blending are unchanged. Tests compare
reused masks against fresh `Mask::new` output across fills, round strokes, curves,
negative coordinates, empty paths and changing mask dimensions. Clearing the
mask before every draw is required because Zeno writes only covered pixels.

The renderer is excluded from workspace membership. `scripts/check.sh` runs its
unit tests explicitly; release source hashes include the vendored files.
