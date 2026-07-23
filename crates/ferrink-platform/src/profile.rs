use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::{DisplayUpdateAbiKind, PixelLayout, ProbeReport, QuarterTurn, RefreshCapabilities};

pub const PROFILE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceProfile {
    pub schema_version: u32,
    pub id: String,
    pub display_name: String,
    pub review_status: ProfileReviewStatus,
    pub identity: IdentityMatch,
    pub display: DisplayMatch,
    #[serde(default)]
    pub requirements: CapabilityRequirements,
    #[serde(default)]
    pub runtime: Option<RuntimeSelection>,
    pub evidence: ProfileEvidence,
}

impl DeviceProfile {
    pub fn from_toml(input: &str) -> Result<Self, ProfileParseError> {
        let profile: Self = toml::from_str(input).map_err(ProfileParseError::Toml)?;
        profile.validate().map_err(ProfileParseError::Validation)?;
        Ok(profile)
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != PROFILE_SCHEMA_VERSION {
            errors.push(format!(
                "unsupported profile schema version {}",
                self.schema_version
            ));
        }
        if self.id.is_empty()
            || !self.id.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || character == '-'
                    || character == '_'
            })
        {
            errors.push("profile id must use lowercase ASCII letters, digits, '-' or '_'".into());
        }
        if self.identity.serial_prefixes.is_empty()
            && self.review_status != ProfileReviewStatus::EvidencePending
        {
            errors.push("at least one reviewed serial prefix is required".into());
        }
        if self.display.width == 0 || self.display.height == 0 {
            errors.push("profile display geometry must be non-zero".into());
        }
        if self.display.bits_per_pixel == 0 {
            errors.push("profile bits_per_pixel must be non-zero".into());
        }
        if let Some(runtime) = &self.runtime {
            validate_runtime_selection(runtime, &self.display, &mut errors);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn evaluate(&self, report: &ProbeReport) -> ProfileEvaluation {
        let mut missing = Vec::new();
        let mut mismatches = Vec::new();

        if self.identity.serial_prefixes.is_empty() {
            missing.push("profile.identity.serial_prefixes".to_owned());
        } else {
            match report.identity.serial_prefix.as_deref() {
                Some(prefix)
                    if self
                        .identity
                        .serial_prefixes
                        .iter()
                        .any(|item| item == prefix) => {}
                Some(prefix) => mismatches.push(format!("serial prefix {prefix} is not reviewed")),
                None => missing.push("identity.serial_prefix".to_owned()),
            }
        }

        if !self.identity.firmware_versions.is_empty() {
            if report.system.firmware.is_empty() {
                missing.push("system.firmware".to_owned());
            } else if !report.system.firmware.iter().any(|observed| {
                self.identity
                    .firmware_versions
                    .iter()
                    .any(|expected| observed.value.contains(expected))
            }) {
                mismatches.push("firmware is not reviewed by this profile".to_owned());
            }
        }

        if report.framebuffers.is_empty() {
            missing.push("framebuffers".to_owned());
        } else if !report.framebuffers.iter().any(|framebuffer| {
            framebuffer.visible_width == self.display.width
                && framebuffer.visible_height == self.display.height
                && framebuffer.bits_per_pixel == self.display.bits_per_pixel
                && self
                    .display
                    .pixel_layout
                    .is_none_or(|layout| framebuffer.pixel_layout == layout)
                && self
                    .display
                    .rotation
                    .is_none_or(|rotation| framebuffer.rotation == rotation)
        }) {
            mismatches.push("framebuffer geometry or pixel capability does not match".to_owned());
        }

        if self.requirements.multitouch_axes {
            let has_multitouch_axes = report.inputs.iter().any(|input| {
                input.axes.iter().any(|axis| axis.code == 0x35)
                    && input.axes.iter().any(|axis| axis.code == 0x36)
            });
            if !has_multitouch_axes {
                missing.push("input.abs_mt_position_x_y".to_owned());
            }
        }
        if self.requirements.rtc_wakealarm
            && !report.power.rtcs.iter().any(|rtc| rtc.wakealarm.is_some())
        {
            missing.push("power.rtc_wakealarm".to_owned());
        }
        if self.requirements.suspend_to_mem
            && !report
                .power
                .suspend
                .states
                .iter()
                .any(|state| state == "mem")
        {
            missing.push("power.suspend_mem".to_owned());
        }

        let status = if !mismatches.is_empty() {
            ProfileEvaluationStatus::Mismatch
        } else if !missing.is_empty() {
            ProfileEvaluationStatus::MissingCapabilities
        } else {
            ProfileEvaluationStatus::Match
        };
        ProfileEvaluation {
            status,
            missing,
            mismatches,
        }
    }
}

#[derive(Debug)]
pub enum ProfileParseError {
    Toml(toml::de::Error),
    Validation(Vec<String>),
}

impl std::fmt::Display for ProfileParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Toml(error) => write!(formatter, "invalid TOML: {error}"),
            Self::Validation(errors) => write!(formatter, "invalid profile: {}", errors.join("; ")),
        }
    }
}

