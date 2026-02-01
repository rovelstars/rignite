use crate::graphics::UefiDisplay;
use embedded_graphics::{pixelcolor::Rgb888, prelude::*};

// Normalized vertices (0.0 .. 1.0) derived from the SVG path data
const POLY1: &[(f32, f32)] = &[
    (0.8817402, 0.38227734),
    (0.7071699, 0.55684376),
    (0.70656055, 0.55623436),
    (0.4989121, 0.76388085),
    (0.4997754, 0.76474416),
    (0.49946484, 0.7645527),
    (0.1171875, 1.0),
    (0.38999414, 0.65508205),
    (0.1171875, 0.38227734),
    (0.49946484, 0.0),
];

const POLY2: &[(f32, f32)] = &[
    (0.8817402, 1.0),
    (0.5413223, 0.7903339),
    (0.63880664, 0.69285154),
];

const POLY3: &[(f32, f32)] = &[
    (0.3257031, 0.3831465),
    (0.49859765, 0.556041),
    (0.6714922, 0.3831465),
    (0.49859765, 0.21025196),
];

pub struct Logo;

impl Logo {
    pub fn new() -> Self {
        Self
    }

    pub fn draw(&self, display: &mut UefiDisplay, x: i32, y: i32, size: u32, opacity: f32) {
        let size_f = size as f32;
        // Gradient Colors: #9333EA to #DB2777
        let start_color = (147.0, 51.0, 234.0);
        let end_color = (219.0, 39.0, 119.0);

        for dy in 0..size {
            for dx in 0..size {
                // 2x2 Supersampling for basic anti-aliasing
                let mut coverage = 0.0;
                let offsets = [0.25, 0.75];

                for oy in offsets {
                    for ox in offsets {
                        let uv_x = (dx as f32 + ox) / size_f;
                        let uv_y = (dy as f32 + oy) / size_f;

                        let in_p1 = is_inside(POLY1, uv_x, uv_y);
                        let in_p2 = is_inside(POLY2, uv_x, uv_y);
                        let in_p3 = is_inside(POLY3, uv_x, uv_y);

                        // Shape logic: (Poly1 minus Poly3) union Poly2
                        if (in_p1 && !in_p3) || in_p2 {
                            coverage += 0.25;
                        }
                    }
                }

                if coverage > 0.0 {
                    let uv_y = (dy as f32 + 0.5) / size_f; // Use pixel center for gradient

                    // Calculate Vertical Linear Gradient
                    let r = start_color.0 + (end_color.0 - start_color.0) * uv_y;
                    let g = start_color.1 + (end_color.1 - start_color.1) * uv_y;
                    let b = start_color.2 + (end_color.2 - start_color.2) * uv_y;

                    // Combine coverage (AA) with global opacity
                    let final_opacity = opacity * coverage;

                    // Blend with black background (Color * Alpha)
                    let color = Rgb888::new(
                        (r * final_opacity) as u8,
                        (g * final_opacity) as u8,
                        (b * final_opacity) as u8,
                    );

                    let _ = Pixel(Point::new(x + dx as i32, y + dy as i32), color).draw(display);
                }
            }
        }
    }
}

// Ray-casting algorithm for point-in-polygon check
fn is_inside(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];

        let intersect = ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi);

        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}
