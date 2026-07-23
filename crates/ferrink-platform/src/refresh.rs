//! Toolkit-neutral display refresh requests and failures.

use std::num::{NonZeroU16, NonZeroU32};

use crate::{CoordinateAxis, DisplayExtent, DisplayRegion};

/// Whether an E Ink update uses the interactive or cleaning path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshMode {
    /// Update only the requested region without requiring a cleaning waveform.
    Partial,
    /// Use the backend's reviewed full/clean update policy for the region.
    Full,
}

/// Whether a caller needs positive completion evidence after submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshCompletionPolicy {
    /// Return after the backend accepts the update request.
    DoNotWait,
    /// Wait no longer than the supplied duration for the update marker.
    Wait {
        /// Strict upper bound for the completion operation.
        timeout_millis: NonZeroU32,
    },
}

/// A visible display region validated against one exact display extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshRegion {
    x: u32,
    y: u32,
    width: NonZeroU32,
    height: NonZeroU32,
}

impl RefreshRegion {
    /// Validates a non-empty region inside `visible`.
    ///
    /// # Errors
    ///
    /// Returns [`RefreshRequestError`] when either region dimension is zero,
    /// coordinate arithmetic overflows, or the region leaves the display.
    pub fn try_new(
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        visible: DisplayExtent,
    ) -> Result<Self, RefreshRequestError> {
        let width = NonZeroU32::new(width).ok_or(RefreshRequestError::ZeroRegionExtent {
            axis: CoordinateAxis::X,
        })?;
        let height = NonZeroU32::new(height).ok_or(RefreshRequestError::ZeroRegionExtent {
            axis: CoordinateAxis::Y,
        })?;
        let right = x
            .checked_add(width.get())
            .ok_or(RefreshRequestError::RegionOverflow)?;
        let bottom = y
            .checked_add(height.get())
            .ok_or(RefreshRequestError::RegionOverflow)?;
        if right > visible.width() || bottom > visible.height() {
            return Err(RefreshRequestError::RegionOutOfBounds { display: visible });
        }
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    /// Intersects an unsigned dirty rectangle with `visible`.
    ///
    /// Returns `None` for an empty rectangle or one completely outside the
    /// display. Endpoint overflow is clipped to the display edge.
    #[must_use]
    pub fn clipped(
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        visible: DisplayExtent,
    ) -> Option<Self> {
        if width == 0 || height == 0 || x >= visible.width() || y >= visible.height() {
            return None;
        }
        let right = x.saturating_add(width).min(visible.width());
        let bottom = y.saturating_add(height).min(visible.height());
        Self::try_new(x, y, right - x, bottom - y, visible).ok()
    }

    /// Returns a region covering the complete visible display.
    #[must_use]
    pub const fn full(visible: DisplayExtent) -> Self {
        Self {
            x: 0,
            y: 0,
            width: visible.non_zero_width(),
            height: visible.non_zero_height(),
        }
    }

    /// Returns the horizontal origin in visible pixels.
    #[must_use]
    pub const fn x(self) -> u32 {
        self.x
    }

    /// Returns the vertical origin in visible pixels.
    #[must_use]
    pub const fn y(self) -> u32 {
        self.y
    }

    /// Returns the non-zero width in pixels.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width.get()
    }

    /// Returns the non-zero height in pixels.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height.get()
    }

    /// Returns the region area without overflowing a 64-bit count.
    #[must_use]
    pub fn pixel_count(self) -> u64 {
        u64::from(self.width.get()) * u64::from(self.height.get())
    }
}

impl From<RefreshRegion> for DisplayRegion {
    fn from(region: RefreshRegion) -> Self {
        Self {
            x: region.x,
            y: region.y,
            width: region.width,
            height: region.height,
        }
    }
}

/// A toolkit-independent display update request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshRequest {
    region: RefreshRegion,
    mode: RefreshMode,
    completion: RefreshCompletionPolicy,
}

impl RefreshRequest {
    /// Creates a request from an already validated visible region.
    #[must_use]
    pub const fn new(
        region: RefreshRegion,
        mode: RefreshMode,
        completion: RefreshCompletionPolicy,
    ) -> Self {
        Self {
            region,
            mode,
            completion,
        }
    }

    /// Returns the validated visible region.
    #[must_use]
    pub const fn region(self) -> RefreshRegion {
        self.region
    }

    /// Returns the requested update mode.
    #[must_use]
    pub const fn mode(self) -> RefreshMode {
        self.mode
    }

