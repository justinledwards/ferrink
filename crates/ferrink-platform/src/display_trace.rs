//! Strict display characterization result schema without framebuffer I/O.

use std::num::{NonZeroU16, NonZeroU32, NonZeroU64};

use serde::{Deserialize, Serialize};

use crate::{
    CHARACTERIZATION_REDACTION_POLICY, DisplayExtent, DisplayPlanOpenMode, FramebufferMemoryAccess,
    PixelLayout, ProbeWarning, QuarterTurn, RedactionMetadata,
    trace::REQUIRED_REDACTION_CATEGORIES, trace::valid_profile_id,
};

/// Display trace schema implemented by this crate.
pub const DISPLAY_TRACE_SCHEMA_VERSION: u32 = 1;
/// Hard maximum accepted serialized display trace size.
pub const MAX_DISPLAY_TRACE_JSON_BYTES: usize = 65_536;
/// Display trace v1 permits exactly one attempt.
pub const MAX_DISPLAY_TRACE_ATTEMPTS: usize = 1;
/// Display trace v1 limits the requested region to a small test patch.
pub const MAX_DISPLAY_TRACE_REGION_PIXELS: u64 = 4_096;
/// Display trace v1 limits an update-completion wait to five seconds.
pub const MAX_DISPLAY_WAIT_MILLIS: u32 = 5_000;

/// Non-secret framebuffer properties that must match passive evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FramebufferFingerprint {
    /// Reviewed framebuffer path.
    pub device: String,
    /// Kernel driver identifier.
    pub driver_id: String,
    /// Visible pixel extent.
    pub visible: DisplayExtent,
    /// Virtual framebuffer pixel extent.
    pub virtual_extent: DisplayExtent,
    /// Kernel-reported line stride in bytes.
    pub line_length: NonZeroU32,
    /// Kernel-reported framebuffer memory length in bytes.
    pub memory_length: NonZeroU32,
    /// Bits per pixel.
    pub bits_per_pixel: NonZeroU32,
    /// Passive pixel layout classification.
    pub pixel_layout: PixelLayout,
    /// Checked Linux framebuffer rotation.
    pub rotation: QuarterTurn,
}

/// A non-empty display update rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayRegion {
    /// Horizontal origin in visible pixels.
    pub x: u32,
    /// Vertical origin in visible pixels.
    pub y: u32,
    /// Non-zero width in pixels.
    pub width: NonZeroU32,
    /// Non-zero height in pixels.
    pub height: NonZeroU32,
}

/// Reviewed Kindle MXCFB request layout family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayUpdateAbiKind {
    /// Legacy 72-byte request layout.
    Legacy72,
    /// Rex 80-byte request layout.
    Rex80,
    /// Zelda 88-byte request layout.
    Zelda88,
}

impl DisplayUpdateAbiKind {
    /// Returns the reviewed request size for this ABI family.
    #[must_use]
    pub const fn expected_request_size(self) -> u32 {
        match self {
            Self::Legacy72 => 72,
            Self::Rex80 => 80,
            Self::Zelda88 => 88,
        }
    }

    /// Returns the reviewed Kindle ioctl number for this request layout.
    #[must_use]
    pub const fn expected_request_ioctl(self) -> u64 {
        match self {
            Self::Legacy72 => 0x4048_462e,
            Self::Rex80 => 0x4050_462e,
            Self::Zelda88 => 0x4058_462e,
        }
    }
}

/// ABI name and independently recorded request size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayUpdateAbi {
    /// Reviewed layout name.
    pub kind: DisplayUpdateAbiKind,
    /// Size supplied to the ioctl encoder.
    pub request_size: NonZeroU32,
}

/// A bounded completion-marker wait requested before execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayWaitPlan {
    /// Maximum wait duration.
    pub timeout_millis: NonZeroU32,
}

