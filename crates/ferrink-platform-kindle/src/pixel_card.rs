//! Single-attempt, host-testable KOA3 pixel-write semantics card.

use std::num::{NonZeroI32, NonZeroU32};
use std::time::Duration;

use ferrink_platform::{
    DisplayUpdateAbiKind, Gray8FramebufferLayout, RefreshCapabilities, RefreshCompletionPolicy,
    RefreshError, RefreshMode, RefreshRegion, RefreshRequest, RefreshRequestError,
    ResolvedRuntimeDevice, StockRepaintMechanism, UpdateMarker, UpdateMarkerSequence,
};

use crate::DisplayTarget;

/// Single-use identifier for the first KOA3 mapped-pixel card.
pub const KOA3_PIXEL_CARD_ID: &str = "koa3-pixel-write-v1";

const REGION_X: u32 = 600;
const REGION_Y: u32 = 808;
const REGION_WIDTH: u32 = 64;
const REGION_HEIGHT: u32 = 64;
const REGION_PIXELS: usize = 4_096;
const OBSERVATION_WINDOW: Duration = Duration::from_secs(3);
const CARD_MARKER: NonZeroU32 = NonZeroU32::new(0x464b_0002).unwrap();

/// Display target that can hold the exact pattern for one bounded observation
/// window before the core restores the original mapped bytes.
pub trait PixelCardTarget: DisplayTarget {
    /// Holds the just-submitted pattern for the exact supplied duration.
    ///
    /// # Errors
    ///
    /// Returns a bounded timing error. The card core restores its region before
    /// propagating the failure.
    fn dwell(&mut self, duration: Duration) -> Result<(), PixelCardIoError>;
}

/// One exact, single-attempt pixel semantics plan derived from KOA3 runtime
/// facts and a previously reviewed stock-return mechanism.
#[derive(Debug, PartialEq, Eq)]
pub struct Koa3PixelCard {
    layout: Gray8FramebufferLayout,
    update_abi: DisplayUpdateAbiKind,
    capabilities: RefreshCapabilities,
    request: RefreshRequest,
    marker: UpdateMarker,
    attempted: bool,
}

impl Koa3PixelCard {
    /// Builds the fixed KOA3 card after every prerequisite capability exists.
    ///
    /// # Errors
    ///
    /// Returns [`PixelCardError`] unless the exact KOA3 profile, Zelda ABI,
    /// recorded framebuffer layout, and reviewed `xrefresh` stock-return action
    /// are present.
    pub fn try_from_runtime(device: &ResolvedRuntimeDevice) -> Result<Self, PixelCardError> {
        if device.profile_id() != "reference-portrait" {
            return Err(PixelCardError::WrongProfile);
        }
        if device.stock_repaint() != Some(StockRepaintMechanism::XrefreshDisplay0) {
            return Err(PixelCardError::StockRepaintUnavailable);
        }
        let layout = device.framebuffer_layout();
        if layout.visible().width() != 1264
            || layout.visible().height() != 1680
            || layout.virtual_extent().width() != 1280
            || layout.virtual_extent().height() != 3584
            || layout.x_offset() != 0
            || layout.y_offset() != 0
            || layout.line_length() != 1280
            || layout.memory_length() != 4_587_520
        {
            return Err(PixelCardError::LayoutMismatch);
        }
        let refresh = device.refresh().ok_or(PixelCardError::RefreshUnavailable)?;
        if refresh.update_abi() != DisplayUpdateAbiKind::Zelda88 {
            return Err(PixelCardError::WrongRefreshAbi);
        }
        let region = RefreshRegion::try_new(
            REGION_X,
            REGION_Y,
            REGION_WIDTH,
            REGION_HEIGHT,
            layout.visible(),
        )
        .map_err(PixelCardError::InvalidRequest)?;
        let request = RefreshRequest::new(
            region,
            RefreshMode::Partial,
            RefreshCompletionPolicy::DoNotWait,
        );
        let capabilities = refresh.capabilities();
        capabilities
            .validate(request)
            .map_err(PixelCardError::InvalidRequest)?;
        let marker = UpdateMarkerSequence::starting_at(CARD_MARKER).allocate();
        Ok(Self {
            layout,
            update_abi: refresh.update_abi(),
            capabilities,
            request,
            marker,
            attempted: false,
        })
    }

