//! Fail-closed resolution from reviewed profiles and passive reports.

use std::num::NonZeroU32;

use crate::{
    AxisRange, DeviceProfile, DisplayExtent, DisplayUpdateAbiKind, ElfClass, Endianness,
    FramebufferCapability, FramebufferLayoutError, Gray8FramebufferLayout, InputDeviceCapability,
    InputEventAbi, InputEventDecodeError, InputEventDecoder, InputTransform, ProbeReport,
    ProfileEvaluation, ProfileEvaluationStatus, ProfileReviewStatus, QuarterTurn,
    RefreshCapabilities, RefreshCapabilityError, RuntimeRefreshSelection, RuntimeSelection,
    StockRepaintMechanism, TransformError,
};

/// Maximum integration authority carried by a resolved profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeAuthority {
    /// Manual L0/L1 foreground execution only; never a boot-default claim.
    ForegroundOnly,
    /// The profile itself has passed the separately defined support gates.
    SupportedProfile,
}

/// Reviewed refresh capability resolved for one exact request ABI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedRefreshConfig {
    update_abi: DisplayUpdateAbiKind,
    capabilities: RefreshCapabilities,
}

impl ResolvedRefreshConfig {
    /// Returns the exact reviewed request layout.
    #[must_use]
    pub const fn update_abi(self) -> DisplayUpdateAbiKind {
        self.update_abi
    }

    /// Returns the toolkit-neutral reviewed refresh capability.
    #[must_use]
    pub const fn capabilities(self) -> RefreshCapabilities {
        self.capabilities
    }
}

/// Fully resolved host-side runtime configuration for one passive report.
///
/// Construction validates all owned values. This type opens no paths and
/// carries no standing authorization to perform device I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRuntimeDevice {
    profile_id: String,
    authority: RuntimeAuthority,
    framebuffer_path: String,
    framebuffer_capability: FramebufferCapability,
    framebuffer_layout: Gray8FramebufferLayout,
    framebuffer_rotation: QuarterTurn,
    input_path: String,
    input_name: String,
    input_capability: InputDeviceCapability,
    input_event_abi: InputEventAbi,
    input_endianness: Endianness,
    input_transform: InputTransform,
    refresh: Option<ResolvedRefreshConfig>,
    stock_repaint: Option<StockRepaintMechanism>,
}

impl ResolvedRuntimeDevice {
    /// Resolves one reviewed profile against one fresh passive report.
    ///
    /// The selection is unique and fail-closed: report warnings, incomplete
    /// review, missing runtime policy, ambiguous devices, invalid paths,
    /// unsupported layout/ABI, missing axes, or profile mismatches all fail.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeResolutionError`] for any invalid, missing,
    /// contradictory, ambiguous, or unsupported input.
    pub fn resolve(
        profile: &DeviceProfile,
        report: &ProbeReport,
    ) -> Result<Self, RuntimeResolutionError> {
        profile
            .validate()
            .map_err(RuntimeResolutionError::InvalidProfile)?;
        report
            .validate()
            .map_err(RuntimeResolutionError::InvalidReport)?;
        if !report.warnings.is_empty() {
            return Err(RuntimeResolutionError::ProbeWarningsPresent {
                count: report.warnings.len(),
            });
        }
        let authority = match profile.review_status {
            ProfileReviewStatus::EvidencePending => {
                return Err(RuntimeResolutionError::ProfileEvidencePending);
            }
            ProfileReviewStatus::ObservedForeground => RuntimeAuthority::ForegroundOnly,
            ProfileReviewStatus::Supported => RuntimeAuthority::SupportedProfile,
        };
        let evaluation = profile.evaluate(report);
        if evaluation.status != ProfileEvaluationStatus::Match {
            return Err(RuntimeResolutionError::ProfileDidNotMatch(evaluation));
        }
        let runtime = profile
            .runtime
            .as_ref()
            .ok_or(RuntimeResolutionError::RuntimePolicyMissing)?;
        let framebuffer = select_framebuffer(profile, report, runtime)?;
        if !valid_numbered_path(&framebuffer.device, "/dev/fb") {
            return Err(RuntimeResolutionError::InvalidFramebufferPath);
        }
        let framebuffer_layout = Gray8FramebufferLayout::try_from_capability(framebuffer)
            .map_err(RuntimeResolutionError::FramebufferLayout)?;
        let framebuffer_rotation = QuarterTurn::try_from_linux_framebuffer(framebuffer.rotation)
            .map_err(RuntimeResolutionError::FramebufferRotation)?;

        let input = select_touch(report, runtime)?;
        if !valid_numbered_path(&input.device, "/dev/input/event") {
            return Err(RuntimeResolutionError::InvalidInputPath);
        }
        let raw_x = select_axis(input, runtime.touch.x_axis_code)?;
        let raw_y = select_axis(input, runtime.touch.y_axis_code)?;
        let transform_extent =
            extent_before_rotation(framebuffer_layout.visible(), runtime.touch.rotation);
        let input_transform = InputTransform::new(raw_x, raw_y, transform_extent)
            .with_swap_xy(runtime.touch.swap_xy)
            .with_invert_x(runtime.touch.invert_x)
            .with_invert_y(runtime.touch.invert_y)
            .with_rotation(runtime.touch.rotation);
        if input_transform.output_extent() != framebuffer_layout.visible() {
            return Err(RuntimeResolutionError::InputOutputExtentMismatch);
        }

        let executable = report
            .system
            .executable_abi
            .as_ref()
            .ok_or(RuntimeResolutionError::ExecutableAbiMissing)?;
        let input_event_abi = report.system.input_event_abi;
        let class_matches_pointer_width = matches!(
            (executable.class, input_event_abi.pointer_width_bits),
            (ElfClass::Elf32, 32) | (ElfClass::Elf64, 64)
        );
        if !class_matches_pointer_width {
            return Err(RuntimeResolutionError::ExecutableInputAbiMismatch {
                class: executable.class,
                pointer_width_bits: input_event_abi.pointer_width_bits,
            });
        }
        InputEventDecoder::try_new(input_event_abi, executable.endianness, NonZeroU32::MIN)
            .map_err(RuntimeResolutionError::InputEventAbi)?;
        let refresh = runtime.refresh.map(resolve_refresh).transpose()?;

        Ok(Self {
            profile_id: profile.id.clone(),
            authority,
            framebuffer_path: framebuffer.device.clone(),
            framebuffer_capability: framebuffer.clone(),
            framebuffer_layout,
            framebuffer_rotation,
            input_path: input.device.clone(),
            input_name: runtime.touch.name.clone(),
            input_capability: input.clone(),
            input_event_abi,
            input_endianness: executable.endianness,
            input_transform,
            refresh,
            stock_repaint: runtime.stock_repaint,
        })
    }

