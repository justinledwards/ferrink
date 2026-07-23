//! Host-testable policy core for a future Kindle foreground adapter.
//!
//! Default builds contain no Linux device implementation. The opt-in
//! `linux-device` feature adds exact-path read-only metadata revalidation and a
//! nonblocking input descriptor seam. It does not map framebuffer memory,
//! submit a display update, request an input grab, or change a stock service.

#![deny(unsafe_op_in_unsafe_fn)]

mod input_grab;
mod input_loop;
#[cfg(all(target_os = "linux", feature = "linux-device"))]
mod linux;
#[cfg(all(
    target_os = "linux",
    target_arch = "arm",
    target_pointer_width = "32",
    feature = "linux-foreground-display"
))]
mod linux_display;
#[cfg(all(
    target_os = "linux",
    target_arch = "arm",
    target_pointer_width = "32",
    feature = "linux-pixel-card"
))]
mod linux_pixel;
#[cfg(all(target_os = "linux", feature = "linux-stock-repaint"))]
mod linux_stock;
mod pixel_card;
mod revalidate;
mod slint_bridge;
mod stock_repaint;
mod touch_card;
#[cfg(any(
    test,
    all(
        target_os = "linux",
        target_arch = "arm",
        target_pointer_width = "32",
        any(feature = "linux-foreground-display", feature = "linux-pixel-card")
    )
))]
mod zelda;

pub use input_grab::*;
pub use input_loop::*;
#[cfg(all(target_os = "linux", feature = "linux-device"))]
pub use linux::*;
#[cfg(all(
    target_os = "linux",
    target_arch = "arm",
    target_pointer_width = "32",
    feature = "linux-foreground-display"
))]
pub use linux_display::*;
#[cfg(all(
    target_os = "linux",
    target_arch = "arm",
    target_pointer_width = "32",
    feature = "linux-pixel-card"
))]
pub use linux_pixel::*;
#[cfg(all(target_os = "linux", feature = "linux-stock-repaint"))]
pub use linux_stock::*;
pub use pixel_card::*;
pub use revalidate::*;
pub use slint_bridge::*;
pub use stock_repaint::*;
pub use touch_card::*;

use std::num::NonZeroU32;

use ferrink_platform::{
    DisplayUpdateAbiKind, FramebufferWriteError, Gray8Conversion, Gray8FramebufferLayout,
    InputEventDecodeError, InputEventDecoder, InputReplayError, RefreshCapabilities, RefreshError,
    RefreshRequest, RefreshRequestError, ResolvedRuntimeDevice, Rgb8Pixel, TouchContactEvent,
    TouchTracker, UpdateMarker, UpdateMarkerSequence,
};

/// Device-memory and refresh-submission seam used by [`L0DisplayCore`].
///
/// A production implementation will own a separately revalidated mapping and
/// exact reviewed ioctl implementation. Tests use an ordinary byte vector.
pub trait DisplayTarget {
    /// Returns the complete validated Gray8 framebuffer memory range.
    fn framebuffer_memory(&mut self) -> &mut [u8];

    /// Submits one already validated refresh request with the resolved ABI.
    ///
    /// # Errors
    ///
    /// Returns a structured [`RefreshError`] carrying the supplied marker when
    /// submission or bounded completion fails.
    fn submit_refresh(
        &mut self,
        update_abi: DisplayUpdateAbiKind,
        request: RefreshRequest,
        marker: UpdateMarker,
    ) -> Result<(), RefreshError>;
}

/// Host-testable display sequencing for a resolved foreground device.
#[derive(Debug, PartialEq, Eq)]
pub struct L0DisplayCore {
    layout: Gray8FramebufferLayout,
    update_abi: DisplayUpdateAbiKind,
    capabilities: RefreshCapabilities,
    markers: UpdateMarkerSequence,
}