    /// Returns the requested completion policy.
    #[must_use]
    pub const fn completion(self) -> RefreshCompletionPolicy {
        self.completion
    }
}

/// Reviewed refresh operations exposed by a concrete display backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshCapabilities {
    partial: bool,
    full: bool,
    maximum_completion_wait_millis: Option<NonZeroU32>,
}

impl RefreshCapabilities {
    /// Constructs a non-empty refresh capability set.
    ///
    /// A missing maximum completion duration means that completion waiting has
    /// not been reviewed and must fail closed.
    ///
    /// # Errors
    ///
    /// Returns [`RefreshCapabilityError::NoRefreshModes`] if neither partial
    /// nor full updates are available.
    pub const fn try_new(
        partial: bool,
        full: bool,
        maximum_completion_wait_millis: Option<NonZeroU32>,
    ) -> Result<Self, RefreshCapabilityError> {
        if !partial && !full {
            return Err(RefreshCapabilityError::NoRefreshModes);
        }
        Ok(Self {
            partial,
            full,
            maximum_completion_wait_millis,
        })
    }

    /// Returns whether `mode` has been reviewed for the backend.
    #[must_use]
    pub const fn supports(self, mode: RefreshMode) -> bool {
        match mode {
            RefreshMode::Partial => self.partial,
            RefreshMode::Full => self.full,
        }
    }

    /// Returns the reviewed completion-wait bound, if any.
    #[must_use]
    pub const fn maximum_completion_wait_millis(self) -> Option<NonZeroU32> {
        self.maximum_completion_wait_millis
    }

    /// Checks a request against the reviewed mode and completion capabilities.
    ///
    /// # Errors
    ///
    /// Returns [`RefreshRequestError`] when the requested mode or completion
    /// behavior has not been reviewed.
    pub fn validate(self, request: RefreshRequest) -> Result<(), RefreshRequestError> {
        if !self.supports(request.mode) {
            return Err(RefreshRequestError::UnsupportedMode { mode: request.mode });
        }
        let RefreshCompletionPolicy::Wait { timeout_millis } = request.completion else {
            return Ok(());
        };
        let Some(maximum) = self.maximum_completion_wait_millis else {
            return Err(RefreshRequestError::CompletionWaitUnsupported);
        };
        if timeout_millis > maximum {
            return Err(RefreshRequestError::CompletionWaitAboveMaximum {
                observed_millis: timeout_millis,
                maximum_millis: maximum,
            });
        }
        Ok(())
    }
}

/// A non-zero update marker that cannot be confused with another integer ID.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UpdateMarker(NonZeroU32);

impl UpdateMarker {
    /// Returns the integer passed to the platform update ABI.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// Allocates sequential, non-zero update markers for one backend instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateMarkerSequence {
    next: NonZeroU32,
}

impl Default for UpdateMarkerSequence {
    fn default() -> Self {
        Self {
            next: NonZeroU32::MIN,
        }
    }
}

impl UpdateMarkerSequence {
    /// Starts a sequence at an explicitly restored non-zero value.
    #[must_use]
    pub const fn starting_at(next: NonZeroU32) -> Self {
        Self { next }
    }

    /// Allocates the current marker and advances, wrapping `u32::MAX` to one.
    #[must_use]
    pub fn allocate(&mut self) -> UpdateMarker {
        let marker = UpdateMarker(self.next);
        self.next = match self.next.get().checked_add(1) {
            Some(next) => match NonZeroU32::new(next) {
                Some(next) => next,
                None => NonZeroU32::MIN,
            },
            None => NonZeroU32::MIN,
        };
        marker
    }
}

/// Invalid reviewed refresh capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RefreshCapabilityError {
    /// A display backend exposed neither partial nor full updates.
    NoRefreshModes,
}

impl std::fmt::Display for RefreshCapabilityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoRefreshModes => formatter.write_str("at least one refresh mode is required"),
        }
    }
}

impl std::error::Error for RefreshCapabilityError {}

/// A refresh request that cannot be safely submitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RefreshRequestError {
    /// One region dimension was zero.
    ZeroRegionExtent { axis: CoordinateAxis },
    /// Region endpoint arithmetic overflowed.
    RegionOverflow,
    /// The region exceeded the visible display.
    RegionOutOfBounds { display: DisplayExtent },
    /// The requested update mode was not reviewed for this backend.
    UnsupportedMode { mode: RefreshMode },
    /// Completion waiting was not reviewed for this backend.
    CompletionWaitUnsupported,
    /// The requested completion wait exceeded the reviewed bound.
    CompletionWaitAboveMaximum {
        observed_millis: NonZeroU32,
        maximum_millis: NonZeroU32,
    },
}