/// Exact display operation reviewed before execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayUpdatePlan {
    /// Framebuffer descriptor access mode.
    pub open_mode: DisplayPlanOpenMode,
    /// Permitted framebuffer pixel-memory access.
    pub memory_access: FramebufferMemoryAccess,
    /// Small visible test region.
    pub region: DisplayRegion,
    /// MXCFB request layout and size.
    pub abi: DisplayUpdateAbi,
    /// Fully encoded ioctl request number.
    pub request_ioctl: NonZeroU64,
    /// Exact waveform mode value from the reviewed profile.
    pub waveform_mode: u32,
    /// Exact partial/full update mode value.
    pub update_mode: u32,
    /// Exact driver temperature value.
    pub temperature: i32,
    /// Exact update flags from the reviewed profile.
    pub flags: u32,
    /// Exact dither mode.
    pub dither_mode: i32,
    /// Exact quantization bit setting.
    pub quant_bit: i32,
    /// Whether every alternate-buffer field was zero.
    pub alternate_buffer_zeroed: bool,
    /// Exact grayscale/color histogram modes.
    pub histogram_modes: [u32; 2],
    /// Exact PXP/EPDC timestamp fields.
    pub timestamps: [u32; 2],
    /// Unique non-zero update marker.
    pub marker: NonZeroU32,
    /// Optional bounded completion wait.
    pub wait: Option<DisplayWaitPlan>,
}

/// Live checks completed before a future active display request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayPreflight {
    /// Synthetic or host-only trace for which no live checks were performed.
    NotPerformed,
    /// Every separately prompted live condition was affirmatively confirmed.
    Confirmed {
        /// An operator was physically at the device.
        operator_present: bool,
        /// The stock UI was readable and responsive.
        stock_ui_healthy: bool,
        /// An independent maintenance connection was healthy.
        maintenance_connection_healthy: bool,
        /// Power was adequate and the device was not entering sleep.
        power_adequate_and_awake: bool,
        /// No update, reboot, shutdown, USB, or storage transition was active.
        no_transition_or_update: bool,
        /// The create-new output target and bounded space were ready.
        output_ready: bool,
        /// A fresh passive report passed exact validation.
        fresh_passive_report: bool,
    },
}

/// Result of submitting one update request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplaySubmissionResult {
    /// The kernel accepted the request.
    Submitted,
    /// Opening the exact framebuffer path failed before submission.
    OpenError { errno: NonZeroU16 },
    /// Submission failed with a positive errno value.
    Error { errno: NonZeroU16 },
}

/// Result of a requested marker-completion wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayCompletionResult {
    /// The marker completed before the timeout.
    Completed,
    /// The bounded wait expired.
    TimedOut,
    /// The wait failed with a positive errno value.
    Error { errno: NonZeroU16 },
}

/// Bounded operator observation of the physical panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayObservation {
    /// No physical observation was recorded.
    NotObserved,
    /// The physical panel did not visibly change.
    Unchanged,
    /// The requested region visibly updated.
    Updated,
    /// The panel visibly flashed.
    Flashed,
    /// The panel showed corruption or an unsafe artifact.
    Corrupted,
    /// The operator could not confidently classify the physical result.
    Uncertain,
}

/// Operator-confirmed stock UI condition after an active attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayStockHealth {
    /// Synthetic or interrupted trace without a post-run check.
    NotChecked,
    /// The stock UI remained readable and responsive.
    Healthy,
    /// The stock UI was visibly or functionally unhealthy.
    Unhealthy,
    /// The operator could not confidently establish stock health.
    Uncertain,
}

/// One attempted update and its exact results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayUpdateAttempt {
    /// Operation reviewed before submission.
    pub plan: DisplayUpdatePlan,
    /// Submission result or errno.
    pub submission: DisplaySubmissionResult,
    /// Completion result when a wait was requested and submission succeeded.
    pub completion: Option<DisplayCompletionResult>,
    /// Bounded physical observation.
    pub observation: DisplayObservation,
}

/// A sanitized, bounded display characterization result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayTrace {
    /// Version of this trace schema.
    pub schema_version: u32,
    /// Mandatory redaction contract.
    pub redaction: RedactionMetadata,
    /// Exact reviewed profile ID.
    pub profile_id: String,
    /// Framebuffer fingerprint from passive evidence.
    pub framebuffer: FramebufferFingerprint,
    /// Live confirmations collected before opening the framebuffer.
    pub preflight: DisplayPreflight,
    /// Attempted operations. Version 1 requires exactly one.
    pub attempts: Vec<DisplayUpdateAttempt>,
    /// Stock UI health after the attempted operation.
    pub post_run_stock_health: DisplayStockHealth,
    /// Redacted bounded warnings.
    #[serde(default)]
    pub warnings: Vec<ProbeWarning>,
}

