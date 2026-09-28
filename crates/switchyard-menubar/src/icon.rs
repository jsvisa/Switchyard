// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The menu bar glyph, drawn in code.
//!
//! macOS scales the image to 18 points tall, so the source is drawn at 2x for
//! retina displays. It is a template image: only the alpha channel matters,
//! and the system recolors it for light and dark menu bars.

/// Side length of the generated image, in pixels.
pub const SIZE: u32 = 36;

/// Half the stroke width, in pixels.
const HALF_STROKE: f64 = 1.7;

/// A track that splits in two: one request in, two tiers out.
const SEGMENTS: [((f64, f64), (f64, f64)); 3] = [
    ((5.0, 18.0), (17.0, 18.0)),
    ((17.0, 18.0), (30.0, 7.0)),
    ((17.0, 18.0), (30.0, 29.0)),
];

/// Renders the glyph as RGBA pixels, row-major from the top left.
pub fn glyph() -> Vec<u8> {
    let mut pixels = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let point = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            let distance = SEGMENTS
                .iter()
                .map(|(start, end)| distance_to_segment(point, *start, *end))
                .fold(f64::INFINITY, f64::min);
            // One pixel of falloff past the stroke edge softens the diagonals.
            let coverage = (HALF_STROKE + 0.5 - distance).clamp(0.0, 1.0);
            pixels.extend_from_slice(&[0, 0, 0, (coverage * 255.0).round() as u8]);
        }
    }
    pixels
}

fn distance_to_segment(point: (f64, f64), start: (f64, f64), end: (f64, f64)) -> f64 {
    let segment = (end.0 - start.0, end.1 - start.1);
    let length_squared = segment.0 * segment.0 + segment.1 * segment.1;
    let to_point = (point.0 - start.0, point.1 - start.1);
    let projection = if length_squared == 0.0 {
        0.0
    } else {
        ((to_point.0 * segment.0 + to_point.1 * segment.1) / length_squared).clamp(0.0, 1.0)
    };
    let closest = (
        start.0 + segment.0 * projection,
        start.1 + segment.1 * projection,
    );
    ((point.0 - closest.0).powi(2) + (point.1 - closest.1).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(pixels: &[u8], x: u32, y: u32) -> u8 {
        pixels[((y * SIZE + x) * 4 + 3) as usize]
    }

    #[test]
    fn renders_a_full_rgba_buffer() {
        let pixels = glyph();

        assert_eq!(pixels.len(), (SIZE * SIZE * 4) as usize);
    }

    #[test]
    fn draws_the_trunk_and_leaves_the_corners_clear() {
        let pixels = glyph();

        assert_eq!(alpha(&pixels, 10, 18), 255, "the trunk is solid");
        assert_eq!(alpha(&pixels, 0, 0), 0, "corners stay transparent");
        assert_eq!(alpha(&pixels, SIZE - 1, SIZE - 1), 0);
    }

    #[test]
    fn draws_both_branches() {
        let pixels = glyph();

        assert!(alpha(&pixels, 29, 7) > 0, "upper branch reaches its end");
        assert!(alpha(&pixels, 29, 29) > 0, "lower branch reaches its end");
        assert_eq!(alpha(&pixels, 29, 18), 0, "nothing runs straight through");
    }
}
