// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

//! Path rendering support for the software renderer using zeno

use super::draw_functions::{PremultipliedRgbaColor, TargetPixel};
use super::{PhysicalRect, PhysicalRegion};
use alloc::vec::Vec;
use zeno::{Cap, Fill, Join, Mask, Stroke, Style};

pub use zeno::Command;

/// Reuse mask capacity and Zeno's edge storage across paths and frames.
/// The mask is cleared on every draw: Zeno only writes covered pixels.
#[derive(Default)]
pub(crate) struct PathRasterizer {
    mask: Vec<u8>,
    scratch: zeno::Scratch,
}

impl PathRasterizer {
    fn rasterize(
        &mut self,
        commands: &[Command],
        width: usize,
        height: usize,
        style: Style<'_>,
    ) -> &[u8] {
        self.mask.resize(width * height, 0);
        self.mask.fill(0);
        Mask::with_scratch(commands, &mut self.scratch)
            .size(width as u32, height as u32)
            .style(style)
            .render_into(&mut self.mask, None);
        &self.mask
    }
}

/// Convert Slint's PathDataIterator to zeno's Command format
pub fn convert_path_data_to_zeno(
    path_data: i_slint_core::graphics::PathDataIterator,
    rotation: crate::RotationInfo,
    scale_factor: i_slint_core::lengths::ScaleFactor,
    offset: euclid::Vector2D<f32, i_slint_core::lengths::PhysicalPx>,
) -> Vec<Command> {
    use crate::Transform as _;
    use i_slint_core::lengths::LogicalPoint;
    use lyon_path::Event;
    let mut commands = Vec::new();

    let convert_point = |p| {
        let p = (LogicalPoint::from_untyped(p) * scale_factor + offset).transformed(rotation);
        zeno::Point::new(p.x, p.y)
    };

    for event in path_data.iter() {
        match event {
            Event::Begin { at } => {
                commands.push(Command::MoveTo(convert_point(at)));
            }
            Event::Line { to, .. } => {
                commands.push(Command::LineTo(convert_point(to)));
            }
            Event::Quadratic { ctrl, to, .. } => {
                commands.push(Command::QuadTo(convert_point(ctrl), convert_point(to)));
            }
            Event::Cubic {
                ctrl1, ctrl2, to, ..
            } => {
                commands.push(Command::CurveTo(
                    convert_point(ctrl1),
                    convert_point(ctrl2),
                    convert_point(to),
                ));
            }
            Event::End { close, .. } => {
                if close {
                    commands.push(Command::Close);
                }
            }
        }
    }

    commands
}

/// Common rendering logic for both filled and stroked paths
fn render_path_with_style<T: TargetPixel>(
    commands: &[Command],
    path_geometry: &PhysicalRect,
    clip_geometry: &PhysicalRect,
    dirty_region: &PhysicalRegion,
    color: PremultipliedRgbaColor,
    style: zeno::Style,
    buffer: &mut impl crate::target_pixel_buffer::TargetPixelBuffer<TargetPixel = T>,
    rasterizer: &mut PathRasterizer,
) {
    if buffer.num_lines() == 0 {
        return;
    }
    let buffer_geometry = PhysicalRect::from_size(euclid::size2(
        buffer.line_slice(0).len() as i16,
        buffer.num_lines() as i16,
    ));
    let Some(clip) = path_geometry
        .intersection(clip_geometry)
        .and_then(|r| r.intersection(&buffer_geometry))
    else {
        return;
    };
    let dirty = dirty_region.intersection(&clip);
    if dirty.count == 0 {
        return;
    }

    // Rasterize once in the original path coordinates. Keep these coordinates
    // independent of the dirty rectangles to preserve antialias rounding.
    let mask_width = path_geometry.width() as usize;
    let mask_height = path_geometry.height() as usize;
    // #region debug-point B:render-touch-stall
    #[cfg(feature = "render-profiling")]
    let profile_mask = crate::profiling::Span::new(crate::profiling::Phase::Mask);
    #[cfg(feature = "render-profiling")]
    crate::profiling::mask_pixels(mask_width * mask_height);
    // #endregion
    let mask_buffer = rasterizer.rasterize(commands, mask_width, mask_height, style);
    // #region debug-point B:render-touch-stall
    #[cfg(feature = "render-profiling")]
    drop(profile_mask);
    // #endregion

    // iter() merges overlapping dirty rectangles. Blend each dirty pixel once;
    // drawing outside them would composite onto uncleared, retained pixels.
    for (origin, size) in dirty.iter() {
        for screen_y in origin.y as usize..origin.y as usize + size.height as usize {
            let mask_y = (screen_y as isize - path_geometry.origin.y as isize) as usize;
            let line = buffer.line_slice(screen_y);
            let start = origin.x as usize;
            let end = start + size.width as usize;
            for (i, pixel) in line[start..end].iter_mut().enumerate() {
                let mask_x =
                    (start as isize + i as isize - path_geometry.origin.x as isize) as usize;
                let coverage = mask_buffer[mask_y * mask_width + mask_x];
                if coverage > 0 {
                    let coverage_factor = coverage as u16;
                    let alpha_color = PremultipliedRgbaColor {
                        red: ((color.red as u16 * coverage_factor) / 255) as u8,
                        green: ((color.green as u16 * coverage_factor) / 255) as u8,
                        blue: ((color.blue as u16 * coverage_factor) / 255) as u8,
                        alpha: ((color.alpha as u16 * coverage_factor) / 255) as u8,
                    };
                    T::blend(pixel, alpha_color);
                }
            }
        }
    }
}

