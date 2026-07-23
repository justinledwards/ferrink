//! Strict, sanitized input characterization traces and host-side touch replay.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::{
    DisplayPoint, InputDeviceId, InputEventAbi, InputTransform, ProbeWarning, RawPoint,
    RedactionMetadata, TransformError,
};

/// Input trace schema implemented by this crate.
pub const INPUT_TRACE_SCHEMA_VERSION: u32 = 1;
/// Redaction policy required by characterization traces.
pub const CHARACTERIZATION_REDACTION_POLICY: &str = "ferrink-characterization-v1";
/// Hard maximum accepted serialized input trace size.
pub const MAX_INPUT_TRACE_JSON_BYTES: usize = 1_048_576;
/// Hard maximum declared capture duration.
pub const MAX_INPUT_TRACE_DURATION_MILLIS: u32 = 30_000;
/// Hard maximum declared event count.
pub const MAX_INPUT_TRACE_EVENTS: u32 = 10_000;

pub(crate) const REQUIRED_REDACTION_CATEGORIES: &[&str] = &[
    "full_serial_numbers",
    "network_credentials_and_ssids",
    "tokens_and_account_data",
    "ssh_keys",
    "document_names",
    "process_command_lines",
    "key_text",
];

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const SYN_REPORT: u16 = 0;
const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
const ABS_MT_SLOT: u16 = 47;
const ABS_MT_POSITION_X: u16 = 53;
const ABS_MT_POSITION_Y: u16 = 54;
const ABS_MT_TRACKING_ID: u16 = 57;
const ABS_MT_PRESSURE: u16 = 58;
const BTN_TOOL_FINGER: u16 = 325;
const BTN_TOUCH: u16 = 330;
const MAX_TOUCH_SLOTS: i32 = 256;

/// Returns redaction metadata suitable for a new characterization trace.
#[must_use]
pub fn characterization_redaction_metadata() -> RedactionMetadata {
    RedactionMetadata {
        enabled: true,
        policy: CHARACTERIZATION_REDACTION_POLICY.to_owned(),
        excluded_categories: REQUIRED_REDACTION_CATEGORIES
            .iter()
            .map(|category| (*category).to_owned())
            .collect(),
    }
}

/// Non-secret identity and capability fields for the selected input device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputDeviceFingerprint {
    /// Reviewed evdev path.
    pub device: String,
    /// Kernel device name, redacted by the collector.
    pub name: Option<String>,
    /// Bus/vendor/product/version identifiers.
    pub id: InputDeviceId,
    /// Kernel capability bitsets copied from the passive report.
    pub capabilities: BTreeMap<String, String>,
}

/// A reviewed Linux event type/code pair permitted in a trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputEventCode {
    /// Linux input event type.
    pub event_type: u16,
    /// Linux input event code within `event_type`.
    pub code: u16,
}

/// Fixed capture bounds declared before an input trace starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputTraceLimits {
    /// Maximum wall duration for the future event-read loop.
    pub max_duration_millis: NonZeroU32,
    /// Maximum number of serialized events.
    pub max_events: NonZeroU32,
}

/// One raw Linux input event with a monotonic offset from capture start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputTraceEvent {
    /// Monotonic microseconds since capture start.
    pub offset_micros: u64,
    /// Linux input event type.
    pub event_type: u16,
    /// Linux input event code.
    pub code: u16,
    /// Signed Linux input event value.
    pub value: i32,
}

/// Why a bounded input capture stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputTraceStopReason {
    /// The reviewed interaction completed.
    Completed,
    /// The declared duration limit was reached.
    DurationLimit,
    /// The declared event limit was reached.
    EventLimit,
    /// The selected device disappeared.
    DeviceRemoved,
    /// A read failed before completion.
    ReadError,
    /// The operator stopped capture from the foreground session.
    OperatorStopped,
}