impl std::fmt::Display for RefreshRequestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroRegionExtent { axis } => {
                write!(formatter, "refresh region {axis} extent must be non-zero")
            }
            Self::RegionOverflow => formatter.write_str("refresh region arithmetic overflow"),
            Self::RegionOutOfBounds { display } => write!(
                formatter,
                "refresh region exceeds {}x{} display",
                display.width(),
                display.height()
            ),
            Self::UnsupportedMode { mode } => {
                write!(formatter, "refresh mode {mode:?} is not supported")
            }
            Self::CompletionWaitUnsupported => {
                formatter.write_str("refresh completion wait is not supported")
            }
            Self::CompletionWaitAboveMaximum {
                observed_millis,
                maximum_millis,
            } => write!(
                formatter,
                "refresh completion wait {} ms exceeds {} ms maximum",
                observed_millis.get(),
                maximum_millis.get()
            ),
        }
    }
}

impl std::error::Error for RefreshRequestError {}

/// A structured failure from a future concrete display backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RefreshError {
    /// The request failed validation before platform I/O.
    InvalidRequest(RefreshRequestError),
    /// The reviewed framebuffer could not be opened or acquired.
    DeviceUnavailable { errno: Option<NonZeroU16> },
    /// The platform rejected an update submission.
    SubmissionFailed {
        marker: UpdateMarker,
        errno: Option<NonZeroU16>,
    },
    /// Waiting for the submitted marker failed.
    CompletionFailed {
        marker: UpdateMarker,
        errno: Option<NonZeroU16>,
    },
    /// The submitted marker did not complete within its strict bound.
    CompletionTimedOut {
        marker: UpdateMarker,
        timeout_millis: NonZeroU32,
    },
}

impl std::fmt::Display for RefreshError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(error) => write!(formatter, "invalid refresh request: {error}"),
            Self::DeviceUnavailable { errno } => {
                write!(
                    formatter,
                    "display device is unavailable{}",
                    ErrnoSuffix(*errno)
                )
            }
            Self::SubmissionFailed { marker, errno } => write!(
                formatter,
                "refresh marker {} submission failed{}",
                marker.get(),
                ErrnoSuffix(*errno)
            ),
            Self::CompletionFailed { marker, errno } => write!(
                formatter,
                "refresh marker {} completion failed{}",
                marker.get(),
                ErrnoSuffix(*errno)
            ),
            Self::CompletionTimedOut {
                marker,
                timeout_millis,
            } => write!(
                formatter,
                "refresh marker {} did not complete within {} ms",
                marker.get(),
                timeout_millis.get()
            ),
        }
    }
}

impl std::error::Error for RefreshError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidRequest(error) => Some(error),
            Self::DeviceUnavailable { .. }
            | Self::SubmissionFailed { .. }
            | Self::CompletionFailed { .. }
            | Self::CompletionTimedOut { .. } => None,
        }
    }
}

impl From<RefreshRequestError> for RefreshError {
    fn from(error: RefreshRequestError) -> Self {
        Self::InvalidRequest(error)
    }
}

struct ErrnoSuffix(Option<NonZeroU16>);

impl std::fmt::Display for ErrnoSuffix {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(errno) = self.0 {
            write!(formatter, " with errno {}", errno.get())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visible() -> DisplayExtent {
        DisplayExtent::try_new(1264, 1680).unwrap()
    }

    fn non_zero(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).unwrap()
    }

    #[test]
    fn refresh_regions_are_non_empty_and_inside_the_visible_display() {
        let region = RefreshRegion::try_new(1200, 1600, 64, 80, visible()).unwrap();
        assert_eq!(region.x(), 1200);
        assert_eq!(region.y(), 1600);
        assert_eq!(region.width(), 64);
        assert_eq!(region.height(), 80);
        assert_eq!(region.pixel_count(), 5_120);

        assert!(matches!(
            RefreshRegion::try_new(0, 0, 0, 1, visible()),
            Err(RefreshRequestError::ZeroRegionExtent {
                axis: CoordinateAxis::X
            })
        ));
        assert!(matches!(
            RefreshRegion::try_new(0, 0, 1, 0, visible()),
            Err(RefreshRequestError::ZeroRegionExtent {
                axis: CoordinateAxis::Y
            })
        ));
        assert!(matches!(
            RefreshRegion::try_new(u32::MAX, 0, 1, 1, visible()),
            Err(RefreshRequestError::RegionOverflow)
        ));
        assert!(matches!(
            RefreshRegion::try_new(1263, 1679, 2, 1, visible()),
            Err(RefreshRequestError::RegionOutOfBounds { .. })
        ));
    }

