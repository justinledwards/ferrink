//! Fail-closed 8-bit grayscale framebuffer layout and memory-slice writes.

use std::num::NonZeroU32;

use crate::{DisplayExtent, FramebufferCapability, PixelLayout, RefreshRegion};

/// One toolkit-neutral 24-bit RGB source pixel.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Rgb8Pixel {
    /// Red channel.
    pub red: u8,
    /// Green channel.
    pub green: u8,
    /// Blue channel.
    pub blue: u8,
}

/// Conversion policy from RGB renderer output to an 8-bit grayscale panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gray8Conversion {
    /// Preserve an integer approximation of perceptual luminance.
    Grayscale,
    /// Quantize luminance to black or white at the inclusive threshold.
    Bilevel {
        /// Luminance values at or above this threshold become white.
        threshold: u8,
    },
}

impl Gray8Conversion {
    fn convert(self, pixel: Rgb8Pixel) -> u8 {
        let luminance = (u32::from(pixel.red) * 77
            + u32::from(pixel.green) * 150
            + u32::from(pixel.blue) * 29
            + 128)
            >> 8;
        let luminance = u8::try_from(luminance).unwrap_or(u8::MAX);
        match self {
            Self::Grayscale => luminance,
            Self::Bilevel { threshold } => {
                if luminance >= threshold {
                    u8::MAX
                } else {
                    u8::MIN
                }
            }
        }
    }
}

/// A validated byte layout for an 8-bit grayscale Linux framebuffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gray8FramebufferLayout {
    visible: DisplayExtent,
    virtual_extent: DisplayExtent,
    x_offset: u32,
    y_offset: u32,
    line_length: NonZeroU32,
    memory_length: NonZeroU32,
}

impl Gray8FramebufferLayout {
    /// Validates a grayscale framebuffer layout without opening or mapping it.
    ///
    /// # Errors
    ///
    /// Returns [`FramebufferLayoutError`] if the visible surface plus offset
    /// leaves the virtual surface, row stride is too short, memory is too
    /// small, or coordinate arithmetic overflows.
    pub fn try_new(
        visible: DisplayExtent,
        virtual_extent: DisplayExtent,
        x_offset: u32,
        y_offset: u32,
        line_length: NonZeroU32,
        memory_length: NonZeroU32,
    ) -> Result<Self, FramebufferLayoutError> {
        let visible_right = x_offset
            .checked_add(visible.width())
            .ok_or(FramebufferLayoutError::VisibleOffsetOverflow)?;
        let visible_bottom = y_offset
            .checked_add(visible.height())
            .ok_or(FramebufferLayoutError::VisibleOffsetOverflow)?;
        if visible_right > virtual_extent.width() || visible_bottom > virtual_extent.height() {
            return Err(FramebufferLayoutError::VisibleOutsideVirtual);
        }
        if line_length.get() < virtual_extent.width() {
            return Err(FramebufferLayoutError::LineLengthTooShort {
                observed: line_length.get(),
                minimum: virtual_extent.width(),
            });
        }
        let minimum_memory_length = u64::from(line_length.get())
            .checked_mul(u64::from(virtual_extent.height()))
            .ok_or(FramebufferLayoutError::ArithmeticOverflow)?;
        if u64::from(memory_length.get()) < minimum_memory_length {
            return Err(FramebufferLayoutError::MemoryLengthTooShort {
                observed: memory_length.get(),
                minimum: minimum_memory_length,
            });
        }
        Ok(Self {
            visible,
            virtual_extent,
            x_offset,
            y_offset,
            line_length,
            memory_length,
        })
    }

    /// Builds a validated layout from one passive framebuffer capability.
    ///
    /// # Errors
    ///
    /// Returns [`FramebufferLayoutError`] unless the capability describes a
    /// complete 8-bit grayscale layout with non-zero extents, stride, and
    /// memory length.
    pub fn try_from_capability(
        capability: &FramebufferCapability,
    ) -> Result<Self, FramebufferLayoutError> {
        if capability.bits_per_pixel != 8 || capability.pixel_layout != PixelLayout::Grayscale8 {
            return Err(FramebufferLayoutError::UnsupportedPixelFormat {
                bits_per_pixel: capability.bits_per_pixel,
                pixel_layout: capability.pixel_layout,
            });
        }
        let visible = DisplayExtent::try_new(capability.visible_width, capability.visible_height)
            .map_err(|_| FramebufferLayoutError::InvalidVisibleExtent)?;
        let virtual_extent =
            DisplayExtent::try_new(capability.virtual_width, capability.virtual_height)
                .map_err(|_| FramebufferLayoutError::InvalidVirtualExtent)?;
        let line_length = NonZeroU32::new(capability.line_length)
            .ok_or(FramebufferLayoutError::ZeroLineLength)?;
        let memory_length = NonZeroU32::new(capability.memory_length)
            .ok_or(FramebufferLayoutError::ZeroMemoryLength)?;
        Self::try_new(
            visible,
            virtual_extent,
            capability.x_offset,
            capability.y_offset,
            line_length,
            memory_length,
        )
    }