/// A sanitized, bounded input characterization trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputTrace {
    /// Version of this trace schema.
    pub schema_version: u32,
    /// Mandatory redaction contract.
    pub redaction: RedactionMetadata,
    /// Exact reviewed profile ID.
    pub profile_id: String,
    /// Selected device fingerprint from passive evidence.
    pub device: InputDeviceFingerprint,
    /// ABI used to decode future raw event records.
    pub input_event_abi: InputEventAbi,
    /// Reviewed raw-to-display mapping.
    pub transform: InputTransform,
    /// Bounds declared before capture.
    pub limits: InputTraceLimits,
    /// Touch-only codes approved before capture.
    pub allowed_events: Vec<InputEventCode>,
    /// Monotonic bounded event sequence.
    pub events: Vec<InputTraceEvent>,
    /// Why capture stopped.
    pub stop_reason: InputTraceStopReason,
    /// Redacted bounded warnings.
    #[serde(default)]
    pub warnings: Vec<ProbeWarning>,
}

impl InputTrace {
    /// Parses and validates a bounded input trace.
    ///
    /// # Errors
    ///
    /// Returns [`InputTraceError`] for oversized or malformed JSON and for any
    /// schema, privacy, bound, ordering, ABI, path, or event-allowlist failure.
    pub fn from_json(input: &str) -> Result<Self, InputTraceError> {
        if input.len() > MAX_INPUT_TRACE_JSON_BYTES {
            return Err(InputTraceError::InputTooLarge {
                bytes: input.len(),
                maximum: MAX_INPUT_TRACE_JSON_BYTES,
            });
        }
        let trace: Self = serde_json::from_str(input).map_err(InputTraceError::Json)?;
        trace.validate().map_err(InputTraceError::Validation)?;
        Ok(trace)
    }

