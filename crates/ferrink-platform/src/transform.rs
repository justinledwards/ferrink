//! Validated display geometry and raw-input coordinate transforms.

use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

/// Identifies a display coordinate axis in validation errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinateAxis {
    /// Horizontal display extent.
    X,
    /// Vertical display extent.
    Y,
}

impl std::fmt::Display for CoordinateAxis {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::X => formatter.write_str("x"),
            Self::Y => formatter.write_str("y"),
        }
    }
}

/// Errors produced while constructing or applying coordinate transforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransformError {
    /// An inclusive raw axis range had no usable span.
    InvalidAxisRange { minimum: i32, maximum: i32 },
    /// A display extent was zero.
    ZeroDisplayExtent { axis: CoordinateAxis },
    /// A Linux framebuffer rotation value was not one of `0..=3`.
    UnsupportedFramebufferRotation { value: u32 },
    /// An intermediate value could not be represented safely.
    ArithmeticOverflow,
}

impl std::fmt::Display for TransformError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAxisRange { minimum, maximum } => {
                write!(
                    formatter,
                    "invalid inclusive axis range {minimum}..={maximum}"
                )
            }
            Self::ZeroDisplayExtent { axis } => {
                write!(formatter, "display {axis} extent must be non-zero")
            }
            Self::UnsupportedFramebufferRotation { value } => {
                write!(formatter, "unsupported framebuffer rotation {value}")
            }
            Self::ArithmeticOverflow => formatter.write_str("coordinate arithmetic overflow"),
        }
    }
}

impl std::error::Error for TransformError {}

/// A non-zero display extent in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayExtent {
    width: NonZeroU32,
    height: NonZeroU32,
}

impl DisplayExtent {
    /// Constructs an extent from already-validated non-zero dimensions.
    #[must_use]
    pub const fn new(width: NonZeroU32, height: NonZeroU32) -> Self {
        Self { width, height }
    }

    /// Validates and constructs an extent from ordinary integers.
    ///
    /// # Errors
    ///
    /// Returns [`TransformError::ZeroDisplayExtent`] if either dimension is
    /// zero.
    pub fn try_new(width: u32, height: u32) -> Result<Self, TransformError> {
        let width = NonZeroU32::new(width).ok_or(TransformError::ZeroDisplayExtent {
            axis: CoordinateAxis::X,
        })?;
        let height = NonZeroU32::new(height).ok_or(TransformError::ZeroDisplayExtent {
            axis: CoordinateAxis::Y,
        })?;
        Ok(Self::new(width, height))
    }

    /// Returns the horizontal pixel count.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width.get()
    }

    /// Returns the vertical pixel count.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height.get()
    }

    pub(crate) const fn non_zero_width(self) -> NonZeroU32 {
        self.width
    }

    pub(crate) const fn non_zero_height(self) -> NonZeroU32 {
        self.height
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAxisRange {
    minimum: i32,
    maximum: i32,
}

/// A validated inclusive range reported by an input axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawAxisRange")]
pub struct AxisRange {
    minimum: i32,
    maximum: i32,
}

impl AxisRange {
    /// Validates an inclusive raw input range.
    ///
    /// # Errors
    ///
    /// Returns [`TransformError::InvalidAxisRange`] unless `minimum` is
    /// strictly less than `maximum`.
    pub const fn try_new(minimum: i32, maximum: i32) -> Result<Self, TransformError> {
        if minimum >= maximum {
            return Err(TransformError::InvalidAxisRange { minimum, maximum });
        }
        Ok(Self { minimum, maximum })
    }

    /// Returns the inclusive minimum.
    #[must_use]
    pub const fn minimum(self) -> i32 {
        self.minimum
    }

    /// Returns the inclusive maximum.
    #[must_use]
    pub const fn maximum(self) -> i32 {
        self.maximum
    }
}

impl TryFrom<RawAxisRange> for AxisRange {
    type Error = TransformError;

    fn try_from(raw: RawAxisRange) -> Result<Self, Self::Error> {
        Self::try_new(raw.minimum, raw.maximum)
    }
}

/// An explicit display rotation in quarter turns.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuarterTurn {
    /// No rotation.
    #[default]
    Upright,
    /// Ninety degrees clockwise.
    Clockwise,
    /// One hundred eighty degrees.
    UpsideDown,
    /// Ninety degrees counter-clockwise.
    CounterClockwise,
}