    /// Returns the visible display extent in physical framebuffer coordinates.
    #[must_use]
    pub const fn visible(self) -> DisplayExtent {
        self.visible
    }

    /// Returns the complete virtual framebuffer extent.
    #[must_use]
    pub const fn virtual_extent(self) -> DisplayExtent {
        self.virtual_extent
    }

    /// Returns the visible horizontal offset inside the virtual framebuffer.
    #[must_use]
    pub const fn x_offset(self) -> u32 {
        self.x_offset
    }

    /// Returns the visible vertical offset inside the virtual framebuffer.
    #[must_use]
    pub const fn y_offset(self) -> u32 {
        self.y_offset
    }

    /// Returns the kernel-reported byte stride for one virtual row.
    #[must_use]
    pub const fn line_length(self) -> u32 {
        self.line_length.get()
    }

    /// Returns the kernel-reported framebuffer memory length.
    #[must_use]
    pub const fn memory_length(self) -> u32 {
        self.memory_length.get()
    }

    /// Converts and writes one validated physical dirty region into memory.
    ///
    /// The target is an ordinary byte slice; this function does not open or
    /// map a framebuffer and does not submit a refresh. `source_stride_pixels`
    /// permits a renderer row to contain padding. All bounds are checked before
    /// the first target byte is changed.
    ///
    /// # Errors
    ///
    /// Returns [`FramebufferWriteError`] if the region does not belong to this
    /// layout, a stride or slice is too short, or address arithmetic cannot be
    /// represented safely. On error, `target` is unchanged.
    pub fn write_rgb8_region(
        self,
        target: &mut [u8],
        region: RefreshRegion,
        source: &[Rgb8Pixel],
        source_stride_pixels: NonZeroU32,
        conversion: Gray8Conversion,
    ) -> Result<(), FramebufferWriteError> {
        let region_right = region
            .x()
            .checked_add(region.width())
            .ok_or(FramebufferWriteError::ArithmeticOverflow)?;
        let region_bottom = region
            .y()
            .checked_add(region.height())
            .ok_or(FramebufferWriteError::ArithmeticOverflow)?;
        if region_right > self.visible.width() || region_bottom > self.visible.height() {
            return Err(FramebufferWriteError::RegionOutsideLayout);
        }
        if source_stride_pixels.get() < region.width() {
            return Err(FramebufferWriteError::SourceStrideTooShort {
                observed: source_stride_pixels.get(),
                minimum: region.width(),
            });
        }

        let width = usize::try_from(region.width())
            .map_err(|_| FramebufferWriteError::ArithmeticOverflow)?;
        let height = usize::try_from(region.height())
            .map_err(|_| FramebufferWriteError::ArithmeticOverflow)?;
        let source_stride = usize::try_from(source_stride_pixels.get())
            .map_err(|_| FramebufferWriteError::ArithmeticOverflow)?;
        let required_source = height
            .saturating_sub(1)
            .checked_mul(source_stride)
            .and_then(|offset| offset.checked_add(width))
            .ok_or(FramebufferWriteError::ArithmeticOverflow)?;
        if source.len() < required_source {
            return Err(FramebufferWriteError::SourceTooShort {
                observed: source.len(),
                minimum: required_source,
            });
        }

        let required_target = usize::try_from(self.memory_length.get())
            .map_err(|_| FramebufferWriteError::ArithmeticOverflow)?;
        if target.len() < required_target {
            return Err(FramebufferWriteError::TargetTooShort {
                observed: target.len(),
                minimum: required_target,
            });
        }

        let target_stride = usize::try_from(self.line_length.get())
            .map_err(|_| FramebufferWriteError::ArithmeticOverflow)?;
        let target_x = self
            .x_offset
            .checked_add(region.x())
            .and_then(|value| usize::try_from(value).ok())
            .ok_or(FramebufferWriteError::ArithmeticOverflow)?;
        let target_y = self
            .y_offset
            .checked_add(region.y())
            .and_then(|value| usize::try_from(value).ok())
            .ok_or(FramebufferWriteError::ArithmeticOverflow)?;
        let final_target_end = target_y
            .checked_add(height.saturating_sub(1))
            .and_then(|row| row.checked_mul(target_stride))
            .and_then(|offset| offset.checked_add(target_x))
            .and_then(|offset| offset.checked_add(width))
            .ok_or(FramebufferWriteError::ArithmeticOverflow)?;
        if final_target_end > required_target {
            return Err(FramebufferWriteError::RegionOutsideLayout);
        }

        for row in 0..height {
            let source_start = row * source_stride;
            let target_start = (target_y + row) * target_stride + target_x;
            let source_row = &source[source_start..source_start + width];
            let target_row = &mut target[target_start..target_start + width];
            for (target_pixel, source_pixel) in target_row.iter_mut().zip(source_row) {
                *target_pixel = conversion.convert(*source_pixel);
            }
        }
        Ok(())
    }
}