    /// Validates all fail-closed input trace invariants.
    ///
    /// # Errors
    ///
    /// Returns every detected [`InputTraceValidationError`].
    pub fn validate(&self) -> Result<(), Vec<InputTraceValidationError>> {
        let mut errors = Vec::new();
        if self.schema_version != INPUT_TRACE_SCHEMA_VERSION {
            errors.push(InputTraceValidationError::UnsupportedSchema {
                observed: self.schema_version,
            });
        }
        if !self.redaction.enabled {
            errors.push(InputTraceValidationError::RedactionDisabled);
        }
        if self.redaction.policy != CHARACTERIZATION_REDACTION_POLICY {
            errors.push(InputTraceValidationError::UnexpectedRedactionPolicy);
        }
        for category in REQUIRED_REDACTION_CATEGORIES {
            if !self
                .redaction
                .excluded_categories
                .iter()
                .any(|excluded| excluded == category)
            {
                errors.push(InputTraceValidationError::MissingRedactionCategory { category });
            }
        }
        if !valid_profile_id(&self.profile_id) {
            errors.push(InputTraceValidationError::InvalidProfileId);
        }
        if !valid_event_path(&self.device.device) {
            errors.push(InputTraceValidationError::InvalidDevicePath);
        }
        if !valid_input_event_abi(self.input_event_abi) {
            errors.push(InputTraceValidationError::InvalidInputEventAbi);
        }

        let duration_limit = self.limits.max_duration_millis.get();
        if duration_limit > MAX_INPUT_TRACE_DURATION_MILLIS {
            errors.push(InputTraceValidationError::DurationLimitAboveMaximum {
                declared: duration_limit,
                maximum: MAX_INPUT_TRACE_DURATION_MILLIS,
            });
        }
        let event_limit = self.limits.max_events.get();
        if event_limit > MAX_INPUT_TRACE_EVENTS {
            errors.push(InputTraceValidationError::EventLimitAboveMaximum {
                declared: event_limit,
                maximum: MAX_INPUT_TRACE_EVENTS,
            });
        }

        let mut declared = BTreeSet::new();
        if self.allowed_events.is_empty() {
            errors.push(InputTraceValidationError::MissingAllowedEvents);
        }
        for code in &self.allowed_events {
            if !safe_touch_code(*code) {
                errors.push(InputTraceValidationError::DisallowedCodeDeclaration {
                    event_type: code.event_type,
                    code: code.code,
                });
            }
            if !declared.insert(*code) {
                errors.push(InputTraceValidationError::DuplicateAllowedEvent {
                    event_type: code.event_type,
                    code: code.code,
                });
            }
        }

        if self.events.len() > usize::try_from(event_limit).unwrap_or(usize::MAX) {
            errors.push(InputTraceValidationError::TooManyEvents {
                observed: self.events.len(),
                declared: event_limit,
            });
        }
        let maximum_offset = u64::from(duration_limit).saturating_mul(1_000);
        let mut previous_offset = None;
        for (index, event) in self.events.iter().enumerate() {
            if previous_offset.is_some_and(|previous| event.offset_micros < previous) {
                errors.push(InputTraceValidationError::DecreasingOffset { index });
            }
            previous_offset = Some(event.offset_micros);
            if event.offset_micros > maximum_offset {
                errors.push(InputTraceValidationError::EventPastDuration { index });
            }
            let code = InputEventCode {
                event_type: event.event_type,
                code: event.code,
            };
            if !declared.contains(&code) {
                errors.push(InputTraceValidationError::UndeclaredEvent {
                    index,
                    event_type: event.event_type,
                    code: event.code,
                });
            }
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
    /// Returns [`InputTraceError`] when validation or serialization fails, or
    /// when serialized output exceeds [`MAX_INPUT_TRACE_JSON_BYTES`].
    pub fn to_json_pretty(&self) -> Result<String, InputTraceError> {
        self.validate().map_err(InputTraceError::Validation)?;
        let json = serde_json::to_string_pretty(self).map_err(InputTraceError::Json)?;
        if json.len() > MAX_INPUT_TRACE_JSON_BYTES {
            return Err(InputTraceError::InputTooLarge {
                bytes: json.len(),
                maximum: MAX_INPUT_TRACE_JSON_BYTES,
            });
        }
        Ok(json)
    }

    /// Replays reviewed Protocol-B touch events into logical display events.
    ///
    /// Secondary contacts never release the selected primary contact, and
    /// coordinates received before a tracking ID are retained until the next
    /// synchronization report.
    ///
    /// # Errors
    ///
    /// Returns [`InputReplayError`] if the trace is invalid, a slot/tracking
    /// value is unsafe, replay state is contradictory, or transformation fails.
    pub fn replay_touch(&self) -> Result<Vec<LogicalTouchEvent>, InputReplayError> {
        self.validate().map_err(InputReplayError::InvalidTrace)?;
        let mut tracker = TouchTracker::new(self.transform);
        let mut output = Vec::new();
        for event in &self.events {
            if let Some(contact) = tracker.push(event.event_type, event.code, event.value)? {
                output.push(LogicalTouchEvent {
                    offset_micros: event.offset_micros,
                    phase: contact.phase,
                    point: contact.point,
                });
            }
        }
        Ok(output)
    }
}

/// Failures returned while parsing or serializing an input trace.
#[derive(Debug)]
#[non_exhaustive]
pub enum InputTraceError {
    /// Serialized input exceeded the fixed byte limit.
    InputTooLarge { bytes: usize, maximum: usize },
    /// JSON syntax or shape was invalid.
    Json(serde_json::Error),
    /// Parsed data violated trace invariants.
    Validation(Vec<InputTraceValidationError>),
}

impl std::fmt::Display for InputTraceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InputTooLarge { bytes, maximum } => {
                write!(
                    formatter,
                    "input trace is {bytes} bytes; maximum is {maximum}"
                )
            }
            Self::Json(error) => write!(formatter, "invalid input trace JSON: {error}"),
            Self::Validation(errors) => {
                write!(formatter, "invalid input trace: {errors:?}")
            }
        }
    }
}

impl std::error::Error for InputTraceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::InputTooLarge { .. } | Self::Validation(_) => None,
        }
    }
}

/// A fail-closed input trace validation error.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputTraceValidationError {
    /// Schema version is not implemented.
    UnsupportedSchema { observed: u32 },
    /// Redaction was disabled.
    RedactionDisabled,
    /// Redaction policy did not match this schema.
    UnexpectedRedactionPolicy,
    /// One mandatory privacy category was absent.
    MissingRedactionCategory { category: &'static str },
    /// Profile ID was empty or contained unsafe characters.
    InvalidProfileId,
    /// Device path was not a numbered `/dev/input/event*` path.
    InvalidDevicePath,
    /// Event ABI fields were not internally consistent.
    InvalidInputEventAbi,
    /// Declared duration exceeded the hard schema bound.
    DurationLimitAboveMaximum { declared: u32, maximum: u32 },
    /// Declared event count exceeded the hard schema bound.
    EventLimitAboveMaximum { declared: u32, maximum: u32 },
    /// No event code was reviewed before capture.
    MissingAllowedEvents,
    /// A declared event code is outside the touch-only schema.
    DisallowedCodeDeclaration { event_type: u16, code: u16 },
    /// A declared event code was repeated.
    DuplicateAllowedEvent { event_type: u16, code: u16 },
    /// Serialized event count exceeded the declared bound.
    TooManyEvents { observed: usize, declared: u32 },
    /// A monotonic offset decreased.
    DecreasingOffset { index: usize },
    /// An event occurred after the declared duration.
    EventPastDuration { index: usize },
    /// An event was not in the reviewed per-trace allowlist.
    UndeclaredEvent {
        index: usize,
        event_type: u16,
        code: u16,
    },
}