impl L0DisplayCore {
    /// Builds display policy from one fail-closed runtime resolution.
    ///
    /// # Errors
    ///
    /// Returns [`L0DisplayError::RefreshUnavailable`] when the exact device has
    /// no reviewed refresh ABI and capabilities.
    pub fn try_from_runtime(device: &ResolvedRuntimeDevice) -> Result<Self, L0DisplayError> {
        let refresh = device.refresh().ok_or(L0DisplayError::RefreshUnavailable)?;
        Ok(Self {
            layout: device.framebuffer_layout(),
            update_abi: refresh.update_abi(),
            capabilities: refresh.capabilities(),
            markers: UpdateMarkerSequence::default(),
        })
    }

    /// Returns the exact validated framebuffer layout.
    #[must_use]
    pub const fn layout(&self) -> Gray8FramebufferLayout {
        self.layout
    }

    /// Returns the exact reviewed update ABI.
    #[must_use]
    pub const fn update_abi(&self) -> DisplayUpdateAbiKind {
        self.update_abi
    }

    /// Converts one dirty RGB region into target memory and submits its update.
    ///
    /// Request capability validation and all source/target bounds checks happen
    /// before the first target byte changes. A marker is allocated only after
    /// the memory write succeeds. A later submission failure can therefore
    /// leave changed mapped bytes, but it is returned immediately and never
    /// retried with another ABI or marker.
    ///
    /// # Errors
    ///
    /// Returns [`L0DisplayError`] for an unreviewed request, invalid source or
    /// target memory, or structured refresh failure.
    pub fn present_rgb8<T: DisplayTarget>(
        &mut self,
        target: &mut T,
        request: RefreshRequest,
        source: &[Rgb8Pixel],
        source_stride_pixels: NonZeroU32,
        conversion: Gray8Conversion,
    ) -> Result<UpdateMarker, L0DisplayError> {
        self.capabilities
            .validate(request)
            .map_err(L0DisplayError::InvalidRequest)?;
        self.layout
            .write_rgb8_region(
                target.framebuffer_memory(),
                request.region(),
                source,
                source_stride_pixels,
                conversion,
            )
            .map_err(L0DisplayError::FramebufferWrite)?;
        let marker = self.markers.allocate();
        target
            .submit_refresh(self.update_abi, request, marker)
            .map_err(L0DisplayError::Refresh)?;
        Ok(marker)
    }
}

/// Failure while preparing or presenting one foreground display update.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum L0DisplayError {
    /// The resolved device has no reviewed refresh mechanism.
    RefreshUnavailable,
    /// The request exceeded reviewed mode or completion authority.
    InvalidRequest(RefreshRequestError),
    /// Source or target memory did not match the validated layout.
    FramebufferWrite(FramebufferWriteError),
    /// The exact reviewed refresh operation failed.
    Refresh(RefreshError),
}

impl std::fmt::Display for L0DisplayError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RefreshUnavailable => {
                formatter.write_str("resolved device has no reviewed refresh mechanism")
            }
            Self::InvalidRequest(error) => write!(formatter, "invalid refresh request: {error}"),
            Self::FramebufferWrite(error) => write!(formatter, "framebuffer write failed: {error}"),
            Self::Refresh(error) => write!(formatter, "refresh failed: {error}"),
        }
    }
}

impl std::error::Error for L0DisplayError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidRequest(error) => Some(error),
            Self::FramebufferWrite(error) => Some(error),
            Self::Refresh(error) => Some(error),
            Self::RefreshUnavailable => None,
        }
    }
}

/// Atomic decoded-input and touch-tracking core for one resolved device.
#[derive(Debug)]
pub struct L0InputCore {
    decoder: InputEventDecoder,
    tracker: TouchTracker,
}

impl L0InputCore {
    /// Creates bounded input state from one fail-closed runtime resolution.
    ///
    /// # Errors
    ///
    /// Returns [`L0InputError::Decode`] when the resolved event ABI or requested
    /// record bound is unsupported.
    pub fn try_from_runtime(
        device: &ResolvedRuntimeDevice,
        maximum_records: NonZeroU32,
    ) -> Result<Self, L0InputError> {
        let decoder = InputEventDecoder::try_new(
            device.input_event_abi(),
            device.input_endianness(),
            maximum_records,
        )
        .map_err(L0InputError::Decode)?;
        Ok(Self {
            decoder,
            tracker: TouchTracker::new(device.input_transform()),
        })
    }