impl DisplayTrace {
    /// Parses and validates a bounded display trace.
    ///
    /// # Errors
    ///
    /// Returns [`DisplayTraceError`] for oversized or malformed JSON and for
    /// any schema, privacy, fingerprint, request, bound, or result failure.
    pub fn from_json(input: &str) -> Result<Self, DisplayTraceError> {
        if input.len() > MAX_DISPLAY_TRACE_JSON_BYTES {
            return Err(DisplayTraceError::InputTooLarge {
                bytes: input.len(),
                maximum: MAX_DISPLAY_TRACE_JSON_BYTES,
            });
        }
        let trace: Self = serde_json::from_str(input).map_err(DisplayTraceError::Json)?;
        trace.validate().map_err(DisplayTraceError::Validation)?;
        Ok(trace)
    }

    /// Validates all fail-closed display trace invariants.
    ///
    /// # Errors
    ///
    /// Returns every detected [`DisplayTraceValidationError`].
    pub fn validate(&self) -> Result<(), Vec<DisplayTraceValidationError>> {
        let mut errors = Vec::new();
        if self.schema_version != DISPLAY_TRACE_SCHEMA_VERSION {
            errors.push(DisplayTraceValidationError::UnsupportedSchema {
                observed: self.schema_version,
            });
        }
        validate_redaction(&self.redaction, &mut errors);
        if !valid_profile_id(&self.profile_id) {
            errors.push(DisplayTraceValidationError::InvalidProfileId);
        }
        validate_framebuffer(&self.framebuffer, &mut errors);
        if !preflight_is_complete(self.preflight) {
            errors.push(DisplayTraceValidationError::IncompletePreflight);
        }
        if self.attempts.len() != MAX_DISPLAY_TRACE_ATTEMPTS {
            errors.push(DisplayTraceValidationError::InvalidAttemptCount {
                observed: self.attempts.len(),
                required: MAX_DISPLAY_TRACE_ATTEMPTS,
            });
        }
        for (index, attempt) in self.attempts.iter().enumerate() {
            validate_attempt(index, attempt, &self.framebuffer, &mut errors);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Serializes a validated trace as pretty JSON.
    ///
    /// # Errors
    ///
    /// Returns [`DisplayTraceError`] when validation or serialization fails,
    /// or when output exceeds [`MAX_DISPLAY_TRACE_JSON_BYTES`].
    pub fn to_json_pretty(&self) -> Result<String, DisplayTraceError> {
        self.validate().map_err(DisplayTraceError::Validation)?;
        let json = serde_json::to_string_pretty(self).map_err(DisplayTraceError::Json)?;
        if json.len() > MAX_DISPLAY_TRACE_JSON_BYTES {
            return Err(DisplayTraceError::InputTooLarge {
                bytes: json.len(),
                maximum: MAX_DISPLAY_TRACE_JSON_BYTES,
            });
        }
        Ok(json)
    }
}

/// Failures returned while parsing or serializing a display trace.
#[derive(Debug)]
#[non_exhaustive]
pub enum DisplayTraceError {
    /// Serialized input exceeded the fixed byte limit.
    InputTooLarge { bytes: usize, maximum: usize },
    /// JSON syntax or shape was invalid.
    Json(serde_json::Error),
    /// Parsed data violated trace invariants.
    Validation(Vec<DisplayTraceValidationError>),
}

impl std::fmt::Display for DisplayTraceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InputTooLarge { bytes, maximum } => {
                write!(
                    formatter,
                    "display trace is {bytes} bytes; maximum is {maximum}"
                )
            }
            Self::Json(error) => write!(formatter, "invalid display trace JSON: {error}"),
            Self::Validation(errors) => write!(formatter, "invalid display trace: {errors:?}"),
        }
    }
}

impl std::error::Error for DisplayTraceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::InputTooLarge { .. } | Self::Validation(_) => None,
        }
    }
}