    /// Returns the exact visible update region.
    #[must_use]
    pub const fn region(&self) -> RefreshRegion {
        self.request.region()
    }

    /// Returns the single card-specific update marker.
    #[must_use]
    pub const fn marker(&self) -> UpdateMarker {
        self.marker
    }

    /// Returns the fixed observation window after the update submission.
    #[must_use]
    pub const fn observation_window(&self) -> Duration {
        OBSERVATION_WINDOW
    }

    /// Writes the exact checker pattern, submits one update, holds it for the
    /// observation window, and restores the original mapped bytes.
    ///
    /// The attempt is consumed before memory access. Every target/source bound
    /// and request capability is checked before the first byte changes. The
    /// original 4,096 bytes are restored after submission failure or dwell
    /// failure as well as after success. No second refresh is submitted.
    ///
    /// # Errors
    ///
    /// Returns [`PixelCardError`] for a repeated attempt, memory drift,
    /// submission failure, dwell failure, or restoration failure.
    pub fn execute(
        &mut self,
        target: &mut impl PixelCardTarget,
    ) -> Result<PixelCardPass, PixelCardError> {
        if self.attempted {
            return Err(PixelCardError::AlreadyAttempted);
        }
        self.attempted = true;
        self.capabilities
            .validate(self.request)
            .map_err(PixelCardError::InvalidRequest)?;

        let original = snapshot_region(
            self.layout,
            self.request.region(),
            target.framebuffer_memory(),
        )
        .map_err(PixelCardError::Memory)?;
        write_region(
            self.layout,
            self.request.region(),
            target.framebuffer_memory(),
            &checker_pattern(),
        )
        .map_err(PixelCardError::Memory)?;

        let operation = match target.submit_refresh(self.update_abi, self.request, self.marker) {
            Ok(()) => target
                .dwell(OBSERVATION_WINDOW)
                .map(|()| self.marker)
                .map_err(PixelCardOperationError::Dwell),
            Err(error) => Err(PixelCardOperationError::Refresh(error)),
        };
        let restoration = write_region(
            self.layout,
            self.request.region(),
            target.framebuffer_memory(),
            &original,
        );

        match (operation, restoration) {
            (Ok(marker), Ok(())) => Ok(PixelCardPass {
                marker,
                restored_bytes: REGION_PIXELS,
            }),
            (Err(operation), Ok(())) => Err(PixelCardError::Operation(operation)),
            (Ok(_), Err(restoration)) => Err(PixelCardError::Restoration(restoration)),
            (Err(operation), Err(restoration)) => Err(PixelCardError::OperationAndRestoration {
                operation,
                restoration,
            }),
        }
    }
}

/// Successful one-shot memory/update/restore sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelCardPass {
    marker: UpdateMarker,
    restored_bytes: usize,
}

impl PixelCardPass {
    /// Returns the only marker submitted by the card.
    #[must_use]
    pub const fn marker(self) -> UpdateMarker {
        self.marker
    }

    /// Returns the exact number of mapped bytes restored before return.
    #[must_use]
    pub const fn restored_bytes(self) -> usize {
        self.restored_bytes
    }
}

/// Bounded observation-window failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelCardIoError {
    errno: Option<NonZeroI32>,
}

impl PixelCardIoError {
    /// Constructs a redacted dwell failure.
    #[must_use]
    pub const fn new(errno: Option<NonZeroI32>) -> Self {
        Self { errno }
    }

    /// Returns a positive operating-system error number when available.
    #[must_use]
    pub const fn errno(self) -> Option<NonZeroI32> {
        self.errno
    }
}