/// An invalid passive framebuffer layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FramebufferLayoutError {
    /// The capability was not exactly 8-bit grayscale.
    UnsupportedPixelFormat {
        bits_per_pixel: u32,
        pixel_layout: PixelLayout,
    },
    /// Visible width or height was zero.
    InvalidVisibleExtent,
    /// Virtual width or height was zero.
    InvalidVirtualExtent,
    /// Kernel line stride was zero.
    ZeroLineLength,
    /// Kernel framebuffer memory length was zero.
    ZeroMemoryLength,
    /// Adding a visible offset and dimension overflowed.
    VisibleOffsetOverflow,
    /// The offset visible surface left the virtual surface.
    VisibleOutsideVirtual,
    /// A virtual row did not fit in the reported byte stride.
    LineLengthTooShort { observed: u32, minimum: u32 },
    /// The reported memory could not hold every virtual row.
    MemoryLengthTooShort { observed: u32, minimum: u64 },
    /// An intermediate layout calculation overflowed.
    ArithmeticOverflow,
}

impl std::fmt::Display for FramebufferLayoutError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPixelFormat {
                bits_per_pixel,
                pixel_layout,
            } => write!(
                formatter,
                "unsupported framebuffer format: {bits_per_pixel} bpp {pixel_layout:?}"
            ),
            Self::InvalidVisibleExtent => formatter.write_str("visible extent must be non-zero"),
            Self::InvalidVirtualExtent => formatter.write_str("virtual extent must be non-zero"),
            Self::ZeroLineLength => formatter.write_str("framebuffer line length must be non-zero"),
            Self::ZeroMemoryLength => {
                formatter.write_str("framebuffer memory length must be non-zero")
            }
            Self::VisibleOffsetOverflow => {
                formatter.write_str("visible framebuffer offset arithmetic overflow")
            }
            Self::VisibleOutsideVirtual => {
                formatter.write_str("visible framebuffer leaves the virtual extent")
            }
            Self::LineLengthTooShort { observed, minimum } => write!(
                formatter,
                "framebuffer line length {observed} is shorter than {minimum}"
            ),
            Self::MemoryLengthTooShort { observed, minimum } => write!(
                formatter,
                "framebuffer memory length {observed} is shorter than {minimum}"
            ),
            Self::ArithmeticOverflow => {
                formatter.write_str("framebuffer layout arithmetic overflow")
            }
        }
    }
}

impl std::error::Error for FramebufferLayoutError {}

/// A rejected memory-slice pixel write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FramebufferWriteError {
    /// The region was validated for a different or larger display.
    RegionOutsideLayout,
    /// The source row stride was shorter than the dirty-region width.
    SourceStrideTooShort { observed: u32, minimum: u32 },
    /// The source slice could not hold every requested dirty row.
    SourceTooShort { observed: usize, minimum: usize },
    /// The target slice was shorter than the validated framebuffer mapping.
    TargetTooShort { observed: usize, minimum: usize },
    /// Address or length arithmetic could not be represented safely.
    ArithmeticOverflow,
}

impl std::fmt::Display for FramebufferWriteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RegionOutsideLayout => {
                formatter.write_str("dirty region leaves the framebuffer layout")
            }
            Self::SourceStrideTooShort { observed, minimum } => write!(
                formatter,
                "source stride {observed} pixels is shorter than {minimum}"
            ),
            Self::SourceTooShort { observed, minimum } => write!(
                formatter,
                "source contains {observed} pixels; at least {minimum} are required"
            ),
            Self::TargetTooShort { observed, minimum } => write!(
                formatter,
                "target contains {observed} bytes; at least {minimum} are required"
            ),
            Self::ArithmeticOverflow => {
                formatter.write_str("framebuffer write arithmetic overflow")
            }
        }
    }
}

