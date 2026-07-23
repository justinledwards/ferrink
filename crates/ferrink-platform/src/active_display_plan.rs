//! Offline-only validation for a synthetic reference active display plan.
//!
//! This module accepts already collected strings and data structures. It has no
//! device adapter and cannot open or operate on a framebuffer.

use std::num::{NonZeroU32, NonZeroU64};

use serde::{Deserialize, Serialize};

use crate::{
    DeviceProfile, DisplayRegion, DisplayUpdateAbi, DisplayUpdateAbiKind, EvidenceLevel,
    FramebufferCapability, InputEventAbi, PixelBitfield, PixelLayout, ProbeReport,
    ProfileEvaluationStatus, ProfileReviewStatus, REDACTION_POLICY,
};

/// Active display plan schema understood by this crate.
pub const ACTIVE_DISPLAY_PLAN_SCHEMA_VERSION: u32 = 1;
/// Hard maximum serialized plan size.
pub const MAX_ACTIVE_DISPLAY_PLAN_JSON_BYTES: usize = 65_536;
/// Single-use identifier for the synthetic reference mechanism test.
pub const REFERENCE_PORTRAIT_DISPLAY_MECHANISM_PLAN_ID: &str =
    "reference-portrait-display-mechanism-v1";
/// Synthetic profile to which the test plan is pinned.
pub const REFERENCE_PORTRAIT_DISPLAY_MECHANISM_PROFILE_ID: &str = "reference-portrait";

const REQUIRED_PROBE_VERSION: &str = "0.0.1";
const REQUIRED_SERIAL_PREFIX: &str = "TEST";
const REQUIRED_FIRMWARE_SUBSTRING: &str = "0.0.1-test";
const REQUIRED_KERNEL_RELEASE: &str = "reference-kernel-portrait";
const REQUIRED_MACHINE: &str = "armv7l";
const REQUIRED_OUTPUT_PARENT: &str = "/mnt/us/ferrink/evidence";
const REQUIRED_REQUEST_IOCTL: u64 = 0x4058_462e;
const REQUIRED_WAVEFORM_MODE: u32 = 257;
const REQUIRED_UPDATE_MODE: u32 = 0;
const REQUIRED_MARKER: u32 = 0x464b_0001;
const REQUIRED_TEMPERATURE: i32 = 0x1000;
const REQUIRED_PROBE_REDACTION_CATEGORIES: &[&str] = &[
    "full_serial_numbers",
    "network_credentials_and_ssids",
    "tokens_and_account_data",
    "ssh_keys",
    "document_names",
    "process_command_lines",
];

/// The only descriptor mode authorized by the first KOA3 plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayPlanOpenMode {
    /// Open the framebuffer without write access.
    ReadOnly,
}

/// The first KOA3 plan authorizes no access to framebuffer pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FramebufferMemoryAccess {
    /// Do not read, map, or write framebuffer memory.
    None,
}

/// Exact passive evidence the active plan must see again before execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveDisplayEvidenceMatch {
    /// Version of the passive collector whose semantics were reviewed.
    pub probe_version: String,
    /// Short, non-secret model prefix from the redacted report.
    pub serial_prefix: String,
    /// Required substring in at least one sourced firmware value.
    pub firmware_contains: String,
    /// Exact reviewed kernel release.
    pub kernel_release: String,
    /// Exact reviewed machine string.
    pub machine: String,
    /// Exact libc event layout, retained as an architecture guard.
    pub input_event_abi: InputEventAbi,
    /// Full non-secret framebuffer capability expected at the selected path.
    pub framebuffer: FramebufferCapability,
}

/// Exact request fields authorized by the first KOA3 mechanism plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveDisplayRequestPlan {
    /// Framebuffer path to which the request would be submitted.
    pub device: String,
    /// Descriptor access mode.
    pub open_mode: DisplayPlanOpenMode,
    /// Permitted access to framebuffer pixel memory.
    pub memory_access: FramebufferMemoryAccess,
    /// Fully encoded ioctl request number.
    pub request_ioctl: NonZeroU64,
    /// Small visible update region.
    pub region: DisplayRegion,
    /// Reviewed request layout and size.
    pub abi: DisplayUpdateAbi,
    /// Driver waveform number.
    pub waveform_mode: u32,
    /// Driver update mode number.
    pub update_mode: u32,
    /// Non-zero marker unique to this single-use plan.
    pub marker: NonZeroU32,
    /// Driver temperature value.
    pub temperature: i32,
    /// Update flags.
    pub flags: u32,
    /// Dither mode.
    pub dither_mode: i32,
    /// Quantization bit setting.
    pub quant_bit: i32,
    /// Whether every alternate-buffer field is zero.
    pub alternate_buffer_zeroed: bool,
    /// Zelda grayscale and color histogram modes.
    pub histogram_modes: [u32; 2],
    /// Zelda PXP and EPDC timestamps.
    pub timestamps: [u32; 2],
    /// A completion wait in milliseconds; absent in the first plan.
    pub completion_wait_millis: Option<NonZeroU32>,
    /// Hard request-attempt limit.
    pub maximum_attempts: NonZeroU32,
}