/// Logical phase produced by deterministic touch replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogicalTouchPhase {
    /// Primary contact became complete and active.
    Pressed,
    /// Primary contact coordinates changed.
    Moved,
    /// Primary contact ended.
    Released,
}

/// One logical contact update emitted by the incremental touch tracker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TouchContactEvent {
    /// Logical contact phase.
    pub phase: LogicalTouchPhase,
    /// Transformed point inside the output display extent.
    pub point: DisplayPoint,
}

/// One logical display-space touch event produced at `SYN_REPORT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogicalTouchEvent {
    /// Monotonic offset of the synchronization report.
    pub offset_micros: u64,
    /// Logical contact phase.
    pub phase: LogicalTouchPhase,
    /// Transformed point inside the output display extent.
    pub point: DisplayPoint,
}

/// A deterministic touch replay failure.
#[derive(Debug)]
#[non_exhaustive]
pub enum InputReplayError {
    /// Trace validation failed before replay.
    InvalidTrace(Vec<InputTraceValidationError>),
    /// Slot value was negative or above the schema bound.
    InvalidSlot { value: i32 },
    /// Tracking ID was less than `-1`.
    InvalidTrackingId { value: i32 },
    /// A live slot's tracking ID changed without a release.
    TrackingIdReplaced { slot: i32 },
    /// Internal slot state became contradictory.
    InvalidState,
    /// Coordinate mapping failed.
    Transform(TransformError),
}

impl std::fmt::Display for InputReplayError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTrace(errors) => write!(formatter, "invalid input trace: {errors:?}"),
            Self::InvalidSlot { value } => write!(formatter, "invalid touch slot {value}"),
            Self::InvalidTrackingId { value } => {
                write!(formatter, "invalid touch tracking ID {value}")
            }
            Self::TrackingIdReplaced { slot } => {
                write!(
                    formatter,
                    "tracking ID replaced without release in slot {slot}"
                )
            }
            Self::InvalidState => formatter.write_str("contradictory touch replay state"),
            Self::Transform(error) => write!(formatter, "touch transform failed: {error}"),
        }
    }
}

impl std::error::Error for InputReplayError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transform(error) => Some(error),
            Self::InvalidTrace(_)
            | Self::InvalidSlot { .. }
            | Self::InvalidTrackingId { .. }
            | Self::TrackingIdReplaced { .. }
            | Self::InvalidState => None,
        }
    }
}