    /// Returns the reviewed profile identifier.
    #[must_use]
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Returns the maximum integration authority of this resolution.
    #[must_use]
    pub const fn authority(&self) -> RuntimeAuthority {
        self.authority
    }

    /// Returns the current numbered framebuffer path from the passive report.
    #[must_use]
    pub fn framebuffer_path(&self) -> &str {
        &self.framebuffer_path
    }

    /// Returns the exact passive framebuffer capability selected at resolution.
    ///
    /// A concrete adapter must re-query and compare this snapshot after opening
    /// the descriptor and before mapping or writing framebuffer memory.
    #[must_use]
    pub const fn framebuffer_capability(&self) -> &FramebufferCapability {
        &self.framebuffer_capability
    }

    /// Returns the validated Gray8 layout.
    #[must_use]
    pub const fn framebuffer_layout(&self) -> Gray8FramebufferLayout {
        self.framebuffer_layout
    }

    /// Returns checked framebuffer rotation metadata.
    #[must_use]
    pub const fn framebuffer_rotation(&self) -> QuarterTurn {
        self.framebuffer_rotation
    }

    /// Returns the current numbered input path from the passive report.
    #[must_use]
    pub fn input_path(&self) -> &str {
        &self.input_path
    }

    /// Returns the exact reviewed kernel input name.
    #[must_use]
    pub fn input_name(&self) -> &str {
        &self.input_name
    }

    /// Returns the exact passive input capability selected at resolution.
    ///
    /// A concrete adapter must re-query and compare this snapshot after opening
    /// the descriptor and before reading events or requesting a grab.
    #[must_use]
    pub const fn input_capability(&self) -> &InputDeviceCapability {
        &self.input_capability
    }

    /// Returns the probed raw-event ABI.
    #[must_use]
    pub const fn input_event_abi(&self) -> InputEventAbi {
        self.input_event_abi
    }

    /// Returns the byte order used by the raw-event decoder.
    #[must_use]
    pub const fn input_endianness(&self) -> Endianness {
        self.input_endianness
    }

    /// Returns the reviewed raw-touch to physical-display transform.
    #[must_use]
    pub const fn input_transform(&self) -> InputTransform {
        self.input_transform
    }

    /// Returns reviewed refresh readiness, absent when the exact ABI is open.
    #[must_use]
    pub const fn refresh(&self) -> Option<ResolvedRefreshConfig> {
        self.refresh
    }

    /// Returns the live-reviewed stock-return action, if one has passed.
    #[must_use]
    pub const fn stock_repaint(&self) -> Option<StockRepaintMechanism> {
        self.stock_repaint
    }
}