/// A fail-closed display trace validation error.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DisplayTraceValidationError {
    /// Schema version is not implemented.
    UnsupportedSchema { observed: u32 },
    /// Redaction was disabled.
    RedactionDisabled,
    /// Redaction policy did not match this schema.
    UnexpectedRedactionPolicy,
    /// One mandatory privacy category was absent.
    MissingRedactionCategory { category: &'static str },
    /// Profile ID was empty, too long, or contained unsafe characters.
    InvalidProfileId,
    /// Framebuffer path was not a numbered `/dev/fb*` path.
    InvalidFramebufferPath,
    /// Virtual geometry was smaller than visible geometry.
    InvalidVirtualExtent,
    /// Stride was too short for visible geometry and bit depth.
    InvalidLineLength,
    /// Memory length was too short for stride and virtual height.
    InvalidMemoryLength,
    /// Version 1 did not contain exactly one attempt.
    InvalidAttemptCount { observed: usize, required: usize },
    /// Region arithmetic overflowed.
    RegionOverflow { index: usize },
    /// Region exceeded the visible framebuffer.
    RegionOutOfBounds { index: usize },
    /// Region exceeded the version-1 small-patch limit.
    RegionAboveMaximum {
        index: usize,
        pixels: u64,
        maximum: u64,
    },
    /// ABI name and request size contradicted each other.
    InvalidRequestSize {
        index: usize,
        observed: u32,
        expected: u32,
    },
    /// ABI name and ioctl request number contradicted each other.
    InvalidRequestIoctl {
        index: usize,
        observed: u64,
        expected: u64,
    },
    /// A confirmed preflight contained at least one false condition.
    IncompletePreflight,
    /// Completion wait exceeded the version-1 timeout bound.
    WaitAboveMaximum {
        index: usize,
        observed: u32,
        maximum: u32,
    },
    /// Completion result was inconsistent with submission and wait plan.
    InconsistentCompletion { index: usize },
}

fn validate_redaction(
    redaction: &RedactionMetadata,
    errors: &mut Vec<DisplayTraceValidationError>,
) {
    if !redaction.enabled {
        errors.push(DisplayTraceValidationError::RedactionDisabled);
    }
    if redaction.policy != CHARACTERIZATION_REDACTION_POLICY {
        errors.push(DisplayTraceValidationError::UnexpectedRedactionPolicy);
    }
    for category in REQUIRED_REDACTION_CATEGORIES {
        if !redaction
            .excluded_categories
            .iter()
            .any(|excluded| excluded == category)
        {
            errors.push(DisplayTraceValidationError::MissingRedactionCategory { category });
        }
    }
}

fn validate_framebuffer(
    framebuffer: &FramebufferFingerprint,
    errors: &mut Vec<DisplayTraceValidationError>,
) {
    if !valid_framebuffer_path(&framebuffer.device) {
        errors.push(DisplayTraceValidationError::InvalidFramebufferPath);
    }
    if framebuffer.virtual_extent.width() < framebuffer.visible.width()
        || framebuffer.virtual_extent.height() < framebuffer.visible.height()
    {
        errors.push(DisplayTraceValidationError::InvalidVirtualExtent);
    }
    let minimum_line_length = u64::from(framebuffer.visible.width())
        .saturating_mul(u64::from(framebuffer.bits_per_pixel.get()))
        .div_ceil(8);
    if u64::from(framebuffer.line_length.get()) < minimum_line_length {
        errors.push(DisplayTraceValidationError::InvalidLineLength);
    }
    let minimum_memory_length = u64::from(framebuffer.line_length.get())
        .saturating_mul(u64::from(framebuffer.virtual_extent.height()));
    if u64::from(framebuffer.memory_length.get()) < minimum_memory_length {
        errors.push(DisplayTraceValidationError::InvalidMemoryLength);
    }
}

fn preflight_is_complete(preflight: DisplayPreflight) -> bool {
    match preflight {
        DisplayPreflight::NotPerformed => true,
        DisplayPreflight::Confirmed {
            operator_present,
            stock_ui_healthy,
            maintenance_connection_healthy,
            power_adequate_and_awake,
            no_transition_or_update,
            output_ready,
            fresh_passive_report,
        } => {
            operator_present
                && stock_ui_healthy
                && maintenance_connection_healthy
                && power_adequate_and_awake
                && no_transition_or_update
                && output_ready
                && fresh_passive_report
        }
    }
}