/// Immutable, offline-validated definition of the first active KOA3 request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveDisplayPlan {
    /// Plan schema version.
    pub schema_version: u32,
    /// Single-use reviewed plan identifier.
    pub plan_id: String,
    /// Must remain true; attempted requests consume this plan.
    pub single_use: bool,
    /// Exact reviewed profile identifier.
    pub profile_id: String,
    /// Passive evidence required before any later active adapter may proceed.
    pub evidence: ActiveDisplayEvidenceMatch,
    /// Pre-existing userstore directory under which create-new output belongs.
    pub output_parent: String,
    /// Exact bounded display request.
    pub request: ActiveDisplayRequestPlan,
}

impl ActiveDisplayPlan {
    /// Parses and validates a bounded plan definition without accessing hardware.
    ///
    /// # Errors
    ///
    /// Returns [`ActiveDisplayPlanError`] for oversized or malformed JSON or
    /// when the definition differs from the reviewed single-use plan.
    pub fn from_json(input: &str) -> Result<Self, ActiveDisplayPlanError> {
        if input.len() > MAX_ACTIVE_DISPLAY_PLAN_JSON_BYTES {
            return Err(ActiveDisplayPlanError::InputTooLarge {
                bytes: input.len(),
                maximum: MAX_ACTIVE_DISPLAY_PLAN_JSON_BYTES,
            });
        }
        let plan: Self = serde_json::from_str(input).map_err(ActiveDisplayPlanError::Json)?;
        plan.validate_definition()
            .map_err(ActiveDisplayPlanError::Validation)?;
        Ok(plan)
    }

    /// Validates every immutable field in the reviewed plan definition.
    ///
    /// # Errors
    ///
    /// Returns every field that differs from the first KOA3 plan.
    pub fn validate_definition(&self) -> Result<(), Vec<ActiveDisplayPlanValidationError>> {
        let mut errors = Vec::new();
        compare(
            &mut errors,
            "schema_version",
            ACTIVE_DISPLAY_PLAN_SCHEMA_VERSION,
            self.schema_version,
        );
        compare(
            &mut errors,
            "plan_id",
            REFERENCE_PORTRAIT_DISPLAY_MECHANISM_PLAN_ID,
            self.plan_id.as_str(),
        );
        compare(&mut errors, "single_use", true, self.single_use);
        compare(
            &mut errors,
            "profile_id",
            REFERENCE_PORTRAIT_DISPLAY_MECHANISM_PROFILE_ID,
            self.profile_id.as_str(),
        );
        compare(
            &mut errors,
            "evidence.probe_version",
            REQUIRED_PROBE_VERSION,
            self.evidence.probe_version.as_str(),
        );
        compare(
            &mut errors,
            "evidence.serial_prefix",
            REQUIRED_SERIAL_PREFIX,
            self.evidence.serial_prefix.as_str(),
        );
        compare(
            &mut errors,
            "evidence.firmware_contains",
            REQUIRED_FIRMWARE_SUBSTRING,
            self.evidence.firmware_contains.as_str(),
        );
        compare(
            &mut errors,
            "evidence.kernel_release",
            REQUIRED_KERNEL_RELEASE,
            self.evidence.kernel_release.as_str(),
        );
        compare(
            &mut errors,
            "evidence.machine",
            REQUIRED_MACHINE,
            self.evidence.machine.as_str(),
        );
        compare(
            &mut errors,
            "evidence.input_event_abi",
            required_input_event_abi(),
            self.evidence.input_event_abi,
        );
        validate_expected_framebuffer(&self.evidence.framebuffer, &mut errors);
        compare(
            &mut errors,
            "output_parent",
            REQUIRED_OUTPUT_PARENT,
            self.output_parent.as_str(),
        );
        validate_request(&self.request, &mut errors);

        finish(errors)
    }

