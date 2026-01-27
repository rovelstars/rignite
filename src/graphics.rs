use embedded_graphics::{
    draw_target::DrawTarget,
    geometry::Size,
    pixelcolor::{Bgr888, Rgb888},
    prelude::*,
};
use uefi::proto::console::gop::{GraphicsOutput, PixelFormat};

pub struct UefiDisplay<'a> {
    gop: &'a mut GraphicsOutput,
}

impl<'a> UefiDisplay<'a> {
    pub fn new(gop: &'a mut GraphicsOutput) -> Self {
        Self { gop }
    }

    pub fn flush(&mut self) {
        // No-op
    }

    pub fn width(&self) -> u32 {
        self.gop.current_mode_info().resolution().0 as u32
    }

    pub fn height(&self) -> u32 {
        self.gop.current_mode_info().resolution().1 as u32
    }

    pub fn clear(&mut self, color: Rgb888) -> uefi::Result {
        let (w, h) = self.gop.current_mode_info().resolution();
        let pixel = uefi::proto::console::gop::BltPixel::new(color.r(), color.g(), color.b());

        self.gop.blt(uefi::proto::console::gop::BltOp::VideoFill {
            color: pixel,
            dest: (0, 0),
            dims: (w, h),
        })
    }

    pub fn set_highest_resolution(&mut self) -> uefi::Result {
        log::info!("Querying video modes...");
        let mode = self
            .gop
            .modes()
            .filter(|m| {
                let (w, h) = m.info().resolution();
                // Filter out excessive resolutions that might crash QEMU/VirtIO or exhaust ramfb
                w <= 1280 && h <= 720
            })
            .max_by_key(|m| {
                let info = m.info();
                let (w, h) = info.resolution();
                log::info!("Found mode: {}x{}", w, h);
                w * h
            });

        if let Some(mode) = mode {
            let info = mode.info();
            let (w, h) = info.resolution();
            log::info!("Setting mode: {}x{}", w, h);
            self.gop.set_mode(&mode)?;
            let info = self.gop.current_mode_info();
            log::info!("Mode set successfully. Format: {:?}", info.pixel_format());
        } else {
            log::warn!("No video modes found or all filtered out.");
        }
        Ok(())
    }
}

impl DrawTarget for UefiDisplay<'_> {
    type Color = Rgb888;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        let mode_info = self.gop.current_mode_info();
        let (width, height) = mode_info.resolution();
        let pixel_format = mode_info.pixel_format();

        for Pixel(coord, color) in pixels.into_iter() {
            if coord.x >= 0 && coord.x < width as i32 && coord.y >= 0 && coord.y < height as i32 {
                let x = coord.x as usize;
                let y = coord.y as usize;

                // If BltOnly (or safe fallback), use Blt
                if pixel_format == PixelFormat::BltOnly {
                    let pixel =
                        uefi::proto::console::gop::BltPixel::new(color.r(), color.g(), color.b());
                    let _ = self.gop.blt(uefi::proto::console::gop::BltOp::VideoFill {
                        color: pixel,
                        dest: (x, y),
                        dims: (1, 1),
                    });
                    continue;
                }

                let mut framebuffer = self.gop.frame_buffer();
                let stride = mode_info.stride();
                let index = (y * stride + x) * 4;

                let (r, g, b) = (color.r(), color.g(), color.b());

                // Write to framebuffer
                unsafe {
                    match pixel_format {
                        PixelFormat::Rgb => {
                            framebuffer.write_value(index, r);
                            framebuffer.write_value(index + 1, g);
                            framebuffer.write_value(index + 2, b);
                        }
                        PixelFormat::Bgr => {
                            framebuffer.write_value(index, b);
                            framebuffer.write_value(index + 1, g);
                            framebuffer.write_value(index + 2, r);
                        }
                        _ => {
                            // Fallback for BitMask, etc. (Assume BGR/RGB 32 for now, or use Blt)
                            // If we are here, it's not BltOnly, but might be BitMask.
                            // Safest to default to Blt for unknown formats too?
                            // Let's use Blt for anything not explicitly RGB/BGR to be safe.
                            let pixel = uefi::proto::console::gop::BltPixel::new(
                                color.r(),
                                color.g(),
                                color.b(),
                            );
                            let _ = self.gop.blt(uefi::proto::console::gop::BltOp::VideoFill {
                                color: pixel,
                                dest: (x, y),
                                dims: (1, 1),
                            });
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

impl OriginDimensions for UefiDisplay<'_> {
    fn size(&self) -> Size {
        let (w, h) = self.gop.current_mode_info().resolution();
        Size::new(w as u32, h as u32)
    }
}