impl std::error::Error for ProfileParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileReviewStatus {
    EvidencePending,
    ObservedForeground,
    Supported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityMatch {
    pub serial_prefixes: Vec<String>,
    #[serde(default)]
    pub firmware_versions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayMatch {
    pub width: u32,
    pub height: u32,
    pub bits_per_pixel: u32,
    pub pixel_layout: Option<PixelLayout>,
    pub rotation: Option<u32>,
    pub epdc_update_abi: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRequirements {
    #[serde(default)]
    pub multitouch_axes: bool,
    #[serde(default)]
    pub rtc_wakealarm: bool,
    #[serde(default)]
    pub suspend_to_mem: bool,
}

/// Optional reviewed policy required to resolve a passive report at runtime.
///
/// Historical schema-v1 profiles without this extension remain parseable, but
/// they are not runtime-resolvable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSelection {
    /// How to select exactly one framebuffer from the fresh report.
    pub framebuffer: RuntimeFramebufferSelection,
    /// How to select and transform exactly one touch device.
    pub touch: RuntimeTouchSelection,
    /// Reviewed update capability, absent when no exact update ABI is known.
    pub refresh: Option<RuntimeRefreshSelection>,
    /// Reviewed stock-return mechanism, absent until its live card passes.
    #[serde(default)]
    pub stock_repaint: Option<StockRepaintMechanism>,
}

/// Exact vendor-facing action reviewed for restoring the stock presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StockRepaintMechanism {
    /// Execute `/usr/bin/xrefresh -d :0.0` without a shell or inherited environment.
    XrefreshDisplay0,
}

/// Reviewed framebuffer identity independent of its current numbered path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFramebufferSelection {
    /// Exact non-secret kernel framebuffer driver identifier.
    pub driver_id: String,
}

/// Reviewed touchscreen identity, axes, and logical transform policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeTouchSelection {
    /// Exact redacted kernel input-device name.
    pub name: String,
    /// Absolute axis used as raw touch X.
    pub x_axis_code: u16,
    /// Absolute axis used as raw touch Y.
    pub y_axis_code: u16,
    /// Whether the reviewed logical mapping swaps X and Y.
    #[serde(default)]
    pub swap_xy: bool,
    /// Whether logical X is mirrored before rotation.
    #[serde(default)]
    pub invert_x: bool,
    /// Whether logical Y is mirrored before rotation.
    #[serde(default)]
    pub invert_y: bool,
    /// Explicit input-coordinate rotation, independent of framebuffer metadata.
    #[serde(default)]
    pub rotation: QuarterTurn,
}

/// Reviewed generic refresh capability tied to one exact update ABI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeRefreshSelection {
    /// Exact request layout selected by the reviewed profile.
    pub update_abi: DisplayUpdateAbiKind,
    /// Whether partial updates have sufficient foreground evidence.
    pub partial: bool,
    /// Whether full/clean updates have sufficient foreground evidence.
    pub full: bool,
    /// Optional reviewed upper bound for completion-marker waiting.
    pub maximum_completion_wait_millis: Option<NonZeroU32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileEvidence {
    pub level: EvidenceLevel,
    pub source: String,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceLevel {
    Observed,
    Reported,
    Planned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileEvaluationStatus {
    Match,
    MissingCapabilities,
    Mismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileEvaluation {
    pub status: ProfileEvaluationStatus,
    pub missing: Vec<String>,
    pub mismatches: Vec<String>,
}

fn validate_runtime_selection(
    runtime: &RuntimeSelection,
    display: &DisplayMatch,
    errors: &mut Vec<String>,
) {
    if runtime.framebuffer.driver_id.is_empty()
        || runtime.framebuffer.driver_id.len() > 64
        || !runtime
            .framebuffer
            .driver_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        errors.push(
            "runtime framebuffer driver_id must use 1-64 ASCII letters, digits, or '_'".into(),
        );
    }
    if runtime.touch.name.is_empty()
        || runtime.touch.name.len() > 128
        || runtime.touch.name.chars().any(char::is_control)
    {
        errors.push("runtime touch name must be 1-128 non-control characters".into());
    }
    if runtime.touch.x_axis_code == runtime.touch.y_axis_code {
        errors.push("runtime touch X and Y axis codes must differ".into());
    }
    if let Some(refresh) = runtime.refresh {
        if RefreshCapabilities::try_new(
            refresh.partial,
            refresh.full,
            refresh.maximum_completion_wait_millis,
        )
        .is_err()
        {
            errors.push("runtime refresh must enable partial or full updates".into());
        }
        let expected = update_abi_profile_name(refresh.update_abi);
        if display.epdc_update_abi.as_deref() != Some(expected) {
            errors.push(format!(
                "runtime refresh ABI {expected} does not match profile display ABI"
            ));
        }
    }
}

pub(crate) const fn update_abi_profile_name(kind: DisplayUpdateAbiKind) -> &'static str {
    match kind {
        DisplayUpdateAbiKind::Legacy72 => "legacy-72",
        DisplayUpdateAbiKind::Rex80 => "rex-80",
        DisplayUpdateAbiKind::Zelda88 => "zelda-88",
    }
}
