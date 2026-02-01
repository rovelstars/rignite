extern crate alloc;

use crate::graphics::UefiDisplay;
use embedded_graphics::{pixelcolor::Rgb888, prelude::*};
use fontdue::{Font, FontSettings};

pub struct FontRenderer {
    font: Font,
}

impl FontRenderer {
    pub fn new(font_data: &[u8]) -> Self {
        let font =
            Font::from_bytes(font_data, FontSettings::default()).expect("Failed to load font");
        Self { font }
    }

    pub fn get_text_width(&self, text: &str, scale_px: f32) -> i32 {
        let mut width = 0.0;
        for c in text.chars() {
            if c.is_control() {
                continue;
            }
            let metrics = self.font.metrics(c, scale_px);
            width += metrics.advance_width;
        }
        width as i32
    }

    pub fn draw_text(
        &self,
        display: &mut UefiDisplay,
        text: &str,
        x: i32,
        y: i32,
        scale_px: f32,
        color: Rgb888,
    ) {
        let metrics = self
            .font
            .horizontal_line_metrics(scale_px)
            .unwrap_or_else(|| fontdue::LineMetrics {
                ascent: scale_px * 0.8,
                descent: -scale_px * 0.2,
                line_gap: 0.0,
                new_line_size: scale_px,
            });

        let baseline = y as f32 + metrics.ascent;
        let mut cur_x = x as f32;
        let mut glyph_count = 0;
        let mut pixel_count = 0;

        crate::debug!(
            "FontRenderer: drawing '{}' at ({},{}) color=({},{},{})",
            text,
            x,
            y,
            color.r(),
            color.g(),
            color.b()
        );

        for c in text.chars() {
            if c.is_control() {
                continue;
            }

            let (metrics, bitmap) = self.font.rasterize(c, scale_px);

            if metrics.width > 0 {
                glyph_count += 1;
                // fontdue coordinates: +Y is up. Screen coordinates: +Y is down.
                // Baseline is at `baseline` (screen Y).
                // Glyph top relative to baseline is metrics.ymin + metrics.height.
                // In screen Y: baseline - (ymin + height).
                let glyph_top_y = baseline - (metrics.ymin as f32 + metrics.height as f32);
                let glyph_left_x = cur_x + metrics.xmin as f32;

                for (i, v) in bitmap.iter().enumerate() {
                    let coverage = *v as f32 / 255.0;
                    if coverage > 0.01 {
                        let px = (i % metrics.width) as i32;
                        let py = (i / metrics.width) as i32;

                        let pixel_x = glyph_left_x as i32 + px;
                        let pixel_y = glyph_top_y as i32 + py;

                        let blended_color = Rgb888::new(
                            (color.r() as f32 * coverage) as u8,
                            (color.g() as f32 * coverage) as u8,
                            (color.b() as f32 * coverage) as u8,
                        );

                        let _ = Pixel(Point::new(pixel_x, pixel_y), blended_color).draw(display);
                        pixel_count += 1;
                    }
                }
            }

            cur_x += metrics.advance_width;
        }

        crate::debug!(
            "FontRenderer: drew {} glyphs, {} pixels total",
            glyph_count,
            pixel_count
        );
    }
}