impl From<TransformError> for InputReplayError {
    fn from(error: TransformError) -> Self {
        Self::Transform(error)
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct SlotState {
    tracking_id: Option<i32>,
    raw_x: Option<i32>,
    raw_y: Option<i32>,
    dirty: bool,
    reported: bool,
    pending_release: bool,
}

/// Incremental Protocol-B touch state for a live or replayed event stream.
///
/// Coordinates received before the first tracking ID are retained, one primary
/// contact owns the Slint pointer until release, and secondary-slot movement or
/// release cannot move or release that pointer.
#[derive(Debug, Clone)]
pub struct TouchTracker {
    transform: InputTransform,
    current_slot: i32,
    primary_slot: Option<i32>,
    slots: BTreeMap<i32, SlotState>,
}

impl TouchTracker {
    /// Creates empty touch state for one reviewed coordinate transform.
    #[must_use]
    pub fn new(transform: InputTransform) -> Self {
        Self {
            transform,
            current_slot: 0,
            primary_slot: None,
            slots: BTreeMap::new(),
        }
    }

    /// Applies one decoded Linux event and emits at most one synchronized
    /// contact update.
    ///
    /// # Errors
    ///
    /// Returns [`InputReplayError`] for an unsupported type/code, unsafe slot
    /// or tracking value, contradictory state, or failed coordinate transform.
    pub fn push(
        &mut self,
        event_type: u16,
        code: u16,
        value: i32,
    ) -> Result<Option<TouchContactEvent>, InputReplayError> {
        match (event_type, code) {
            (EV_SYN, SYN_REPORT) => self.synchronize(),
            (EV_ABS, ABS_MT_SLOT) => {
                self.select_slot(value)?;
                Ok(None)
            }
            (EV_ABS, ABS_MT_POSITION_X) => {
                self.update_x(self.current_slot, value);
                Ok(None)
            }
            (EV_ABS, ABS_MT_POSITION_Y) => {
                self.update_y(self.current_slot, value);
                Ok(None)
            }
            (EV_ABS, ABS_MT_TRACKING_ID) => {
                self.update_tracking(value)?;
                Ok(None)
            }
            (EV_ABS, ABS_X) => {
                self.update_x(0, value);
                Ok(None)
            }
            (EV_ABS, ABS_Y) => {
                self.update_y(0, value);
                Ok(None)
            }
            (EV_ABS, ABS_MT_PRESSURE) | (EV_KEY, BTN_TOOL_FINGER) | (EV_KEY, BTN_TOUCH) => Ok(None),
            _ => Err(InputReplayError::InvalidState),
        }
    }

    fn select_slot(&mut self, slot: i32) -> Result<(), InputReplayError> {
        if !(0..MAX_TOUCH_SLOTS).contains(&slot) {
            return Err(InputReplayError::InvalidSlot { value: slot });
        }
        self.current_slot = slot;
        Ok(())
    }

    fn update_x(&mut self, slot: i32, value: i32) {
        let state = self.slots.entry(slot).or_default();
        state.dirty |= state.raw_x != Some(value);
        state.raw_x = Some(value);
    }

    fn update_y(&mut self, slot: i32, value: i32) {
        let state = self.slots.entry(slot).or_default();
        state.dirty |= state.raw_y != Some(value);
        state.raw_y = Some(value);
    }

    fn update_tracking(&mut self, value: i32) -> Result<(), InputReplayError> {
        if value < -1 {
            return Err(InputReplayError::InvalidTrackingId { value });
        }
        let slot = self.current_slot;
        if value == -1 {
            if self.primary_slot == Some(slot) {
                let state = self.slots.entry(slot).or_default();
                state.tracking_id = None;
                state.pending_release = true;
                state.dirty = true;
            } else {
                self.slots.remove(&slot);
            }
            return Ok(());
        }

        let state = self.slots.entry(slot).or_default();
        if state
            .tracking_id
            .is_some_and(|tracking_id| tracking_id != value)
        {
            return Err(InputReplayError::TrackingIdReplaced { slot });
        }
        state.tracking_id = Some(value);
        state.pending_release = false;
        state.dirty = true;
        if self.primary_slot.is_none() {
            self.primary_slot = Some(slot);
        }
        Ok(())
    }

    fn synchronize(&mut self) -> Result<Option<TouchContactEvent>, InputReplayError> {
        let Some(primary_slot) = self.primary_slot else {
            return Ok(None);
        };
        let state = self
            .slots
            .get(&primary_slot)
            .copied()
            .ok_or(InputReplayError::InvalidState)?;

        if state.pending_release {
            let event = state
                .reported
                .then(|| self.map_state(state))
                .transpose()?
                .map(|point| TouchContactEvent {
                    phase: LogicalTouchPhase::Released,
                    point,
                });
            self.slots.remove(&primary_slot);
            self.primary_slot = None;
            return Ok(event);
        }

        if state.tracking_id.is_none() || state.raw_x.is_none() || state.raw_y.is_none() {
            return Ok(None);
        }
        let phase = if !state.reported {
            Some(LogicalTouchPhase::Pressed)
        } else if state.dirty {
            Some(LogicalTouchPhase::Moved)
        } else {
            None
        };
        if let Some(phase) = phase {
            let point = self.map_state(state)?;
            let state = self
                .slots
                .get_mut(&primary_slot)
                .ok_or(InputReplayError::InvalidState)?;
            state.reported = true;
            state.dirty = false;
            return Ok(Some(TouchContactEvent { phase, point }));
        }
        Ok(None)
    }

    fn map_state(&self, state: SlotState) -> Result<DisplayPoint, InputReplayError> {
        let x = state.raw_x.ok_or(InputReplayError::InvalidState)?;
        let y = state.raw_y.ok_or(InputReplayError::InvalidState)?;
        self.transform.map(RawPoint { x, y }).map_err(Into::into)
    }
}

pub(crate) fn valid_profile_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '-'
                || character == '_'
        })
}