impl QuarterTurn {
    /// Converts a Linux `FB_ROTATE_*` numeric value into a named rotation.
    ///
    /// # Errors
    ///
    /// Returns [`TransformError::UnsupportedFramebufferRotation`] for values
    /// outside `0..=3`.
    pub const fn try_from_linux_framebuffer(value: u32) -> Result<Self, TransformError> {
        match value {
            0 => Ok(Self::Upright),
            1 => Ok(Self::Clockwise),
            2 => Ok(Self::UpsideDown),
            3 => Ok(Self::CounterClockwise),
            value => Err(TransformError::UnsupportedFramebufferRotation { value }),
        }
    }
}

impl TryFrom<u32> for QuarterTurn {
    type Error = TransformError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::try_from_linux_framebuffer(value)
    }
}

/// A signed sample from two raw input axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawPoint {
    /// Raw X-axis value.
    pub x: i32,
    /// Raw Y-axis value.
    pub y: i32,
}

/// A pixel coordinate guaranteed by [`InputTransform::map`] to be in bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayPoint {
    /// Horizontal pixel coordinate.
    pub x: u32,
    /// Vertical pixel coordinate.
    pub y: u32,
}

/// A validated mapping from two raw axes to display pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputTransform {
    raw_x: AxisRange,
    raw_y: AxisRange,
    display: DisplayExtent,
    swap_xy: bool,
    invert_x: bool,
    invert_y: bool,
    rotation: QuarterTurn,
}

impl InputTransform {
    /// Creates an upright transform with no swap or inversion.
    #[must_use]
    pub const fn new(raw_x: AxisRange, raw_y: AxisRange, display: DisplayExtent) -> Self {
        Self {
            raw_x,
            raw_y,
            display,
            swap_xy: false,
            invert_x: false,
            invert_y: false,
            rotation: QuarterTurn::Upright,
        }
    }

    /// Sets whether raw X and Y are assigned to the opposite display axes.
    #[must_use]
    pub const fn with_swap_xy(mut self, swap_xy: bool) -> Self {
        self.swap_xy = swap_xy;
        self
    }

    /// Sets whether the unrotated display X coordinate is inverted.
    #[must_use]
    pub const fn with_invert_x(mut self, invert_x: bool) -> Self {
        self.invert_x = invert_x;
        self
    }

    /// Sets whether the unrotated display Y coordinate is inverted.
    #[must_use]
    pub const fn with_invert_y(mut self, invert_y: bool) -> Self {
        self.invert_y = invert_y;
        self
    }

    /// Sets the final display rotation.
    #[must_use]
    pub const fn with_rotation(mut self, rotation: QuarterTurn) -> Self {
        self.rotation = rotation;
        self
    }

    /// Returns the display extent after rotation.
    #[must_use]
    pub const fn output_extent(self) -> DisplayExtent {
        match self.rotation {
            QuarterTurn::Upright | QuarterTurn::UpsideDown => self.display,
            QuarterTurn::Clockwise | QuarterTurn::CounterClockwise => {
                DisplayExtent::new(self.display.height, self.display.width)
            }
        }
    }

    /// Maps a raw sample into a pixel coordinate inside [`Self::output_extent`].
    ///
    /// Values outside the declared raw ranges are clamped. Swap and inversion
    /// are applied before the final quarter-turn.
    ///
    /// # Errors
    ///
    /// Returns [`TransformError::ArithmeticOverflow`] if an intermediate value
    /// cannot be represented, rather than silently wrapping.
    pub fn map(self, raw: RawPoint) -> Result<DisplayPoint, TransformError> {
        let (mut x, mut y) = if self.swap_xy {
            (
                normalize(raw.y, self.raw_y, self.display.width)?,
                normalize(raw.x, self.raw_x, self.display.height)?,
            )
        } else {
            (
                normalize(raw.x, self.raw_x, self.display.width)?,
                normalize(raw.y, self.raw_y, self.display.height)?,
            )
        };

        if self.invert_x {
            x = self.display.width() - 1 - x;
        }
        if self.invert_y {
            y = self.display.height() - 1 - y;
        }

        Ok(match self.rotation {
            QuarterTurn::Upright => DisplayPoint { x, y },
            QuarterTurn::Clockwise => DisplayPoint {
                x: self.display.height() - 1 - y,
                y: x,
            },
            QuarterTurn::UpsideDown => DisplayPoint {
                x: self.display.width() - 1 - x,
                y: self.display.height() - 1 - y,
            },
            QuarterTurn::CounterClockwise => DisplayPoint {
                x: y,
                y: self.display.width() - 1 - x,
            },
        })
    }
}