    /// Checks this exact plan against a reviewed profile and passive report.
    ///
    /// This function performs no file or device I/O. Live stock-health,
    /// connection, battery, update, mount, and operator-presence checks remain
    /// deliberately unresolved.
    ///
    /// # Errors
    ///
    /// Returns all definition, profile, report, and exact-evidence mismatches.
    pub fn validate_against(
        &self,
        profile: &DeviceProfile,
        report: &ProbeReport,
    ) -> Result<(), Vec<ActiveDisplayPlanValidationError>> {
        let mut errors = self.validate_definition().err().unwrap_or_default();

        if let Err(profile_errors) = profile.validate() {
            errors.push(ActiveDisplayPlanValidationError::InvalidProfile(
                profile_errors,
            ));
        }
        if let Err(report_errors) = report.validate() {
            errors.push(ActiveDisplayPlanValidationError::InvalidReport(
                report_errors,
            ));
        }

        compare(
            &mut errors,
            "profile.id",
            self.profile_id.as_str(),
            profile.id.as_str(),
        );
        compare(
            &mut errors,
            "profile.review_status",
            ProfileReviewStatus::ObservedForeground,
            profile.review_status,
        );
        compare(
            &mut errors,
            "profile.identity.serial_prefixes",
            vec![self.evidence.serial_prefix.clone()],
            profile.identity.serial_prefixes.clone(),
        );
        compare(
            &mut errors,
            "profile.identity.firmware_versions",
            vec![self.evidence.firmware_contains.clone()],
            profile.identity.firmware_versions.clone(),
        );
        compare(
            &mut errors,
            "profile.display.width",
            self.evidence.framebuffer.visible_width,
            profile.display.width,
        );
        compare(
            &mut errors,
            "profile.display.height",
            self.evidence.framebuffer.visible_height,
            profile.display.height,
        );
        compare(
            &mut errors,
            "profile.display.bits_per_pixel",
            self.evidence.framebuffer.bits_per_pixel,
            profile.display.bits_per_pixel,
        );
        compare(
            &mut errors,
            "profile.display.pixel_layout",
            Some(self.evidence.framebuffer.pixel_layout),
            profile.display.pixel_layout,
        );
        compare(
            &mut errors,
            "profile.display.rotation",
            Some(self.evidence.framebuffer.rotation),
            profile.display.rotation,
        );
        compare(
            &mut errors,
            "profile.display.epdc_update_abi",
            Some("zelda-88".to_owned()),
            profile.display.epdc_update_abi.clone(),
        );
        compare(
            &mut errors,
            "profile.requirements.multitouch_axes",
            true,
            profile.requirements.multitouch_axes,
        );
        compare(
            &mut errors,
            "profile.requirements.rtc_wakealarm",
            false,
            profile.requirements.rtc_wakealarm,
        );
        compare(
            &mut errors,
            "profile.requirements.suspend_to_mem",
            false,
            profile.requirements.suspend_to_mem,
        );
        compare(
            &mut errors,
            "profile.evidence.level",
            EvidenceLevel::Observed,
            profile.evidence.level,
        );

        let evaluation = profile.evaluate(report);
        if evaluation.status != ProfileEvaluationStatus::Match {
            errors.push(ActiveDisplayPlanValidationError::ProfileEvaluation {
                missing: evaluation.missing,
                mismatches: evaluation.mismatches,
            });
        }

        compare(
            &mut errors,
            "report.probe_version",
            self.evidence.probe_version.as_str(),
            report.probe_version.as_str(),
        );
        compare(
            &mut errors,
            "report.redaction.policy",
            REDACTION_POLICY,
            report.redaction.policy.as_str(),
        );
        for category in REQUIRED_PROBE_REDACTION_CATEGORIES {
            if !report
                .redaction
                .excluded_categories
                .iter()
                .any(|excluded| excluded == category)
            {
                errors.push(mismatch(
                    "report.redaction.excluded_categories",
                    format!("contains {category:?}"),
                    &report.redaction.excluded_categories,
                ));
            }
        }
        compare(
            &mut errors,
            "report.identity.serial_prefix",
            Some(self.evidence.serial_prefix.as_str()),
            report.identity.serial_prefix.as_deref(),
        );
        if !report.system.firmware.iter().any(|item| {
            item.value
                .contains(self.evidence.firmware_contains.as_str())
        }) {
            errors.push(mismatch(
                "report.system.firmware",
                format!("contains {:?}", self.evidence.firmware_contains),
                format!("{:?}", report.system.firmware),
            ));
        }
        compare(
            &mut errors,
            "report.system.kernel_release",
            Some(self.evidence.kernel_release.as_str()),
            report.system.kernel_release.as_deref(),
        );
        compare(
            &mut errors,
            "report.system.machine",
            Some(self.evidence.machine.as_str()),
            report.system.machine.as_deref(),
        );
        compare(
            &mut errors,
            "report.system.input_event_abi",
            self.evidence.input_event_abi,
            report.system.input_event_abi,
        );

        let matching_device = report
            .framebuffers
            .iter()
            .filter(|framebuffer| framebuffer.device == self.request.device)
            .collect::<Vec<_>>();
        if matching_device.len() != 1 {
            errors.push(mismatch(
                "report.framebuffers.selected_device_count",
                "1",
                matching_device.len(),
            ));
        } else {
            compare(
                &mut errors,
                "report.framebuffers[/dev/fb0]",
                &self.evidence.framebuffer,
                matching_device[0],
            );
        }

        if !report.warnings.is_empty() {
            errors.push(ActiveDisplayPlanValidationError::ProbeWarningsPresent {
                count: report.warnings.len(),
            });
        }

        finish(errors)
    }