fn valid_event_path(value: &str) -> bool {
    value
        .strip_prefix("/dev/input/event")
        .is_some_and(|suffix| {
            !suffix.is_empty() && suffix.chars().all(|value| value.is_ascii_digit())
        })
}

fn valid_input_event_abi(abi: InputEventAbi) -> bool {
    matches!(
        (
            abi.pointer_width_bits,
            abi.libc_timeval_bytes,
            abi.libc_input_event_bytes
        ),
        (32, 8, 16) | (64, 16, 24)
    )
}

fn safe_touch_code(code: InputEventCode) -> bool {
    matches!(
        (code.event_type, code.code),
        (EV_SYN, SYN_REPORT)
            | (EV_KEY, BTN_TOOL_FINGER | BTN_TOUCH)
            | (
                EV_ABS,
                ABS_X
                    | ABS_Y
                    | ABS_MT_SLOT
                    | ABS_MT_POSITION_X
                    | ABS_MT_POSITION_Y
                    | ABS_MT_TRACKING_ID
                    | ABS_MT_PRESSURE
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AxisRange, DisplayExtent};

    fn minimal_trace() -> InputTrace {
        InputTrace {
            schema_version: INPUT_TRACE_SCHEMA_VERSION,
            redaction: characterization_redaction_metadata(),
            profile_id: "reference-landscape".to_owned(),
            device: InputDeviceFingerprint {
                device: "/dev/input/event0".to_owned(),
                name: Some("cyttsp".to_owned()),
                id: InputDeviceId::default(),
                capabilities: BTreeMap::new(),
            },
            input_event_abi: InputEventAbi {
                pointer_width_bits: 32,
                libc_timeval_bytes: 8,
                libc_input_event_bytes: 16,
            },
            transform: InputTransform::new(
                AxisRange::try_new(0, 758).unwrap(),
                AxisRange::try_new(0, 1024).unwrap(),
                DisplayExtent::try_new(758, 1024).unwrap(),
            ),
            limits: InputTraceLimits {
                max_duration_millis: NonZeroU32::new(1_000).unwrap(),
                max_events: NonZeroU32::new(100).unwrap(),
            },
            allowed_events: vec![InputEventCode {
                event_type: EV_SYN,
                code: SYN_REPORT,
            }],
            events: Vec::new(),
            stop_reason: InputTraceStopReason::Completed,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn trace_validation_rejects_missing_privacy_and_unreviewed_key_codes() {
        let mut trace = minimal_trace();
        trace
            .redaction
            .excluded_categories
            .retain(|value| value != "key_text");
        trace.allowed_events.push(InputEventCode {
            event_type: EV_KEY,
            code: 30,
        });

        let errors = trace.validate().unwrap_err();
        assert!(
            errors.contains(&InputTraceValidationError::MissingRedactionCategory {
                category: "key_text"
            })
        );
        assert!(
            errors.contains(&InputTraceValidationError::DisallowedCodeDeclaration {
                event_type: EV_KEY,
                code: 30
            })
        );
    }

    #[test]
    fn trace_validation_rejects_decreasing_undeclared_and_over_duration_events() {
        let mut trace = minimal_trace();
        trace.events = vec![
            InputTraceEvent {
                offset_micros: 1_000_001,
                event_type: EV_SYN,
                code: SYN_REPORT,
                value: 0,
            },
            InputTraceEvent {
                offset_micros: 2,
                event_type: EV_ABS,
                code: ABS_X,
                value: 1,
            },
        ];

        let errors = trace.validate().unwrap_err();
        assert!(errors.contains(&InputTraceValidationError::EventPastDuration { index: 0 }));
        assert!(errors.contains(&InputTraceValidationError::DecreasingOffset { index: 1 }));
        assert!(
            errors.contains(&InputTraceValidationError::UndeclaredEvent {
                index: 1,
                event_type: EV_ABS,
                code: ABS_X
            })
        );
    }

    #[test]
    fn trace_json_rejects_unknown_fields_and_zero_limits() {
        let json = minimal_trace().to_json_pretty().unwrap();
        let unknown = json.replacen(
            "\"schema_version\": 1,",
            "\"schema_version\": 1,\n  \"secret\": true,",
            1,
        );
        assert!(matches!(
            InputTrace::from_json(&unknown),
            Err(InputTraceError::Json(_))
        ));

        let zero_limit = json.replace("\"max_events\": 100", "\"max_events\": 0");
        assert!(matches!(
            InputTrace::from_json(&zero_limit),
            Err(InputTraceError::Json(_))
        ));
    }

    #[test]
    fn trace_json_size_is_bounded_before_parsing() {
        let oversized = " ".repeat(MAX_INPUT_TRACE_JSON_BYTES + 1);
        assert!(matches!(
            InputTrace::from_json(&oversized),
            Err(InputTraceError::InputTooLarge { .. })
        ));
    }

    #[test]
    fn replay_rejects_invalid_slot_and_tracking_values() {
        let mut trace = minimal_trace();
        trace.allowed_events.extend([
            InputEventCode {
                event_type: EV_ABS,
                code: ABS_MT_SLOT,
            },
            InputEventCode {
                event_type: EV_ABS,
                code: ABS_MT_TRACKING_ID,
            },
        ]);
        trace.events = vec![InputTraceEvent {
            offset_micros: 0,
            event_type: EV_ABS,
            code: ABS_MT_SLOT,
            value: -1,
        }];
        assert!(matches!(
            trace.replay_touch(),
            Err(InputReplayError::InvalidSlot { value: -1 })
        ));

        trace.events[0] = InputTraceEvent {
            offset_micros: 0,
            event_type: EV_ABS,
            code: ABS_MT_TRACKING_ID,
            value: -2,
        };
        assert!(matches!(
            trace.replay_touch(),
            Err(InputReplayError::InvalidTrackingId { value: -2 })
        ));
    }

    #[test]
    fn incremental_tracker_preserves_pre_tracking_coordinates_and_primary_contact() {
        let mut tracker = TouchTracker::new(minimal_trace().transform);

        assert_eq!(tracker.push(EV_ABS, ABS_MT_POSITION_X, 100).unwrap(), None);
        assert_eq!(tracker.push(EV_ABS, ABS_MT_POSITION_Y, 200).unwrap(), None);
        assert_eq!(tracker.push(EV_ABS, ABS_MT_TRACKING_ID, 7).unwrap(), None);
        let pressed = tracker
            .push(EV_SYN, SYN_REPORT, 0)
            .unwrap()
            .expect("complete primary contact should press");
        assert_eq!(pressed.phase, LogicalTouchPhase::Pressed);

        assert_eq!(tracker.push(EV_ABS, ABS_MT_SLOT, 1).unwrap(), None);
        assert_eq!(tracker.push(EV_ABS, ABS_MT_TRACKING_ID, 8).unwrap(), None);
        assert_eq!(tracker.push(EV_ABS, ABS_MT_POSITION_X, 700).unwrap(), None);
        assert_eq!(tracker.push(EV_ABS, ABS_MT_POSITION_Y, 900).unwrap(), None);
        assert_eq!(tracker.push(EV_SYN, SYN_REPORT, 0).unwrap(), None);
        assert_eq!(tracker.push(EV_ABS, ABS_MT_TRACKING_ID, -1).unwrap(), None);
        assert_eq!(tracker.push(EV_SYN, SYN_REPORT, 0).unwrap(), None);

        assert_eq!(tracker.push(EV_ABS, ABS_MT_SLOT, 0).unwrap(), None);
        assert_eq!(tracker.push(EV_ABS, ABS_MT_TRACKING_ID, -1).unwrap(), None);
        let released = tracker
            .push(EV_SYN, SYN_REPORT, 0)
            .unwrap()
            .expect("primary release should be emitted");
        assert_eq!(released.phase, LogicalTouchPhase::Released);
        assert_eq!(released.point, pressed.point);
    }
}