fn normalize(value: i32, range: AxisRange, extent: NonZeroU32) -> Result<u32, TransformError> {
    let clamped = value.clamp(range.minimum, range.maximum);
    let offset = i64::from(clamped) - i64::from(range.minimum);
    let span = i64::from(range.maximum) - i64::from(range.minimum);
    let offset = u128::from(u64::try_from(offset).map_err(|_| TransformError::ArithmeticOverflow)?);
    let span = u128::from(u64::try_from(span).map_err(|_| TransformError::ArithmeticOverflow)?);
    let last_pixel = extent.get() - 1;
    let scaled = offset
        .checked_mul(u128::from(last_pixel))
        .ok_or(TransformError::ArithmeticOverflow)?
        / span;
    u32::try_from(scaled).map_err(|_| TransformError::ArithmeticOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transform(width: u32, height: u32) -> InputTransform {
        InputTransform::new(
            AxisRange::try_new(0, 2).unwrap(),
            AxisRange::try_new(0, 1).unwrap(),
            DisplayExtent::try_new(width, height).unwrap(),
        )
    }

    #[test]
    fn axis_ranges_and_display_extents_reject_empty_domains() {
        assert!(matches!(
            AxisRange::try_new(4, 4),
            Err(TransformError::InvalidAxisRange { .. })
        ));
        assert!(matches!(
            AxisRange::try_new(5, 4),
            Err(TransformError::InvalidAxisRange { .. })
        ));
        assert!(matches!(
            DisplayExtent::try_new(0, 10),
            Err(TransformError::ZeroDisplayExtent {
                axis: CoordinateAxis::X
            })
        ));
        assert!(matches!(
            DisplayExtent::try_new(10, 0),
            Err(TransformError::ZeroDisplayExtent {
                axis: CoordinateAxis::Y
            })
        ));
    }

    #[test]
    fn normalization_maps_endpoints_and_clamps_out_of_range_values() {
        let transform = InputTransform::new(
            AxisRange::try_new(-10, 10).unwrap(),
            AxisRange::try_new(100, 200).unwrap(),
            DisplayExtent::try_new(101, 51).unwrap(),
        );

        assert_eq!(
            transform.map(RawPoint { x: -20, y: 50 }).unwrap(),
            DisplayPoint { x: 0, y: 0 }
        );
        assert_eq!(
            transform.map(RawPoint { x: 0, y: 150 }).unwrap(),
            DisplayPoint { x: 50, y: 25 }
        );
        assert_eq!(
            transform.map(RawPoint { x: 20, y: 250 }).unwrap(),
            DisplayPoint { x: 100, y: 50 }
        );
    }

    #[test]
    fn swap_and_inversion_are_applied_before_rotation() {
        let transform = transform(3, 2)
            .with_swap_xy(true)
            .with_invert_x(true)
            .with_invert_y(true);

        assert_eq!(
            transform.map(RawPoint { x: 0, y: 0 }).unwrap(),
            DisplayPoint { x: 2, y: 1 }
        );
        assert_eq!(
            transform.map(RawPoint { x: 2, y: 1 }).unwrap(),
            DisplayPoint { x: 0, y: 0 }
        );
    }

    #[test]
    fn quarter_turns_keep_points_inside_the_rotated_extent() {
        let raw = RawPoint { x: 0, y: 0 };
        let upright = transform(3, 2);
        assert_eq!(upright.map(raw).unwrap(), DisplayPoint { x: 0, y: 0 });
        assert_eq!(
            upright.output_extent(),
            DisplayExtent::try_new(3, 2).unwrap()
        );

        let clockwise = upright.with_rotation(QuarterTurn::Clockwise);
        assert_eq!(clockwise.map(raw).unwrap(), DisplayPoint { x: 1, y: 0 });
        assert_eq!(
            clockwise.output_extent(),
            DisplayExtent::try_new(2, 3).unwrap()
        );

        let upside_down = upright.with_rotation(QuarterTurn::UpsideDown);
        assert_eq!(upside_down.map(raw).unwrap(), DisplayPoint { x: 2, y: 1 });

        let counter_clockwise = upright.with_rotation(QuarterTurn::CounterClockwise);
        assert_eq!(
            counter_clockwise.map(raw).unwrap(),
            DisplayPoint { x: 0, y: 2 }
        );
        assert_eq!(
            counter_clockwise.output_extent(),
            DisplayExtent::try_new(2, 3).unwrap()
        );
    }

    #[test]
    fn every_raw_corner_maps_to_the_expected_corner_for_all_rotations() {
        let base = transform(3, 2);
        let raw_corners = [
            RawPoint { x: 0, y: 0 },
            RawPoint { x: 2, y: 0 },
            RawPoint { x: 0, y: 1 },
            RawPoint { x: 2, y: 1 },
        ];
        let cases = [
            (
                QuarterTurn::Upright,
                [
                    DisplayPoint { x: 0, y: 0 },
                    DisplayPoint { x: 2, y: 0 },
                    DisplayPoint { x: 0, y: 1 },
                    DisplayPoint { x: 2, y: 1 },
                ],
            ),
            (
                QuarterTurn::Clockwise,
                [
                    DisplayPoint { x: 1, y: 0 },
                    DisplayPoint { x: 1, y: 2 },
                    DisplayPoint { x: 0, y: 0 },
                    DisplayPoint { x: 0, y: 2 },
                ],
            ),
            (
                QuarterTurn::UpsideDown,
                [
                    DisplayPoint { x: 2, y: 1 },
                    DisplayPoint { x: 0, y: 1 },
                    DisplayPoint { x: 2, y: 0 },
                    DisplayPoint { x: 0, y: 0 },
                ],
            ),
            (
                QuarterTurn::CounterClockwise,
                [
                    DisplayPoint { x: 0, y: 2 },
                    DisplayPoint { x: 0, y: 0 },
                    DisplayPoint { x: 1, y: 2 },
                    DisplayPoint { x: 1, y: 0 },
                ],
            ),
        ];

        for (rotation, expected) in cases {
            let transform = base.with_rotation(rotation);
            for (raw, expected) in raw_corners.into_iter().zip(expected) {
                assert_eq!(transform.map(raw).unwrap(), expected);
            }
        }
    }

    #[test]
    fn swap_and_mirror_calibration_cases_have_distinct_corner_results() {
        let upper_right = RawPoint { x: 2, y: 0 };
        let base = transform(3, 2);
        let cases = [
            (base, DisplayPoint { x: 2, y: 0 }),
            (base.with_swap_xy(true), DisplayPoint { x: 0, y: 1 }),
            (base.with_invert_x(true), DisplayPoint { x: 0, y: 0 }),
            (base.with_invert_y(true), DisplayPoint { x: 2, y: 1 }),
            (
                base.with_swap_xy(true).with_invert_x(true),
                DisplayPoint { x: 2, y: 1 },
            ),
            (
                base.with_swap_xy(true).with_invert_y(true),
                DisplayPoint { x: 0, y: 0 },
            ),
        ];

        for (transform, expected) in cases {
            assert_eq!(transform.map(upper_right).unwrap(), expected);
        }
    }

    #[test]
    fn framebuffer_rotation_conversion_is_checked() {
        assert_eq!(QuarterTurn::try_from(0).unwrap(), QuarterTurn::Upright);
        assert_eq!(
            QuarterTurn::try_from(3).unwrap(),
            QuarterTurn::CounterClockwise
        );
        assert!(matches!(
            QuarterTurn::try_from(4),
            Err(TransformError::UnsupportedFramebufferRotation { value: 4 })
        ));
    }

    #[test]
    fn deserialization_cannot_bypass_axis_validation() {
        let valid: AxisRange = serde_json::from_str(r#"{"minimum":-1,"maximum":1}"#).unwrap();
        assert_eq!(valid.minimum(), -1);
        assert_eq!(valid.maximum(), 1);
        assert!(serde_json::from_str::<AxisRange>(r#"{"minimum":1,"maximum":1}"#).is_err());
        assert!(serde_json::from_str::<DisplayExtent>(r#"{"width":0,"height":1}"#).is_err());
    }

    #[test]
    fn full_i32_axis_range_maps_to_full_u32_display_extent_without_overflow() {
        let transform = InputTransform::new(
            AxisRange::try_new(i32::MIN, i32::MAX).unwrap(),
            AxisRange::try_new(i32::MIN, i32::MAX).unwrap(),
            DisplayExtent::try_new(u32::MAX, u32::MAX).unwrap(),
        );

        assert_eq!(
            transform
                .map(RawPoint {
                    x: i32::MAX,
                    y: i32::MAX,
                })
                .unwrap(),
            DisplayPoint {
                x: u32::MAX - 1,
                y: u32::MAX - 1,
            }
        );
    }
}