fn validate_attempt(
    index: usize,
    attempt: &DisplayUpdateAttempt,
    framebuffer: &FramebufferFingerprint,
    errors: &mut Vec<DisplayTraceValidationError>,
) {
    let region = attempt.plan.region;
    let right = region.x.checked_add(region.width.get());
    let bottom = region.y.checked_add(region.height.get());
    if right.is_none() || bottom.is_none() {
        errors.push(DisplayTraceValidationError::RegionOverflow { index });
    } else if right.is_some_and(|value| value > framebuffer.visible.width())
        || bottom.is_some_and(|value| value > framebuffer.visible.height())
    {
        errors.push(DisplayTraceValidationError::RegionOutOfBounds { index });
    }
    let pixels = u64::from(region.width.get()).saturating_mul(u64::from(region.height.get()));
    if pixels > MAX_DISPLAY_TRACE_REGION_PIXELS {
        errors.push(DisplayTraceValidationError::RegionAboveMaximum {
            index,
            pixels,
            maximum: MAX_DISPLAY_TRACE_REGION_PIXELS,
        });
    }

    let observed_size = attempt.plan.abi.request_size.get();
    let expected_size = attempt.plan.abi.kind.expected_request_size();
    if observed_size != expected_size {
        errors.push(DisplayTraceValidationError::InvalidRequestSize {
            index,
            observed: observed_size,
            expected: expected_size,
        });
    }
    let observed_ioctl = attempt.plan.request_ioctl.get();
    let expected_ioctl = attempt.plan.abi.kind.expected_request_ioctl();
    if observed_ioctl != expected_ioctl {
        errors.push(DisplayTraceValidationError::InvalidRequestIoctl {
            index,
            observed: observed_ioctl,
            expected: expected_ioctl,
        });
    }
    if let Some(wait) = attempt.plan.wait
        && wait.timeout_millis.get() > MAX_DISPLAY_WAIT_MILLIS
    {
        errors.push(DisplayTraceValidationError::WaitAboveMaximum {
            index,
            observed: wait.timeout_millis.get(),
            maximum: MAX_DISPLAY_WAIT_MILLIS,
        });
    }

    let completion_is_consistent = match (attempt.submission, attempt.plan.wait) {
        (DisplaySubmissionResult::Submitted, Some(_)) => attempt.completion.is_some(),
        (DisplaySubmissionResult::Submitted, None)
        | (DisplaySubmissionResult::OpenError { .. }, _)
        | (DisplaySubmissionResult::Error { .. }, _) => attempt.completion.is_none(),
    };
    if !completion_is_consistent {
        errors.push(DisplayTraceValidationError::InconsistentCompletion { index });
    }
}