    /// Returns the number of complete raw records in the current budget.
    #[must_use]
    pub const fn decoded_records(&self) -> u32 {
        self.decoder.decoded_records()
    }

    /// Returns the retained suffix length of an incomplete raw record.
    #[must_use]
    pub const fn partial_bytes(&self) -> usize {
        self.decoder.partial_bytes()
    }

    /// Starts a fresh bounded decoder budget while preserving touch state.
    ///
    /// Incomplete raw-record bytes and any active contact tracked by this core
    /// remain unchanged. Bounded evidence capture must not renew its budget.
    pub fn renew_record_budget(&mut self) -> u32 {
        self.decoder.renew_record_budget()
    }

    /// Decodes one arbitrary read chunk and applies its events atomically.
    ///
    /// The decoder and touch tracker are staged together. If any raw record or
    /// touch transition fails, neither state is advanced.
    ///
    /// # Errors
    ///
    /// Returns [`L0InputError`] for raw ABI/timestamp/bound failures or unsafe
    /// and contradictory touch state.
    pub fn push_bytes(&mut self, bytes: &[u8]) -> Result<Vec<TouchContactEvent>, L0InputError> {
        let mut decoder = self.decoder.clone();
        let mut tracker = self.tracker.clone();
        let decoded = decoder.push(bytes).map_err(L0InputError::Decode)?;
        let mut contacts = Vec::new();
        for event in decoded {
            if let Some(contact) = tracker
                .push(event.event_type, event.code, event.value)
                .map_err(L0InputError::Touch)?
            {
                contacts.push(contact);
            }
        }
        self.decoder = decoder;
        self.tracker = tracker;
        Ok(contacts)
    }
}

/// Failure while decoding a bounded foreground input stream.
#[derive(Debug)]
#[non_exhaustive]
pub enum L0InputError {
    /// Raw Linux record decoding failed.
    Decode(InputEventDecodeError),
    /// Protocol-B touch tracking or transformation failed.
    Touch(InputReplayError),
}

impl std::fmt::Display for L0InputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(error) => write!(formatter, "input decode failed: {error}"),
            Self::Touch(error) => write!(formatter, "touch tracking failed: {error}"),
        }
    }
}