impl std::error::Error for FramebufferWriteError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn non_zero(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).unwrap()
    }

    fn layout() -> Gray8FramebufferLayout {
        Gray8FramebufferLayout::try_new(
            DisplayExtent::try_new(3, 2).unwrap(),
            DisplayExtent::try_new(5, 3).unwrap(),
            1,
            1,
            non_zero(8),
            non_zero(24),
        )
        .unwrap()
    }

    fn source_pixels() -> [Rgb8Pixel; 7] {
        [
            Rgb8Pixel {
                red: 0,
                green: 0,
                blue: 0,
            },
            Rgb8Pixel {
                red: 255,
                green: 0,
                blue: 0,
            },
            Rgb8Pixel {
                red: 0,
                green: 255,
                blue: 0,
            },
            Rgb8Pixel::default(),
            Rgb8Pixel {
                red: 0,
                green: 0,
                blue: 255,
            },
            Rgb8Pixel {
                red: 128,
                green: 128,
                blue: 128,
            },
            Rgb8Pixel {
                red: 255,
                green: 255,
                blue: 255,
            },
        ]
    }

    #[test]
    fn layout_rejects_offsets_stride_and_memory_that_cannot_hold_visible_rows() {
        let visible = DisplayExtent::try_new(3, 2).unwrap();
        let virtual_extent = DisplayExtent::try_new(5, 3).unwrap();
        assert_eq!(
            Gray8FramebufferLayout::try_new(
                visible,
                virtual_extent,
                3,
                0,
                non_zero(8),
                non_zero(24),
            ),
            Err(FramebufferLayoutError::VisibleOutsideVirtual)
        );
        assert_eq!(
            Gray8FramebufferLayout::try_new(
                visible,
                virtual_extent,
                0,
                0,
                non_zero(4),
                non_zero(24),
            ),
            Err(FramebufferLayoutError::LineLengthTooShort {
                observed: 4,
                minimum: 5,
            })
        );
        assert_eq!(
            Gray8FramebufferLayout::try_new(
                visible,
                virtual_extent,
                0,
                0,
                non_zero(8),
                non_zero(23),
            ),
            Err(FramebufferLayoutError::MemoryLengthTooShort {
                observed: 23,
                minimum: 24,
            })
        );
    }

    #[test]
    fn grayscale_write_honors_source_stride_virtual_offset_and_target_padding() {
        let mut target = [0xaa; 24];
        let region = RefreshRegion::full(layout().visible());
        layout()
            .write_rgb8_region(
                &mut target,
                region,
                &source_pixels(),
                non_zero(4),
                Gray8Conversion::Grayscale,
            )
            .unwrap();

        let mut expected = [0xaa; 24];
        expected[9..12].copy_from_slice(&[0, 77, 149]);
        expected[17..20].copy_from_slice(&[29, 128, 255]);
        assert_eq!(target, expected);
    }

    #[test]
    fn bilevel_write_quantizes_at_the_inclusive_threshold() {
        let mut target = [0xaa; 24];
        let region = RefreshRegion::try_new(0, 0, 3, 1, layout().visible()).unwrap();
        let source = [
            Rgb8Pixel {
                red: 127,
                green: 127,
                blue: 127,
            },
            Rgb8Pixel {
                red: 128,
                green: 128,
                blue: 128,
            },
            Rgb8Pixel {
                red: 255,
                green: 255,
                blue: 255,
            },
        ];
        layout()
            .write_rgb8_region(
                &mut target,
                region,
                &source,
                non_zero(3),
                Gray8Conversion::Bilevel { threshold: 128 },
            )
            .unwrap();
        assert_eq!(&target[9..12], &[0, 255, 255]);
    }

    #[test]
    fn every_error_is_detected_before_the_target_changes() {
        let region = RefreshRegion::full(layout().visible());
        let cases = [
            (
                vec![Rgb8Pixel::default(); 6],
                non_zero(4),
                vec![0xaa; 24],
                FramebufferWriteError::SourceTooShort {
                    observed: 6,
                    minimum: 7,
                },
            ),
            (
                source_pixels().to_vec(),
                non_zero(2),
                vec![0xaa; 24],
                FramebufferWriteError::SourceStrideTooShort {
                    observed: 2,
                    minimum: 3,
                },
            ),
            (
                source_pixels().to_vec(),
                non_zero(4),
                vec![0xaa; 23],
                FramebufferWriteError::TargetTooShort {
                    observed: 23,
                    minimum: 24,
                },
            ),
        ];

        for (source, stride, mut target, expected_error) in cases {
            let before = target.clone();
            assert_eq!(
                layout().write_rgb8_region(
                    &mut target,
                    region,
                    &source,
                    stride,
                    Gray8Conversion::Grayscale,
                ),
                Err(expected_error)
            );
            assert_eq!(target, before);
        }

        let larger_display = DisplayExtent::try_new(4, 3).unwrap();
        let foreign_region = RefreshRegion::full(larger_display);
        let source = vec![Rgb8Pixel::default(); 12];
        let mut target = vec![0xaa; 24];
        let before = target.clone();
        assert_eq!(
            layout().write_rgb8_region(
                &mut target,
                foreign_region,
                &source,
                non_zero(4),
                Gray8Conversion::Grayscale,
            ),
            Err(FramebufferWriteError::RegionOutsideLayout)
        );
        assert_eq!(target, before);
    }
}