fn valid_framebuffer_path(value: &str) -> bool {
    value.strip_prefix("/dev/fb").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.chars().all(|value| value.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::characterization_redaction_metadata;

    fn non_zero(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).unwrap()
    }

    fn trace() -> DisplayTrace {
        DisplayTrace {
            schema_version: DISPLAY_TRACE_SCHEMA_VERSION,
            redaction: characterization_redaction_metadata(),
            profile_id: "synthetic-display-trace".to_owned(),
            framebuffer: FramebufferFingerprint {
                device: "/dev/fb0".to_owned(),
                driver_id: "mxc_epdc_fb".to_owned(),
                visible: DisplayExtent::try_new(1264, 1680).unwrap(),
                virtual_extent: DisplayExtent::try_new(1280, 3584).unwrap(),
                line_length: non_zero(1280),
                memory_length: non_zero(4_587_520),
                bits_per_pixel: non_zero(8),
                pixel_layout: PixelLayout::Grayscale8,
                rotation: QuarterTurn::Upright,
            },
            preflight: DisplayPreflight::NotPerformed,
            attempts: vec![DisplayUpdateAttempt {
                plan: DisplayUpdatePlan {
                    open_mode: DisplayPlanOpenMode::ReadOnly,
                    memory_access: FramebufferMemoryAccess::None,
                    region: DisplayRegion {
                        x: 0,
                        y: 0,
                        width: non_zero(64),
                        height: non_zero(64),
                    },
                    abi: DisplayUpdateAbi {
                        kind: DisplayUpdateAbiKind::Zelda88,
                        request_size: non_zero(88),
                    },
                    request_ioctl: NonZeroU64::new(0x4058_462e).unwrap(),
                    waveform_mode: 0,
                    update_mode: 0,
                    temperature: 0x1000,
                    flags: 0,
                    dither_mode: 0,
                    quant_bit: 0,
                    alternate_buffer_zeroed: true,
                    histogram_modes: [0, 0],
                    timestamps: [0, 0],
                    marker: non_zero(1),
                    wait: Some(DisplayWaitPlan {
                        timeout_millis: non_zero(1_000),
                    }),
                },
                submission: DisplaySubmissionResult::Submitted,
                completion: Some(DisplayCompletionResult::Completed),
                observation: DisplayObservation::NotObserved,
            }],
            post_run_stock_health: DisplayStockHealth::NotChecked,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn valid_small_trace_round_trips() {
        let trace = trace();
        let json = trace.to_json_pretty().unwrap();
        assert_eq!(DisplayTrace::from_json(&json).unwrap(), trace);
    }

    #[test]
    fn validation_rejects_large_or_out_of_bounds_regions() {
        let mut trace = trace();
        trace.attempts[0].plan.region.width = non_zero(65);
        let errors = trace.validate().unwrap_err();
        assert!(errors.iter().any(|error| matches!(
            error,
            DisplayTraceValidationError::RegionAboveMaximum { .. }
        )));

        trace.attempts[0].plan.region = DisplayRegion {
            x: 1260,
            y: 0,
            width: non_zero(64),
            height: non_zero(64),
        };
        let errors = trace.validate().unwrap_err();
        assert!(errors.contains(&DisplayTraceValidationError::RegionOutOfBounds { index: 0 }));
    }

    #[test]
    fn validation_rejects_abi_size_and_completion_contradictions() {
        let mut trace = trace();
        trace.attempts[0].plan.abi.request_size = non_zero(80);
        trace.attempts[0].plan.request_ioctl = NonZeroU64::new(0x4050_462e).unwrap();
        trace.attempts[0].submission = DisplaySubmissionResult::Error {
            errno: NonZeroU16::new(22).unwrap(),
        };
        let errors = trace.validate().unwrap_err();
        assert!(
            errors.contains(&DisplayTraceValidationError::InvalidRequestSize {
                index: 0,
                observed: 80,
                expected: 88
            })
        );
        assert!(
            errors.contains(&DisplayTraceValidationError::InvalidRequestIoctl {
                index: 0,
                observed: 0x4050_462e,
                expected: 0x4058_462e
            })
        );
        assert!(errors.contains(&DisplayTraceValidationError::InconsistentCompletion { index: 0 }));
    }

    #[test]
    fn validation_rejects_invalid_stride_memory_and_wait_bounds() {
        let mut trace = trace();
        trace.framebuffer.line_length = non_zero(100);
        trace.framebuffer.memory_length = non_zero(100);
        trace.attempts[0].plan.wait = Some(DisplayWaitPlan {
            timeout_millis: non_zero(MAX_DISPLAY_WAIT_MILLIS + 1),
        });
        let errors = trace.validate().unwrap_err();
        assert!(errors.contains(&DisplayTraceValidationError::InvalidLineLength));
        assert!(errors.contains(&DisplayTraceValidationError::InvalidMemoryLength));
        assert!(
            errors
                .iter()
                .any(|error| matches!(error, DisplayTraceValidationError::WaitAboveMaximum { .. }))
        );
    }

    #[test]
    fn validation_rejects_false_confirmed_preflight_condition() {
        let mut trace = trace();
        trace.preflight = DisplayPreflight::Confirmed {
            operator_present: true,
            stock_ui_healthy: true,
            maintenance_connection_healthy: true,
            power_adequate_and_awake: true,
            no_transition_or_update: true,
            output_ready: false,
            fresh_passive_report: true,
        };
        assert!(
            trace
                .validate()
                .unwrap_err()
                .contains(&DisplayTraceValidationError::IncompletePreflight)
        );
    }

    #[test]
    fn parser_rejects_unknown_fields_and_oversized_input() {
        let json = trace().to_json_pretty().unwrap();
        let unknown = json.replacen(
            "\"schema_version\": 1,",
            "\"schema_version\": 1,\n  \"framebuffer_dump\": \"forbidden\",",
            1,
        );
        assert!(matches!(
            DisplayTrace::from_json(&unknown),
            Err(DisplayTraceError::Json(_))
        ));
        let oversized = " ".repeat(MAX_DISPLAY_TRACE_JSON_BYTES + 1);
        assert!(matches!(
            DisplayTrace::from_json(&oversized),
            Err(DisplayTraceError::InputTooLarge { .. })
        ));
    }

    #[test]
    fn fingerprint_serialization_has_no_framebuffer_payload_field() {
        let value = serde_json::to_value(trace()).unwrap();
        let fields = value.as_object().unwrap();
        assert!(!fields.contains_key("framebuffer_data"));
        let framebuffer = fields.get("framebuffer").unwrap().as_object().unwrap();
        assert!(!framebuffer.contains_key("data"));
        assert!(!framebuffer.contains_key("contents"));
    }
}