/// A profile/report pair that cannot produce a safe runtime configuration.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeResolutionError {
    /// The profile object violated its own schema invariants.
    InvalidProfile(Vec<String>),
    /// The passive report violated its own schema invariants.
    InvalidReport(Vec<String>),
    /// A report with diagnostic warnings cannot enable a runtime path.
    ProbeWarningsPresent { count: usize },
    /// Evidence-pending profiles cannot be resolved for execution.
    ProfileEvidencePending,
    /// Identity, firmware, display, or required capabilities did not match.
    ProfileDidNotMatch(ProfileEvaluation),
    /// The parse-compatible historical profile has no runtime extension.
    RuntimePolicyMissing,
    /// No framebuffer matched both profile geometry and reviewed driver.
    FramebufferNotFound,
    /// More than one framebuffer matched the reviewed selection.
    FramebufferAmbiguous { count: usize },
    /// The selected framebuffer path was not a numbered `/dev/fb*` path.
    InvalidFramebufferPath,
    /// The selected framebuffer could not form a Gray8 memory layout.
    FramebufferLayout(FramebufferLayoutError),
    /// The selected framebuffer reported an unsupported rotation value.
    FramebufferRotation(TransformError),
    /// No input device had the exact reviewed kernel name.
    TouchNotFound,
    /// More than one input device had the exact reviewed kernel name.
    TouchAmbiguous { count: usize },
    /// The selected device lacked one reviewed absolute axis.
    TouchAxisNotFound { code: u16 },
    /// The selected device repeated one reviewed absolute axis.
    TouchAxisAmbiguous { code: u16, count: usize },
    /// A selected axis did not have a usable inclusive range.
    TouchAxisRange {
        code: u16,
        minimum: i32,
        maximum: i32,
    },
    /// The selected input path was not a numbered `/dev/input/event*` path.
    InvalidInputPath,
    /// The configured transform did not end at the physical visible extent.
    InputOutputExtentMismatch,
    /// The passive report did not include executable byte-order/class data.
    ExecutableAbiMissing,
    /// ELF class and libc input-event pointer width contradicted each other.
    ExecutableInputAbiMismatch {
        class: ElfClass,
        pointer_width_bits: u16,
    },
    /// The recorded raw input-event ABI was unsupported.
    InputEventAbi(InputEventDecodeError),
    /// The runtime refresh capability was internally empty.
    RefreshCapability(RefreshCapabilityError),
}

impl std::fmt::Display for RuntimeResolutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidProfile(errors) => write!(formatter, "invalid profile: {errors:?}"),
            Self::InvalidReport(errors) => write!(formatter, "invalid report: {errors:?}"),
            Self::ProbeWarningsPresent { count } => {
                write!(formatter, "probe report contains {count} warnings")
            }
            Self::ProfileEvidencePending => {
                formatter.write_str("profile evidence is still pending")
            }
            Self::ProfileDidNotMatch(evaluation) => {
                write!(formatter, "profile did not match report: {evaluation:?}")
            }
            Self::RuntimePolicyMissing => formatter.write_str("runtime profile policy is missing"),
            Self::FramebufferNotFound => formatter.write_str("reviewed framebuffer was not found"),
            Self::FramebufferAmbiguous { count } => {
                write!(formatter, "reviewed framebuffer matched {count} devices")
            }
            Self::InvalidFramebufferPath => {
                formatter.write_str("selected framebuffer path is invalid")
            }
            Self::FramebufferLayout(error) => {
                write!(formatter, "framebuffer layout is invalid: {error}")
            }
            Self::FramebufferRotation(error) => {
                write!(formatter, "framebuffer rotation is invalid: {error}")
            }
            Self::TouchNotFound => formatter.write_str("reviewed touch device was not found"),
            Self::TouchAmbiguous { count } => {
                write!(formatter, "reviewed touch name matched {count} devices")
            }
            Self::TouchAxisNotFound { code } => {
                write!(formatter, "reviewed touch axis {code} was not found")
            }
            Self::TouchAxisAmbiguous { code, count } => {
                write!(
                    formatter,
                    "reviewed touch axis {code} appeared {count} times"
                )
            }
            Self::TouchAxisRange {
                code,
                minimum,
                maximum,
            } => write!(
                formatter,
                "touch axis {code} has invalid range {minimum}..={maximum}"
            ),
            Self::InvalidInputPath => formatter.write_str("selected input path is invalid"),
            Self::InputOutputExtentMismatch => {
                formatter.write_str("input transform output does not match visible display")
            }
            Self::ExecutableAbiMissing => {
                formatter.write_str("executable ABI is missing from the report")
            }
            Self::ExecutableInputAbiMismatch {
                class,
                pointer_width_bits,
            } => write!(
                formatter,
                "executable class {class:?} contradicts {pointer_width_bits}-bit input ABI"
            ),
            Self::InputEventAbi(error) => write!(formatter, "input event ABI is invalid: {error}"),
            Self::RefreshCapability(error) => {
                write!(formatter, "refresh capability is invalid: {error}")
            }
        }
    }
}