impl std::fmt::Display for PixelCardIoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.errno {
            Some(errno) => write!(
                formatter,
                "pixel observation window failed with errno {errno}"
            ),
            None => formatter.write_str("pixel observation window failed"),
        }
    }
}

impl std::error::Error for PixelCardIoError {}

/// Failure after mapped bytes changed but before restoration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PixelCardOperationError {
    /// The one exact Zelda request was rejected.
    Refresh(RefreshError),
    /// The bounded pattern observation window failed.
    Dwell(PixelCardIoError),
}

impl std::fmt::Display for PixelCardOperationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refresh(error) => write!(formatter, "pixel refresh failed: {error}"),
            Self::Dwell(error) => write!(formatter, "pixel dwell failed: {error}"),
        }
    }
}

impl std::error::Error for PixelCardOperationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Refresh(error) => Some(error),
            Self::Dwell(error) => Some(error),
        }
    }
}

/// Invalid or drifting framebuffer-memory access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PixelCardMemoryError {
    /// The exposed memory is shorter than the revalidated framebuffer mapping.
    TargetTooShort { observed: usize, minimum: usize },
    /// Coordinate, stride, or slice arithmetic overflowed.
    ArithmeticOverflow,
    /// The fixed card region no longer fits the revalidated layout.
    RegionOutsideLayout,
}

impl std::fmt::Display for PixelCardMemoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TargetTooShort { observed, minimum } => write!(
                formatter,
                "framebuffer memory has {observed} bytes; {minimum} required"
            ),
            Self::ArithmeticOverflow => {
                formatter.write_str("pixel-card address arithmetic overflow")
            }
            Self::RegionOutsideLayout => {
                formatter.write_str("pixel-card region leaves the framebuffer layout")
            }
        }
    }
}

impl std::error::Error for PixelCardMemoryError {}

/// Failure while constructing or executing the single-use pixel card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PixelCardError {
    /// This card is specific to the recorded KOA3 profile.
    WrongProfile,
    /// The stock-return mechanism has not passed its own live card.
    StockRepaintUnavailable,
    /// No reviewed refresh capability is present.
    RefreshUnavailable,
    /// The exact live layout differs from the card's recorded KOA3 layout.
    LayoutMismatch,
    /// The exact refresh ABI is not Zelda-88.
    WrongRefreshAbi,
    /// The fixed request failed capability validation.
    InvalidRequest(RefreshRequestError),
    /// This in-process card has already attempted memory access.
    AlreadyAttempted,
    /// Snapshot or pattern write failed before submission.
    Memory(PixelCardMemoryError),
    /// Submission or dwell failed, but restoration succeeded.
    Operation(PixelCardOperationError),
    /// The primary operation succeeded but memory restoration failed.
    Restoration(PixelCardMemoryError),
    /// Both the primary operation and restoration failed.
    OperationAndRestoration {
        operation: PixelCardOperationError,
        restoration: PixelCardMemoryError,
    },
}

impl std::fmt::Display for PixelCardError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongProfile => formatter.write_str("pixel card requires the KOA3 profile"),
            Self::StockRepaintUnavailable => {
                formatter.write_str("reviewed KOA3 stock repaint is unavailable")
            }
            Self::RefreshUnavailable => {
                formatter.write_str("reviewed display refresh is unavailable")
            }
            Self::LayoutMismatch => formatter.write_str("KOA3 framebuffer layout changed"),
            Self::WrongRefreshAbi => formatter.write_str("KOA3 Zelda-88 refresh ABI is absent"),
            Self::InvalidRequest(error) => write!(formatter, "invalid pixel request: {error}"),
            Self::AlreadyAttempted => formatter.write_str("pixel card attempt already consumed"),
            Self::Memory(error) => write!(formatter, "pixel memory access failed: {error}"),
            Self::Operation(error) => write!(formatter, "pixel operation failed: {error}"),
            Self::Restoration(error) => write!(formatter, "pixel restoration failed: {error}"),
            Self::OperationAndRestoration {
                operation,
                restoration,
            } => write!(
                formatter,
                "pixel operation failed ({operation}) and restoration failed ({restoration})"
            ),
        }
    }
}