    #[test]
    fn full_region_exactly_covers_the_visible_display() {
        let region = RefreshRegion::full(visible());
        assert_eq!(region.x(), 0);
        assert_eq!(region.y(), 0);
        assert_eq!(region.width(), 1264);
        assert_eq!(region.height(), 1680);
        assert_eq!(region.pixel_count(), 2_123_520);
        assert_eq!(DisplayRegion::from(region).width.get(), 1264);
    }

    #[test]
    fn dirty_regions_clip_explicitly_without_wrapping() {
        assert_eq!(
            RefreshRegion::clipped(1200, 1600, 100, 100, visible()),
            Some(RefreshRegion::try_new(1200, 1600, 64, 80, visible()).unwrap())
        );
        assert_eq!(
            RefreshRegion::clipped(1200, 1600, u32::MAX, u32::MAX, visible()),
            Some(RefreshRegion::try_new(1200, 1600, 64, 80, visible()).unwrap())
        );
        assert_eq!(RefreshRegion::clipped(1264, 0, 1, 1, visible()), None);
        assert_eq!(RefreshRegion::clipped(0, 0, 0, 1, visible()), None);
    }

    #[test]
    fn refresh_capabilities_reject_unreviewed_modes_and_waits() {
        assert_eq!(
            RefreshCapabilities::try_new(false, false, None),
            Err(RefreshCapabilityError::NoRefreshModes)
        );

        let partial_only = RefreshCapabilities::try_new(true, false, None).unwrap();
        let region = RefreshRegion::try_new(0, 0, 64, 64, visible()).unwrap();
        let full = RefreshRequest::new(
            region,
            RefreshMode::Full,
            RefreshCompletionPolicy::DoNotWait,
        );
        assert_eq!(
            partial_only.validate(full),
            Err(RefreshRequestError::UnsupportedMode {
                mode: RefreshMode::Full
            })
        );

        let wait = RefreshRequest::new(
            region,
            RefreshMode::Partial,
            RefreshCompletionPolicy::Wait {
                timeout_millis: non_zero(1),
            },
        );
        assert_eq!(
            partial_only.validate(wait),
            Err(RefreshRequestError::CompletionWaitUnsupported)
        );
    }

    #[test]
    fn refresh_capabilities_enforce_the_completion_bound() {
        let capabilities = RefreshCapabilities::try_new(true, true, Some(non_zero(5_000))).unwrap();
        let region = RefreshRegion::try_new(0, 0, 64, 64, visible()).unwrap();
        let accepted = RefreshRequest::new(
            region,
            RefreshMode::Partial,
            RefreshCompletionPolicy::Wait {
                timeout_millis: non_zero(5_000),
            },
        );
        capabilities.validate(accepted).unwrap();

        let excessive = RefreshRequest::new(
            region,
            RefreshMode::Partial,
            RefreshCompletionPolicy::Wait {
                timeout_millis: non_zero(5_001),
            },
        );
        assert_eq!(
            capabilities.validate(excessive),
            Err(RefreshRequestError::CompletionWaitAboveMaximum {
                observed_millis: non_zero(5_001),
                maximum_millis: non_zero(5_000),
            })
        );
    }

    #[test]
    fn update_markers_start_at_one_and_wrap_without_emitting_zero() {
        let mut markers = UpdateMarkerSequence::default();
        assert_eq!(markers.allocate().get(), 1);
        assert_eq!(markers.allocate().get(), 2);

        let mut wrapping = UpdateMarkerSequence::starting_at(NonZeroU32::MAX);
        assert_eq!(wrapping.allocate().get(), u32::MAX);
        assert_eq!(wrapping.allocate().get(), 1);
        assert_eq!(wrapping.allocate().get(), 2);
    }

    #[test]
    fn refresh_failures_preserve_stage_marker_and_errno() {
        let marker = UpdateMarkerSequence::default().allocate();
        let errno = NonZeroU16::new(22);
        let failure = RefreshError::SubmissionFailed { marker, errno };

        assert_eq!(
            failure.to_string(),
            "refresh marker 1 submission failed with errno 22"
        );
        assert!(std::error::Error::source(&failure).is_none());

        let request = RefreshRequestError::CompletionWaitUnsupported;
        let failure = RefreshError::from(request);
        assert!(std::error::Error::source(&failure).is_some());
    }
}