impl std::error::Error for RuntimeResolutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::FramebufferLayout(error) => Some(error),
            Self::FramebufferRotation(error) => Some(error),
            Self::InputEventAbi(error) => Some(error),
            Self::RefreshCapability(error) => Some(error),
            Self::InvalidProfile(_)
            | Self::InvalidReport(_)
            | Self::ProbeWarningsPresent { .. }
            | Self::ProfileEvidencePending
            | Self::ProfileDidNotMatch(_)
            | Self::RuntimePolicyMissing
            | Self::FramebufferNotFound
            | Self::FramebufferAmbiguous { .. }
            | Self::InvalidFramebufferPath
            | Self::TouchNotFound
            | Self::TouchAmbiguous { .. }
            | Self::TouchAxisNotFound { .. }
            | Self::TouchAxisAmbiguous { .. }
            | Self::TouchAxisRange { .. }
            | Self::InvalidInputPath
            | Self::InputOutputExtentMismatch
            | Self::ExecutableAbiMissing
            | Self::ExecutableInputAbiMismatch { .. } => None,
        }
    }
}

fn select_framebuffer<'a>(
    profile: &DeviceProfile,
    report: &'a ProbeReport,
    runtime: &RuntimeSelection,
) -> Result<&'a crate::FramebufferCapability, RuntimeResolutionError> {
    let mut matches = report.framebuffers.iter().filter(|framebuffer| {
        framebuffer.driver_id == runtime.framebuffer.driver_id
            && framebuffer.visible_width == profile.display.width
            && framebuffer.visible_height == profile.display.height
            && framebuffer.bits_per_pixel == profile.display.bits_per_pixel
            && profile
                .display
                .pixel_layout
                .is_none_or(|layout| framebuffer.pixel_layout == layout)
            && profile
                .display
                .rotation
                .is_none_or(|rotation| framebuffer.rotation == rotation)
    });
    let Some(framebuffer) = matches.next() else {
        return Err(RuntimeResolutionError::FramebufferNotFound);
    };
    let Some(_) = matches.next() else {
        return Ok(framebuffer);
    };
    Err(RuntimeResolutionError::FramebufferAmbiguous {
        count: 2 + matches.count(),
    })
}

fn select_touch<'a>(
    report: &'a ProbeReport,
    runtime: &RuntimeSelection,
) -> Result<&'a InputDeviceCapability, RuntimeResolutionError> {
    let mut matches = report
        .inputs
        .iter()
        .filter(|input| input.name.as_deref() == Some(runtime.touch.name.as_str()));
    let Some(input) = matches.next() else {
        return Err(RuntimeResolutionError::TouchNotFound);
    };
    let Some(_) = matches.next() else {
        return Ok(input);
    };
    Err(RuntimeResolutionError::TouchAmbiguous {
        count: 2 + matches.count(),
    })
}

fn select_axis(
    input: &InputDeviceCapability,
    code: u16,
) -> Result<AxisRange, RuntimeResolutionError> {
    let mut matches = input.axes.iter().filter(|axis| axis.code == code);
    let Some(axis) = matches.next() else {
        return Err(RuntimeResolutionError::TouchAxisNotFound { code });
    };
    if matches.next().is_some() {
        return Err(RuntimeResolutionError::TouchAxisAmbiguous {
            code,
            count: 2 + matches.count(),
        });
    }
    AxisRange::try_new(axis.minimum, axis.maximum).map_err(|_| {
        RuntimeResolutionError::TouchAxisRange {
            code,
            minimum: axis.minimum,
            maximum: axis.maximum,
        }
    })
}

fn extent_before_rotation(visible: DisplayExtent, rotation: QuarterTurn) -> DisplayExtent {
    match rotation {
        QuarterTurn::Upright | QuarterTurn::UpsideDown => visible,
        QuarterTurn::Clockwise | QuarterTurn::CounterClockwise => {
            DisplayExtent::new(visible.non_zero_height(), visible.non_zero_width())
        }
    }
}

fn resolve_refresh(
    selection: RuntimeRefreshSelection,
) -> Result<ResolvedRefreshConfig, RuntimeResolutionError> {
    let capabilities = RefreshCapabilities::try_new(
        selection.partial,
        selection.full,
        selection.maximum_completion_wait_millis,
    )
    .map_err(RuntimeResolutionError::RefreshCapability)?;
    Ok(ResolvedRefreshConfig {
        update_abi: selection.update_abi,
        capabilities,
    })
}

fn valid_numbered_path(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        !suffix.is_empty() && suffix.chars().all(|character| character.is_ascii_digit())
    })
}