    /// Serializes a validated plan as pretty JSON.
    ///
    /// # Errors
    ///
    /// Returns [`ActiveDisplayPlanError`] if validation or serialization fails.
    pub fn to_json_pretty(&self) -> Result<String, ActiveDisplayPlanError> {
        self.validate_definition()
            .map_err(ActiveDisplayPlanError::Validation)?;
        let output = serde_json::to_string_pretty(self).map_err(ActiveDisplayPlanError::Json)?;
        if output.len() > MAX_ACTIVE_DISPLAY_PLAN_JSON_BYTES {
            return Err(ActiveDisplayPlanError::InputTooLarge {
                bytes: output.len(),
                maximum: MAX_ACTIVE_DISPLAY_PLAN_JSON_BYTES,
            });
        }
        Ok(output)
    }
}

/// Failure parsing or validating an offline active-display plan.
#[derive(Debug)]
#[non_exhaustive]
pub enum ActiveDisplayPlanError {
    /// Serialized input exceeded the fixed byte limit.
    InputTooLarge { bytes: usize, maximum: usize },
    /// JSON syntax or shape was invalid.
    Json(serde_json::Error),
    /// The parsed plan differed from the reviewed definition.
    Validation(Vec<ActiveDisplayPlanValidationError>),
}

impl std::fmt::Display for ActiveDisplayPlanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InputTooLarge { bytes, maximum } => {
                write!(
                    formatter,
                    "active display plan is {bytes} bytes; maximum is {maximum}"
                )
            }
            Self::Json(error) => write!(formatter, "invalid active display plan JSON: {error}"),
            Self::Validation(errors) => {
                write!(formatter, "invalid active display plan: {errors:?}")
            }
        }
    }
}

impl std::error::Error for ActiveDisplayPlanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::InputTooLarge { .. } | Self::Validation(_) => None,
        }
    }
}

/// A fail-closed mismatch found during offline validation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ActiveDisplayPlanValidationError {
    /// One immutable value did not match the reviewed plan or evidence.
    Mismatch {
        field: &'static str,
        expected: String,
        observed: String,
    },
    /// The reviewed profile failed its own validation.
    InvalidProfile(Vec<String>),
    /// The passive report failed its own validation.
    InvalidReport(Vec<String>),
    /// The existing general profile matcher did not report a full match.
    ProfileEvaluation {
        missing: Vec<String>,
        mismatches: Vec<String>,
    },
    /// Version 1 rejects all probe warnings because it cannot safely classify
    /// unknown future warning codes as display-irrelevant.
    ProbeWarningsPresent { count: usize },
}

fn required_input_event_abi() -> InputEventAbi {
    InputEventAbi {
        pointer_width_bits: 32,
        libc_timeval_bytes: 8,
        libc_input_event_bytes: 16,
    }
}