impl std::error::Error for L0InputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(error) => Some(error),
            Self::Touch(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrink_platform::{
        DeviceProfile, LogicalTouchPhase, ProbeReport, RefreshCompletionPolicy, RefreshMode,
        RefreshRegion,
    };
    use std::num::NonZeroU16;

    const KOA3_REPORT: &str =
        include_str!("../../ferrink-platform/tests/fixtures/probe-reference-portrait.json");
    const PW1_REPORT: &str =
        include_str!("../../ferrink-platform/tests/fixtures/probe-reference-landscape.json");
    const KOA3_PROFILE: &str = include_str!("../../../device-profiles/reference-portrait.toml");
    const PW1_PROFILE: &str = include_str!("../../../device-profiles/reference-landscape.toml");

    #[derive(Debug)]
    struct FakeDisplay {
        memory: Vec<u8>,
        submitted: Vec<(DisplayUpdateAbiKind, RefreshRequest, UpdateMarker)>,
        fail_submission: bool,
    }

    impl FakeDisplay {
        fn new(memory_length: u32) -> Self {
            Self {
                memory: vec![0x55; usize::try_from(memory_length).unwrap()],
                submitted: Vec::new(),
                fail_submission: false,
            }
        }
    }

    impl DisplayTarget for FakeDisplay {
        fn framebuffer_memory(&mut self) -> &mut [u8] {
            &mut self.memory
        }

        fn submit_refresh(
            &mut self,
            update_abi: DisplayUpdateAbiKind,
            request: RefreshRequest,
            marker: UpdateMarker,
        ) -> Result<(), RefreshError> {
            self.submitted.push((update_abi, request, marker));
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

    fn resolve(profile: &str, report: &str) -> ResolvedRuntimeDevice {
        let profile = DeviceProfile::from_toml(profile).unwrap();
        let report = ProbeReport::from_json(report).unwrap();
        ResolvedRuntimeDevice::resolve(&profile, &report).unwrap()
    }

    #[test]
    fn exact_koa3_display_core_writes_then_submits_only_the_resolved_abi() {
        let device = resolve(KOA3_PROFILE, KOA3_REPORT);
        let mut core = L0DisplayCore::try_from_runtime(&device).unwrap();
        let mut target = FakeDisplay::new(core.layout().memory_length());
        let region = RefreshRegion::try_new(1, 1, 2, 1, core.layout().visible()).unwrap();
        let request = RefreshRequest::new(
            region,
            RefreshMode::Partial,
            RefreshCompletionPolicy::DoNotWait,
        );
        let source = [
            Rgb8Pixel {
                red: 0,
                green: 0,
                blue: 0,
            },
            Rgb8Pixel {
                red: 255,
                green: 255,
                blue: 255,
            },
        ];

        let marker = core
            .present_rgb8(
                &mut target,
                request,
                &source,
                NonZeroU32::new(2).unwrap(),
                Gray8Conversion::Bilevel { threshold: 128 },
            )
            .unwrap();

        assert_eq!(marker.get(), 1);
        assert_eq!(target.memory[1281..1283], [0, 255]);
        assert_eq!(
            target.submitted,
            [(DisplayUpdateAbiKind::Zelda88, request, marker)]
        );
    }

    #[test]
    fn unreviewed_request_fails_before_memory_or_marker_changes() {
        let device = resolve(KOA3_PROFILE, KOA3_REPORT);
        let mut core = L0DisplayCore::try_from_runtime(&device).unwrap();
        let mut target = FakeDisplay::new(core.layout().memory_length());
        let original = target.memory.clone();
        let request = RefreshRequest::new(
            RefreshRegion::try_new(1, 1, 1, 1, core.layout().visible()).unwrap(),
            RefreshMode::Partial,
            RefreshCompletionPolicy::Wait {
                timeout_millis: NonZeroU32::new(1).unwrap(),
            },
        );

        assert!(matches!(
            core.present_rgb8(
                &mut target,
                request,
                &[Rgb8Pixel::default()],
                NonZeroU32::MIN,
                Gray8Conversion::Grayscale,
            ),
            Err(L0DisplayError::InvalidRequest(
                RefreshRequestError::CompletionWaitUnsupported
            ))
        ));
        assert_eq!(target.memory, original);
        assert!(target.submitted.is_empty());

        let request = RefreshRequest::new(
            RefreshRegion::try_new(1, 1, 1, 1, core.layout().visible()).unwrap(),
            RefreshMode::Partial,
            RefreshCompletionPolicy::DoNotWait,
        );
        assert_eq!(
            core.present_rgb8(
                &mut target,
                request,
                &[Rgb8Pixel::default()],
                NonZeroU32::MIN,
                Gray8Conversion::Grayscale,
            )
            .unwrap()
            .get(),
            1
        );
    }

    #[test]
    fn submission_failure_preserves_its_unique_marker_without_retrying() {
        let device = resolve(KOA3_PROFILE, KOA3_REPORT);
        let mut core = L0DisplayCore::try_from_runtime(&device).unwrap();
        let mut target = FakeDisplay::new(core.layout().memory_length());
        target.fail_submission = true;
        let request = RefreshRequest::new(
            RefreshRegion::try_new(1, 1, 1, 1, core.layout().visible()).unwrap(),
            RefreshMode::Partial,
            RefreshCompletionPolicy::DoNotWait,
        );

        assert!(matches!(
            core.present_rgb8(
                &mut target,
                request,
                &[Rgb8Pixel::default()],
                NonZeroU32::MIN,
                Gray8Conversion::Grayscale,
            ),
            Err(L0DisplayError::Refresh(RefreshError::SubmissionFailed {
                marker,
                errno: Some(errno),
            })) if marker.get() == 1 && errno.get() == 5
        ));
        assert_eq!(target.submitted.len(), 1);
        assert_eq!(target.submitted[0].0, DisplayUpdateAbiKind::Zelda88);
        assert_eq!(target.submitted[0].2.get(), 1);
    }

    #[test]
    fn pw1_display_core_stays_closed_without_a_reviewed_refresh_abi() {
        let device = resolve(PW1_PROFILE, PW1_REPORT);

        assert_eq!(
            L0DisplayCore::try_from_runtime(&device),
            Err(L0DisplayError::RefreshUnavailable)
        );
    }

    #[test]
    fn input_chunks_are_atomic_across_decode_and_touch_tracking() {
        let device = resolve(KOA3_PROFILE, KOA3_REPORT);
        let mut core = L0InputCore::try_from_runtime(&device, NonZeroU32::new(8).unwrap()).unwrap();

        let invalid = record32(1, 0, 3, 57, -2);
        assert!(matches!(
            core.push_bytes(&invalid),
            Err(L0InputError::Touch(InputReplayError::InvalidTrackingId {
                value: -2
            }))
        ));
        assert_eq!(core.decoded_records(), 0);
        assert_eq!(core.partial_bytes(), 0);

        let mut valid = Vec::new();
        valid.extend_from_slice(&record32(1, 0, 3, 53, 100));
        valid.extend_from_slice(&record32(1, 1, 3, 54, 200));
        valid.extend_from_slice(&record32(1, 2, 3, 57, 7));
        valid.extend_from_slice(&record32(1, 3, 0, 0, 0));
        assert!(core.push_bytes(&valid[..7]).unwrap().is_empty());
        let contacts = core.push_bytes(&valid[7..]).unwrap();

        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].phase, LogicalTouchPhase::Pressed);
        assert_eq!(contacts[0].point.x, 100);
        assert_eq!(contacts[0].point.y, 200);
        assert_eq!(core.decoded_records(), 4);
        assert_eq!(core.partial_bytes(), 0);
    }

    #[test]
    fn input_budget_renewal_preserves_an_active_touch() {
        let device = resolve(KOA3_PROFILE, KOA3_REPORT);
        let mut core = L0InputCore::try_from_runtime(&device, NonZeroU32::new(4).unwrap()).unwrap();

        let mut press = Vec::new();
        press.extend_from_slice(&record32(1, 0, 3, 53, 100));
        press.extend_from_slice(&record32(1, 1, 3, 54, 200));
        press.extend_from_slice(&record32(1, 2, 3, 57, 7));
        press.extend_from_slice(&record32(1, 3, 0, 0, 0));
        let contacts = core.push_bytes(&press).unwrap();
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].phase, LogicalTouchPhase::Pressed);

        assert_eq!(core.renew_record_budget(), 4);
        assert_eq!(core.decoded_records(), 0);

        let mut release = Vec::new();
        release.extend_from_slice(&record32(1, 4, 3, 57, -1));
        release.extend_from_slice(&record32(1, 5, 0, 0, 0));
        let contacts = core.push_bytes(&release).unwrap();
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].phase, LogicalTouchPhase::Released);
        assert_eq!(contacts[0].point.x, 100);
        assert_eq!(contacts[0].point.y, 200);
    }

    fn record32(
        seconds: i32,
        microseconds: i32,
        event_type: u16,
        code: u16,
        value: i32,
    ) -> [u8; 16] {
        let mut record = [0; 16];
        record[0..4].copy_from_slice(&seconds.to_le_bytes());
        record[4..8].copy_from_slice(&microseconds.to_le_bytes());
        record[8..10].copy_from_slice(&event_type.to_le_bytes());
        record[10..12].copy_from_slice(&code.to_le_bytes());
        record[12..16].copy_from_slice(&value.to_le_bytes());
        record
    }
}
