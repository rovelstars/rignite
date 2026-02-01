use embedded_graphics::{draw_target::DrawTarget, geometry::Size, pixelcolor::Rgb888, prelude::*};
use uefi::proto::console::gop::{GraphicsOutput, PixelFormat};

use alloc::vec;
use alloc::vec::Vec;

pub struct UefiDisplay<'a> {
    gop: &'a mut GraphicsOutput,
    backbuffer: Vec<u8>,
}

impl<'a> UefiDisplay<'a> {
    pub fn new(gop: &'a mut GraphicsOutput) -> Self {
        let info = gop.current_mode_info();
        let (_, height) = info.resolution();
        let stride = info.stride();
        let len = (height * stride * 4) as usize;
        let backbuffer = vec![0; len];
        Self { gop, backbuffer }
    }

    pub fn flush(&mut self) {
        let mut fb = self.gop.frame_buffer();
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.backbuffer.as_ptr(),
                fb.as_mut_ptr(),
                self.backbuffer.len(),
            );
        }
    }

    pub fn width(&self) -> u32 {
        self.gop.current_mode_info().resolution().0 as u32
    }

    pub fn height(&self) -> u32 {
        self.gop.current_mode_info().resolution().1 as u32
    }

    pub fn clear(&mut self, color: Rgb888) -> uefi::Result {
        let mode_info = self.gop.current_mode_info();
        let pixel_format = mode_info.pixel_format();
        let (r, g, b) = (color.r(), color.g(), color.b());

        for chunk in self.backbuffer.chunks_exact_mut(4) {
            match pixel_format {
                PixelFormat::Rgb => {
                    chunk[0] = r;
                    chunk[1] = g;
                    chunk[2] = b;
                }
                _ => {
                    chunk[0] = b;
                    chunk[1] = g;
                    chunk[2] = r;
                }
            }
        }
        Ok(())
    }

    pub fn set_highest_resolution(&mut self) -> uefi::Result {
        crate::info!("Querying video modes...");
        let mode = self
            .gop
            .modes()
            .filter(|m| {
                let (w, h) = m.info().resolution();
                // Filter out excessive resolutions that might crash QEMU/VirtIO or exhaust ramfb
                w <= 1920 && h <= 1200
            })
            .max_by_key(|m| {
                let info = m.info();
                let (w, h) = info.resolution();
                crate::info!("Found mode: {}x{}", w, h);
                w * h
            });

        if let Some(mode) = mode {
            let info = mode.info();
            let (w, h) = info.resolution();
            crate::info!("Setting mode: {}x{}", w, h);
            self.gop.set_mode(&mode)?;
            let info = self.gop.current_mode_info();
            let (_, height) = info.resolution();
            let stride = info.stride();
            let len = (height * stride * 4) as usize;
            self.backbuffer = vec![0; len];
            crate::info!("Mode set successfully. Format: {:?}", info.pixel_format());
        } else {
            crate::warn!("No video modes found or all filtered out.");
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

        let stride = mode_info.stride();

        for Pixel(coord, color) in pixels.into_iter() {
            if coord.x >= 0 && coord.x < width as i32 && coord.y >= 0 && coord.y < height as i32 {
                let x = coord.x as usize;
                let y = coord.y as usize;
                let index = (y * stride + x) * 4;

                if index + 2 < self.backbuffer.len() {
                    let (r, g, b) = (color.r(), color.g(), color.b());
                    match pixel_format {
                        PixelFormat::Rgb => {
                            self.backbuffer[index] = r;
                            self.backbuffer[index + 1] = g;
                            self.backbuffer[index + 2] = b;
                        }
                        _ => {
                            self.backbuffer[index] = b;
                            self.backbuffer[index + 1] = g;
                            self.backbuffer[index + 2] = r;
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