fn validate_expected_framebuffer(
    framebuffer: &FramebufferCapability,
    errors: &mut Vec<ActiveDisplayPlanValidationError>,
) {
    compare(
        errors,
        "evidence.framebuffer.device",
        "/dev/fb0",
        framebuffer.device.as_str(),
    );
    compare(
        errors,
        "evidence.framebuffer.driver_id",
        "mxc_epdc_fb",
        framebuffer.driver_id.as_str(),
    );
    compare(
        errors,
        "evidence.framebuffer.visible_width",
        1264,
        framebuffer.visible_width,
    );
    compare(
        errors,
        "evidence.framebuffer.visible_height",
        1680,
        framebuffer.visible_height,
    );
    compare(
        errors,
        "evidence.framebuffer.virtual_width",
        1280,
        framebuffer.virtual_width,
    );
    compare(
        errors,
        "evidence.framebuffer.virtual_height",
        3584,
        framebuffer.virtual_height,
    );
    compare(
        errors,
        "evidence.framebuffer.x_offset",
        0,
        framebuffer.x_offset,
    );
    compare(
        errors,
        "evidence.framebuffer.y_offset",
        0,
        framebuffer.y_offset,
    );
    compare(
        errors,
        "evidence.framebuffer.line_length",
        1280,
        framebuffer.line_length,
    );
    compare(
        errors,
        "evidence.framebuffer.memory_length",
        4_587_520,
        framebuffer.memory_length,
    );
    compare(
        errors,
        "evidence.framebuffer.bits_per_pixel",
        8,
        framebuffer.bits_per_pixel,
    );
    compare(
        errors,
        "evidence.framebuffer.grayscale",
        1,
        framebuffer.grayscale,
    );
    compare(
        errors,
        "evidence.framebuffer.pixel_layout",
        PixelLayout::Grayscale8,
        framebuffer.pixel_layout,
    );
    compare(
        errors,
        "evidence.framebuffer.rotation",
        0,
        framebuffer.rotation,
    );
    let grayscale = PixelBitfield {
        offset: 0,
        length: 8,
        msb_right: 0,
    };
    compare(
        errors,
        "evidence.framebuffer.red",
        grayscale,
        framebuffer.red,
    );
    compare(
        errors,
        "evidence.framebuffer.green",
        grayscale,
        framebuffer.green,
    );
    compare(
        errors,
        "evidence.framebuffer.blue",
        grayscale,
        framebuffer.blue,
    );
    compare(
        errors,
        "evidence.framebuffer.transparency",
        PixelBitfield::default(),
        framebuffer.transparency,
    );
}

fn validate_request(
    request: &ActiveDisplayRequestPlan,
    errors: &mut Vec<ActiveDisplayPlanValidationError>,
) {
    compare(
        errors,
        "request.device",
        "/dev/fb0",
        request.device.as_str(),
    );
    compare(
        errors,
        "request.open_mode",
        DisplayPlanOpenMode::ReadOnly,
        request.open_mode,
    );
    compare(
        errors,
        "request.memory_access",
        FramebufferMemoryAccess::None,
        request.memory_access,
    );
    compare(
        errors,
        "request.request_ioctl",
        REQUIRED_REQUEST_IOCTL,
        request.request_ioctl.get(),
    );
    compare(errors, "request.region.x", 600, request.region.x);
    compare(errors, "request.region.y", 808, request.region.y);
    compare(
        errors,
        "request.region.width",
        64,
        request.region.width.get(),
    );
    compare(
        errors,
        "request.region.height",
        64,
        request.region.height.get(),
    );
    compare(
        errors,
        "request.abi.kind",
        DisplayUpdateAbiKind::Zelda88,
        request.abi.kind,
    );
    compare(
        errors,
        "request.abi.request_size",
        88,
        request.abi.request_size.get(),
    );
    compare(
        errors,
        "request.waveform_mode",
        REQUIRED_WAVEFORM_MODE,
        request.waveform_mode,
    );
    compare(
        errors,
        "request.update_mode",
        REQUIRED_UPDATE_MODE,
        request.update_mode,
    );
    compare(
        errors,
        "request.marker",
        REQUIRED_MARKER,
        request.marker.get(),
    );
    compare(
        errors,
        "request.temperature",
        REQUIRED_TEMPERATURE,
        request.temperature,
    );
    compare(errors, "request.flags", 0, request.flags);
    compare(errors, "request.dither_mode", 0, request.dither_mode);
    compare(errors, "request.quant_bit", 0, request.quant_bit);
    compare(
        errors,
        "request.alternate_buffer_zeroed",
        true,
        request.alternate_buffer_zeroed,
    );
    compare(
        errors,
        "request.histogram_modes",
        [0, 0],
        request.histogram_modes,
    );
    compare(errors, "request.timestamps", [0, 0], request.timestamps);
    compare(
        errors,
        "request.completion_wait_millis",
        None::<NonZeroU32>,
        request.completion_wait_millis,
    );
    compare(
        errors,
        "request.maximum_attempts",
        1,
        request.maximum_attempts.get(),
    );
}