/// Render a filled path
///
/// * `commands` - The path commands to render
/// * `path_geometry` - The full bounding box of the path in screen coordinates
/// * `clip_geometry` - The clipped region where the path should be rendered (intersection of path and clip)
/// * `color` - The color to render the path
/// * `buffer` - The target pixel buffer
pub fn render_filled_path<T: TargetPixel>(
    commands: &[Command],
    path_geometry: &PhysicalRect,
    clip_geometry: &PhysicalRect,
    dirty_region: &PhysicalRegion,
    color: PremultipliedRgbaColor,
    buffer: &mut impl crate::target_pixel_buffer::TargetPixelBuffer<TargetPixel = T>,
    rasterizer: &mut PathRasterizer,
) {
    render_path_with_style(
        commands,
        path_geometry,
        clip_geometry,
        dirty_region,
        color,
        zeno::Style::Fill(Fill::NonZero),
        buffer,
        rasterizer,
    );
}

/// Render a stroked path
///
/// * `commands` - The path commands to render
/// * `path_geometry` - The full bounding box of the path in screen coordinates
/// * `clip_geometry` - The clipped region where the path should be rendered (intersection of path and clip)
/// * `color` - The color to render the path
/// * `stroke_width` - The width of the stroke
/// * `buffer` - The target pixel buffer
pub fn render_stroked_path<T: TargetPixel>(
    commands: &[Command],
    path_geometry: &PhysicalRect,
    clip_geometry: &PhysicalRect,
    dirty_region: &PhysicalRegion,
    color: PremultipliedRgbaColor,
    stroke_width: f32,
    stroke_line_cap: i_slint_core::items::LineCap,
    stroke_line_join: i_slint_core::items::LineJoin,
    stroke_miter_limit: f32,
    buffer: &mut impl crate::target_pixel_buffer::TargetPixelBuffer<TargetPixel = T>,
    rasterizer: &mut PathRasterizer,
) {
    let mut stroke = Stroke::new(stroke_width);
    stroke
        .cap(match stroke_line_cap {
            i_slint_core::items::LineCap::Round => Cap::Round,
            i_slint_core::items::LineCap::Square => Cap::Square,
            i_slint_core::items::LineCap::Butt | _ => Cap::Butt,
        })
        .join(match stroke_line_join {
            i_slint_core::items::LineJoin::Round => Join::Round,
            i_slint_core::items::LineJoin::Bevel => Join::Bevel,
            i_slint_core::items::LineJoin::Miter | _ => Join::Miter,
        })
        .miter_limit(stroke_miter_limit);
    let style = Style::Stroke(stroke);
    render_path_with_style(
        commands,
        path_geometry,
        clip_geometry,
        dirty_region,
        color,
        style,
        buffer,
        rasterizer,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use crate::{Rgb565Pixel, target_pixel_buffer::TargetPixelBuffer};

    #[derive(Clone)]
    struct Buffer(Vec<Rgb565Pixel>);

    impl TargetPixelBuffer for Buffer {
        type TargetPixel = Rgb565Pixel;
        fn line_slice(&mut self, line: usize) -> &mut [Rgb565Pixel] {
            &mut self.0[line * 40..(line + 1) * 40]
        }
        fn num_lines(&self) -> usize {
            self.0.len() / 40
        }
    }

    fn region(rects: &[PhysicalRect]) -> PhysicalRegion {
        let mut region = PhysicalRegion::default();
        for (i, rect) in rects.iter().enumerate() {
            region.rectangles[i] = rect.to_box2d();
        }
        region.count = rects.len();
        region
    }

    fn check_dirty_clipping(stroked: bool) {
        let background = Rgb565Pixel(0xffff);
        let initial = Buffer(vec![background; 40 * 32]);
        let commands = [
            Command::MoveTo(zeno::Point::new(2.51, 3.27)),
            Command::LineTo(zeno::Point::new(30.29, 23.51)),
            Command::LineTo(zeno::Point::new(5.37, 24.49)),
            Command::Close,
        ];
        let color = PremultipliedRgbaColor {
            red: 20,
            green: 70,
            blue: 110,
            alpha: 128,
        };
        let path = euclid::rect(-2, 1, 36, 29);
        let clip = euclid::rect(0, 0, 40, 32);
        // Overlapping rectangles must not blend twice; the third is disjoint.
        let rects = [
            euclid::rect(3, 4, 15, 13),
            euclid::rect(10, 8, 13, 15),
            euclid::rect(26, 22, 6, 6),
        ];
        let dirty = region(&rects);
        let mut rasterizer = PathRasterizer::default();
        let mut render = |buffer: &mut Buffer, dirty: &PhysicalRegion| {
            if stroked {
                render_stroked_path(
                    &commands,
                    &path,
                    &clip,
                    dirty,
                    color,
                    2.0,
                    i_slint_core::items::LineCap::Round,
                    i_slint_core::items::LineJoin::Round,
                    4.0,
                    buffer,
                    &mut rasterizer,
                );
            } else {
                render_filled_path(&commands, &path, &clip, dirty, color, buffer, &mut rasterizer);
            }
        };
        let mut full = initial.clone();
        render(&mut full, &region(&[clip]));
        let mut partial = initial.clone();
        render(&mut partial, &dirty);
        let mut inside_changes = 0;
        let mut outside_changes = 0;
        for (index, pixel) in partial.0.iter().enumerate() {
            let pos = euclid::point2((index % 40) as i16, (index / 40) as i16);
            if rects.iter().any(|r| r.contains(pos)) {
                assert_eq!(
                    *pixel, full.0[index],
                    "dirty pixel {pos:?} was blended incorrectly"
                );
                inside_changes += usize::from(*pixel != background);
            } else {
                assert_eq!(*pixel, background, "retained pixel {pos:?} was modified");
                outside_changes += usize::from(full.0[index] != background);
            }
        }
        assert!(inside_changes > 20 && outside_changes > 20);
        let mut empty = initial.clone();
        render(&mut empty, &PhysicalRegion::default());
        assert_eq!(empty.0, initial.0);
    }

    #[test]
    fn filled_paths_blend_only_the_union_of_dirty_regions() {
        check_dirty_clipping(false);
    }

    #[test]
    fn stroked_paths_blend_only_the_union_of_dirty_regions() {
        check_dirty_clipping(true);
    }

    #[test]
    fn reused_masks_match_fresh_rasterization_after_size_and_style_changes() {
        let mut rasterizer = PathRasterizer::default();
        let commands = [
            Command::MoveTo(zeno::Point::new(-2.51, 3.27)),
            Command::CurveTo(
                zeno::Point::new(37.29, -5.51),
                zeno::Point::new(5.37, 24.49),
                zeno::Point::new(28.31, 31.17),
            ),
            Command::LineTo(zeno::Point::new(4.39, 25.01)),
            Command::Close,
        ];
        let mut stroke = Stroke::new(2.0);
        stroke.cap(Cap::Round).join(Join::Round);
        // Large -> small -> same-area/different-pitch -> large, with empty paths
        // interleaved, catches old coverage leaking through reused mask capacity.
        for (width, height) in [(40, 36), (12, 9), (9, 12), (48, 40), (40, 36)] {
            for style in [Style::Stroke(stroke), Style::Fill(Fill::NonZero)] {
                for path in [&commands[..], &[][..], &commands[..2]] {
                    let mut fresh = vec![0; width * height];
                    Mask::new(path)
                        .size(width as u32, height as u32)
                        .style(style)
                        .render_into(&mut fresh, None);
                    assert_eq!(
                        rasterizer.rasterize(path, width, height, style),
                        fresh,
                        "reused {width}x{height} mask differs from fresh Zeno output"
                    );
                }
            }
        }
    }
}