impl std::error::Error for PixelCardError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidRequest(error) => Some(error),
            Self::Memory(error) | Self::Restoration(error) => Some(error),
            Self::Operation(error) => Some(error),
            Self::OperationAndRestoration { operation, .. } => Some(operation),
            Self::WrongProfile
            | Self::StockRepaintUnavailable
            | Self::RefreshUnavailable
            | Self::LayoutMismatch
            | Self::WrongRefreshAbi
            | Self::AlreadyAttempted => None,
        }
    }
}

fn checker_pattern() -> [u8; REGION_PIXELS] {
    let mut pattern = [0_u8; REGION_PIXELS];
    for (index, pixel) in pattern.iter_mut().enumerate() {
        let x = index % usize::try_from(REGION_WIDTH).unwrap_or(usize::MAX);
        let y = index / usize::try_from(REGION_WIDTH).unwrap_or(usize::MAX);
        let border = x < 2 || y < 2 || x >= 62 || y >= 62;
        let white_tile = (x / 8 + y / 8) % 2 == 0;
        *pixel = if border || !white_tile {
            u8::MIN
        } else {
            u8::MAX
        };
    }
    pattern
}

fn snapshot_region(
    layout: Gray8FramebufferLayout,
    region: RefreshRegion,
    memory: &[u8],
) -> Result<[u8; REGION_PIXELS], PixelCardMemoryError> {
    let geometry = validate_memory(layout, region, memory.len())?;
    let mut snapshot = [0_u8; REGION_PIXELS];
    for row in 0..geometry.height {
        let source_start = (geometry.y + row)
            .checked_mul(geometry.stride)
            .and_then(|offset| offset.checked_add(geometry.x))
            .ok_or(PixelCardMemoryError::ArithmeticOverflow)?;
        let destination_start = row
            .checked_mul(geometry.width)
            .ok_or(PixelCardMemoryError::ArithmeticOverflow)?;
        snapshot[destination_start..destination_start + geometry.width]
            .copy_from_slice(&memory[source_start..source_start + geometry.width]);
    }
    Ok(snapshot)
}