fn compare<T: std::fmt::Debug + PartialEq>(
    errors: &mut Vec<ActiveDisplayPlanValidationError>,
    field: &'static str,
    expected: T,
    observed: T,
) {
    if expected != observed {
        errors.push(mismatch(field, expected, observed));
    }
}

fn mismatch(
    field: &'static str,
    expected: impl std::fmt::Debug,
    observed: impl std::fmt::Debug,
) -> ActiveDisplayPlanValidationError {
    ActiveDisplayPlanValidationError::Mismatch {
        field,
        expected: format!("{expected:?}"),
        observed: format!("{observed:?}"),
    }
}

fn finish(
    errors: Vec<ActiveDisplayPlanValidationError>,
) -> Result<(), Vec<ActiveDisplayPlanValidationError>> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str =
        include_str!("../tests/fixtures/reference-portrait-display-mechanism-plan-v1.json");
    const PROFILE: &str = include_str!("../../../device-profiles/reference-portrait.toml");
    const REPORT: &str = include_str!("../tests/fixtures/probe-reference-portrait.json");

    fn plan() -> ActiveDisplayPlan {
        ActiveDisplayPlan::from_json(PLAN).unwrap()
    }

    fn profile() -> DeviceProfile {
        DeviceProfile::from_toml(PROFILE).unwrap()
    }

    fn report() -> ProbeReport {
        ProbeReport::from_json(REPORT).unwrap()
    }

    #[test]
    fn exact_plan_and_passive_evidence_validate_offline() {
        let plan = plan();
        plan.validate_against(&profile(), &report()).unwrap();
        assert_eq!(
            ActiveDisplayPlan::from_json(&plan.to_json_pretty().unwrap()).unwrap(),
            plan
        );
    }

    #[test]
    fn definition_rejects_request_substitution_and_waits() {
        let mut plan = plan();
        plan.request.update_mode = 1;
        plan.request.completion_wait_millis = NonZeroU32::new(1_000);
        let errors = plan.validate_definition().unwrap_err();
        assert!(has_mismatch(&errors, "request.update_mode"));
        assert!(has_mismatch(&errors, "request.completion_wait_millis"));
    }

    #[test]
    fn evidence_rejects_firmware_abi_geometry_and_warnings() {
        let mut report = report();
        report.system.firmware.clear();
        report.system.input_event_abi.libc_input_event_bytes = 24;
        report.framebuffers[0].line_length = 1264;
        report.warnings.push(crate::ProbeWarning {
            subsystem: "future".to_owned(),
            code: "unknown".to_owned(),
            message: "must fail closed".to_owned(),
        });

        let errors = plan().validate_against(&profile(), &report).unwrap_err();
        assert!(has_mismatch(&errors, "report.system.firmware"));
        assert!(has_mismatch(&errors, "report.system.input_event_abi"));
        assert!(has_mismatch(&errors, "report.framebuffers[/dev/fb0]"));
        assert!(errors.iter().any(|error| matches!(
            error,
            ActiveDisplayPlanValidationError::ProbeWarningsPresent { count: 1 }
        )));
    }

    #[test]
    fn evidence_rejects_profile_abi_substitution() {
        let mut profile = profile();
        profile.display.epdc_update_abi = Some("rex-80".to_owned());
        let errors = plan().validate_against(&profile, &report()).unwrap_err();
        assert!(has_mismatch(&errors, "profile.display.epdc_update_abi"));
    }

    #[test]
    fn parser_rejects_unknown_fields_and_oversized_input() {
        let unknown = PLAN.replacen(
            "\"single_use\": true,",
            "\"single_use\": true,\n  \"execute\": true,",
            1,
        );
        assert!(matches!(
            ActiveDisplayPlan::from_json(&unknown),
            Err(ActiveDisplayPlanError::Json(_))
        ));

        let oversized = " ".repeat(MAX_ACTIVE_DISPLAY_PLAN_JSON_BYTES + 1);
        assert!(matches!(
            ActiveDisplayPlan::from_json(&oversized),
            Err(ActiveDisplayPlanError::InputTooLarge { .. })
        ));
    }

    fn has_mismatch(errors: &[ActiveDisplayPlanValidationError], field: &str) -> bool {
        errors.iter().any(|error| {
            matches!(
                error,
                ActiveDisplayPlanValidationError::Mismatch {
                    field: observed,
                    ..
                } if *observed == field
            )
        })
    }
}
