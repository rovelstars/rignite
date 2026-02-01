use crate::graphics::UefiDisplay;
use embedded_graphics::{pixelcolor::Rgb888, prelude::*};

// Fixed 512x512 source size as per build.py (ultra high quality)
const ICON_SIZE: u32 = 512;

pub struct Icon<'a> {
    data: &'a [u8],
}

impl<'a> Icon<'a> {
    pub fn new(data: &'a [u8], _w: u32, _h: u32) -> Self {
        // Ignore w/h args, assume they define source size or just use constant
        // For backwards compat with main.rs call, keep args but rely on ICON_SIZE
        Self { data }
    }

    pub fn draw_scaled(
        &self,
        display: &mut UefiDisplay,
        x: i32,
        y: i32,
        target_size: u32,
        opacity: f32,
    ) {
        // Bicubic scaling (Catmull-Rom) - good balance of speed and quality
        let scale_factor = (ICON_SIZE as f32) / (target_size as f32);

        // Bicubic kernel
        let cubic = |x: f32| -> f32 {
            let x = x.abs();
            if x < 1.0 {
                1.5 * x * x * x - 2.5 * x * x + 1.0
            } else if x < 2.0 {
                -0.5 * x * x * x + 2.5 * x * x - 4.0 * x + 2.0
            } else {
                0.0
            }
        };

        for ty in 0..target_size {
            for tx in 0..target_size {
                let src_x = (tx as f32 + 0.5) * scale_factor - 0.5;
                let src_y = (ty as f32 + 0.5) * scale_factor - 0.5;

                let x0 = if src_x >= 0.0 {
                    src_x as i32
                } else {
                    (src_x - 1.0) as i32
                };
                let y0 = if src_y >= 0.0 {
                    src_y as i32
                } else {
                    (src_y - 1.0) as i32
                };

                let mut r_sum = 0.0f32;
                let mut g_sum = 0.0f32;
                let mut b_sum = 0.0f32;
                let mut a_sum = 0.0f32;
                let mut weight_sum = 0.0f32;

                // Sample 4x4 neighborhood for bicubic
                for dy in -1..=2 {
                    for dx in -1..=2 {
                        let sample_x = (x0 + dx).max(0).min(ICON_SIZE as i32 - 1) as u32;
                        let sample_y = (y0 + dy).max(0).min(ICON_SIZE as i32 - 1) as u32;

                        let idx = ((sample_y * ICON_SIZE + sample_x) * 4) as usize;
                        if idx + 3 >= self.data.len() {
                            continue;
                        }

                        let weight_x = cubic(src_x - (x0 + dx) as f32);
                        let weight_y = cubic(src_y - (y0 + dy) as f32);
                        let weight = weight_x * weight_y;

                        r_sum += self.data[idx] as f32 * weight;
                        g_sum += self.data[idx + 1] as f32 * weight;
                        b_sum += self.data[idx + 2] as f32 * weight;
                        a_sum += self.data[idx + 3] as f32 * weight;
                        weight_sum += weight;
                    }
                }

                if weight_sum > 0.0 {
                    let r = (r_sum / weight_sum).max(0.0).min(255.0) as u8;
                    let g = (g_sum / weight_sum).max(0.0).min(255.0) as u8;
                    let b = (b_sum / weight_sum).max(0.0).min(255.0) as u8;
                    let a = (a_sum / weight_sum).max(0.0).min(255.0) as u8;

                    // Proper alpha blending
                    if a > 1 {
                        let alpha_factor = (a as f32 / 255.0) * opacity;
                        let blended = Rgb888::new(
                            (r as f32 * alpha_factor) as u8,
                            (g as f32 * alpha_factor) as u8,
                            (b as f32 * alpha_factor) as u8,
                        );
                        let _ =
                            Pixel(Point::new(x + tx as i32, y + ty as i32), blended).draw(display);
                    }
                }
            }
        }
    }

    // Kept for compatibility, but redirects to scaled logic with default size?
    // Actually we want dynamic size.
    pub fn draw(&self, display: &mut UefiDisplay, x: i32, y: i32) {
        self.draw_scaled(display, x, y, 64, 1.0);
    }
}