fn write_region(
    layout: Gray8FramebufferLayout,
    region: RefreshRegion,
    memory: &mut [u8],
    pixels: &[u8; REGION_PIXELS],
) -> Result<(), PixelCardMemoryError> {
    let geometry = validate_memory(layout, region, memory.len())?;
    for row in 0..geometry.height {
        let destination_start = (geometry.y + row)
            .checked_mul(geometry.stride)
            .and_then(|offset| offset.checked_add(geometry.x))
            .ok_or(PixelCardMemoryError::ArithmeticOverflow)?;
        let source_start = row
            .checked_mul(geometry.width)
            .ok_or(PixelCardMemoryError::ArithmeticOverflow)?;
        memory[destination_start..destination_start + geometry.width]
            .copy_from_slice(&pixels[source_start..source_start + geometry.width]);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct RegionGeometry {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    stride: usize,
}

fn validate_memory(
    layout: Gray8FramebufferLayout,
    region: RefreshRegion,
    memory_length: usize,
) -> Result<RegionGeometry, PixelCardMemoryError> {
    let right = region
        .x()
        .checked_add(region.width())
        .ok_or(PixelCardMemoryError::ArithmeticOverflow)?;
    let bottom = region
        .y()
        .checked_add(region.height())
        .ok_or(PixelCardMemoryError::ArithmeticOverflow)?;
    if right > layout.visible().width()
        || bottom > layout.visible().height()
        || region.pixel_count() != REGION_PIXELS as u64
    {
        return Err(PixelCardMemoryError::RegionOutsideLayout);
    }
    let required = usize::try_from(layout.memory_length())
        .map_err(|_| PixelCardMemoryError::ArithmeticOverflow)?;
    if memory_length < required {
        return Err(PixelCardMemoryError::TargetTooShort {
            observed: memory_length,
            minimum: required,
        });
    }
    let x = layout
        .x_offset()
        .checked_add(region.x())
        .and_then(|value| usize::try_from(value).ok())
        .ok_or(PixelCardMemoryError::ArithmeticOverflow)?;
    let y = layout
        .y_offset()
        .checked_add(region.y())
        .and_then(|value| usize::try_from(value).ok())
        .ok_or(PixelCardMemoryError::ArithmeticOverflow)?;
    let width =
        usize::try_from(region.width()).map_err(|_| PixelCardMemoryError::ArithmeticOverflow)?;
    let height =
        usize::try_from(region.height()).map_err(|_| PixelCardMemoryError::ArithmeticOverflow)?;
    let stride = usize::try_from(layout.line_length())
        .map_err(|_| PixelCardMemoryError::ArithmeticOverflow)?;
    let final_end = y
        .checked_add(height.saturating_sub(1))
        .and_then(|row| row.checked_mul(stride))
        .and_then(|offset| offset.checked_add(x))
        .and_then(|offset| offset.checked_add(width))
        .ok_or(PixelCardMemoryError::ArithmeticOverflow)?;
    if final_end > required {
        return Err(PixelCardMemoryError::RegionOutsideLayout);
    }
    Ok(RegionGeometry {
        x,
        y,
        width,
        height,
        stride,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrink_platform::{DeviceProfile, ProbeReport};
    use std::num::NonZeroU16;

    const KOA3_REPORT: &str =
        include_str!("../../ferrink-platform/tests/fixtures/probe-reference-portrait.json");
    const KOA3_PROFILE: &str = include_str!("../../../device-profiles/reference-portrait.toml");

    #[derive(Debug)]
    struct FakeTarget {
        memory: Vec<u8>,
        original: Vec<u8>,
        submissions: Vec<(DisplayUpdateAbiKind, RefreshRequest, UpdateMarker)>,
        pattern_seen_at_submission: bool,
        dwell_calls: Vec<Duration>,
        fail_submission: bool,
        fail_dwell: bool,
    }

    impl FakeTarget {
        fn new(memory_length: usize) -> Self {
            let memory = vec![0x7f; memory_length];
            Self {
                original: memory.clone(),
                memory,
                submissions: Vec::new(),
                pattern_seen_at_submission: false,
                dwell_calls: Vec::new(),
                fail_submission: false,
                fail_dwell: false,
            }
        }

        fn region(&self) -> [u8; REGION_PIXELS] {
            snapshot_region(runtime().framebuffer_layout(), card_region(), &self.memory).unwrap()
        }
    }

    impl DisplayTarget for FakeTarget {
        fn framebuffer_memory(&mut self) -> &mut [u8] {
            &mut self.memory
        }

        fn submit_refresh(
            &mut self,
            update_abi: DisplayUpdateAbiKind,
            request: RefreshRequest,
            marker: UpdateMarker,
        ) -> Result<(), RefreshError> {
            self.pattern_seen_at_submission = self.region() == checker_pattern();
            self.submissions.push((update_abi, request, marker));
            if self.fail_submission {
                Err(RefreshError::SubmissionFailed {
                    marker,
                    errno: NonZeroU16::new(5),
                })
            } else {
                Ok(())
            }
        }
    }

    impl PixelCardTarget for FakeTarget {
        fn dwell(&mut self, duration: Duration) -> Result<(), PixelCardIoError> {
            self.dwell_calls.push(duration);
            if self.fail_dwell {
                Err(PixelCardIoError::new(NonZeroI32::new(4)))
            } else {
                Ok(())
            }
        }
    }

    fn runtime() -> ResolvedRuntimeDevice {
        let profile = DeviceProfile::from_toml(KOA3_PROFILE).unwrap();
        let report = ProbeReport::from_json(KOA3_REPORT).unwrap();
        ResolvedRuntimeDevice::resolve(&profile, &report).unwrap()
    }

    fn card_region() -> RefreshRegion {
        RefreshRegion::try_new(
            REGION_X,
            REGION_Y,
            REGION_WIDTH,
            REGION_HEIGHT,
            runtime().framebuffer_layout().visible(),
        )
        .unwrap()
    }

    #[test]
    fn exact_checker_is_submitted_once_then_every_byte_is_restored() {
        let mut card = Koa3PixelCard::try_from_runtime(&runtime()).unwrap();
        let mut target = FakeTarget::new(4_587_520);

        let pass = card.execute(&mut target).unwrap();

        assert!(target.pattern_seen_at_submission);
        assert_eq!(target.submissions.len(), 1);
        assert_eq!(target.submissions[0].0, DisplayUpdateAbiKind::Zelda88);
        assert_eq!(target.submissions[0].1.region(), card.region());
        assert_eq!(target.submissions[0].2.get(), 0x464b_0002);
        assert_eq!(target.dwell_calls, [Duration::from_secs(3)]);
        assert_eq!(target.memory, target.original);
        assert_eq!(pass.marker().get(), 0x464b_0002);
        assert_eq!(pass.restored_bytes(), 4_096);
        assert_eq!(
            card.execute(&mut target),
            Err(PixelCardError::AlreadyAttempted)
        );
        assert_eq!(target.submissions.len(), 1);
    }

    #[test]
    fn submission_and_dwell_failures_restore_without_retry() {
        for (fail_submission, expected) in [(true, "refresh"), (false, "dwell")] {
            let mut card = Koa3PixelCard::try_from_runtime(&runtime()).unwrap();
            let mut target = FakeTarget::new(4_587_520);
            target.fail_submission = fail_submission;
            target.fail_dwell = !fail_submission;

            let error = card.execute(&mut target).unwrap_err();

            match (expected, error) {
                ("refresh", PixelCardError::Operation(PixelCardOperationError::Refresh(_))) => {}
                ("dwell", PixelCardError::Operation(PixelCardOperationError::Dwell(_))) => {}
                (_, other) => panic!("unexpected error: {other:?}"),
            }
            assert_eq!(target.memory, target.original);
            assert_eq!(target.submissions.len(), 1);
            assert!(card.execute(&mut target).is_err());
            assert_eq!(target.submissions.len(), 1);
        }
    }

    #[test]
    fn short_mapping_fails_before_a_byte_or_request_changes() {
        let mut card = Koa3PixelCard::try_from_runtime(&runtime()).unwrap();
        let mut target = FakeTarget::new(4_587_519);

        assert!(matches!(
            card.execute(&mut target),
            Err(PixelCardError::Memory(
                PixelCardMemoryError::TargetTooShort { .. }
            ))
        ));
        assert_eq!(target.memory, target.original);
        assert!(target.submissions.is_empty());
    }

    #[test]
    fn card_stays_closed_without_reviewed_stock_repaint() {
        let mut profile = DeviceProfile::from_toml(KOA3_PROFILE).unwrap();
        profile.runtime.as_mut().unwrap().stock_repaint = None;
        let report = ProbeReport::from_json(KOA3_REPORT).unwrap();
        let runtime = ResolvedRuntimeDevice::resolve(&profile, &report).unwrap();

        assert_eq!(
            Koa3PixelCard::try_from_runtime(&runtime),
            Err(PixelCardError::StockRepaintUnavailable)
        );
    }

    #[test]
    fn checker_has_a_black_border_and_both_high_contrast_values() {
        let pattern = checker_pattern();
        assert!(pattern.contains(&u8::MIN));
        assert!(pattern.contains(&u8::MAX));
        assert!(pattern[..64].iter().all(|pixel| *pixel == u8::MIN));
        assert!(
            pattern[63..]
                .iter()
                .step_by(64)
                .all(|pixel| *pixel == u8::MIN)
        );
    }
}
