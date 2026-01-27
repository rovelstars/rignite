extern crate alloc;

use embedded_graphics::{
    prelude::*,
    pixelcolor::Rgb888,
};
use ab_glyph::{FontRef, Font, PxScale, point, ScaleFont};
use crate::graphics::UefiDisplay;

// Use FontRef directly to avoid cloning data
pub struct FontRenderer<'a> {
    font: FontRef<'a>,
}

type Idx = i32;

impl<'a> FontRenderer<'a> {
    pub fn new(font_data: &'a [u8]) -> Self {
        let font = FontRef::try_from_slice(font_data).expect("Failed to load font");
        Self { font }
    }

    pub fn draw_text(&self, display: &mut UefiDisplay, text: &str, x: i32, y: i32, scale_px: f32, color: Rgb888) {
        let scale = PxScale::from(scale_px);
        let scaled_font = self.font.as_scaled(scale);
        
        let mut caret = point(x as f32, y as f32 + scaled_font.ascent());
        let mut glyph_count = 0;
        let mut pixel_count = 0;

        log::info!("FontRenderer: drawing '{}' at ({},{}) scale={} color=({},{},{})", 
            text, x, y, scale_px, color.r(), color.g(), color.b());

        for c in text.chars() {
            if c.is_control() {
                continue;
            }

            // Get glyph ID and create positioned glyph
            let glyph_id = self.font.glyph_id(c);
            let glyph = glyph_id.with_scale_and_position(scale, caret);
            
            if let Some(outlined) = self.font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                glyph_count += 1;
                
                // Draw glyph with proper antialiasing
                outlined.draw(|px, py, v| {
                     if v > 0.01 {  // Minimal threshold to skip fully transparent pixels
                         let pixel_x = (bounds.min.x as Idx) + (px as Idx);
                         let pixel_y = (bounds.min.y as Idx) + (py as Idx);
                         
                         // Alpha blend: foreground * alpha + background * (1 - alpha)
                         // For black background (0,0,0), this simplifies to: color * v
                         let blended_color = Rgb888::new(
                             (color.r() as f32 * v) as u8,
                             (color.g() as f32 * v) as u8,
                             (color.b() as f32 * v) as u8,
                         );
                         
                         let _ = Pixel(Point::new(pixel_x, pixel_y), blended_color).draw(display);
                         pixel_count += 1;
                     }
                });
            }
            
            // Advance caret
            let h_advance = scaled_font.h_advance(glyph_id);
            caret.x += h_advance;
        }
        
        log::info!("FontRenderer: drew {} glyphs, {} pixels total", glyph_count, pixel_count);
    }
}
