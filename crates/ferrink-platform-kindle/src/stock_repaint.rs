//! Host-testable stock-presentation repaint policy.

use std::num::NonZeroI32;
use std::time::Duration;

use ferrink_platform::{ResolvedRuntimeDevice, StockRepaintMechanism};

/// Single-use identifier for the first KOA3 stock-repaint card.
pub const KOA3_STOCK_REPAINT_CARD_ID: &str = "koa3-stock-repaint-v1";

const XREFRESH_EXECUTABLE: &str = "/usr/bin/xrefresh";
const XREFRESH_ARGUMENTS: &[&str] = &["-d", ":0.0"];
const XREFRESH_TIMEOUT: Duration = Duration::from_secs(5);

/// Immutable, argument-array command selected from a reviewed profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockRepaintCommand {
    mechanism: StockRepaintMechanism,
    executable: &'static str,
    arguments: &'static [&'static str],
    timeout: Duration,
}

impl StockRepaintCommand {
    fn for_mechanism(mechanism: StockRepaintMechanism) -> Self {
        match mechanism {
            StockRepaintMechanism::XrefreshDisplay0 => Self {
                mechanism,
                executable: XREFRESH_EXECUTABLE,
                arguments: XREFRESH_ARGUMENTS,
                timeout: XREFRESH_TIMEOUT,
            },
        }
    }

    /// Returns the typed repaint mechanism represented by this command.
    #[must_use]
    pub const fn mechanism(self) -> StockRepaintMechanism {
        self.mechanism
    }

    /// Returns the exact absolute executable path.
    #[must_use]
    pub const fn executable(self) -> &'static str {
        self.executable
    }

    /// Returns the fixed argument array. It is never interpreted by a shell.
    #[must_use]
    pub const fn arguments(self) -> &'static [&'static str] {
        self.arguments
    }

    /// Returns the strict child-process deadline.
    #[must_use]
    pub const fn timeout(self) -> Duration {
        self.timeout
    }
}

/// Bounded result from one exact repaint child process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockRepaintOutcome {
    /// The exact child exited successfully within the deadline.
    Succeeded,
    /// The exact child exited unsuccessfully without being retried.
    ExitedFailure { code: Option<i32> },
    /// The exact child was terminated at the deadline and reaped.
    TimedOut,
}

/// Process boundary used by [`StockRepaintCore`].
pub trait StockRepaintProcess {
    /// Executes the exact argument-array command once and enforces its timeout.
    ///
    /// # Errors
    ///
    /// Returns a bounded operation error after ensuring that no spawned child
    /// remains running.
    fn run_exact(
        &mut self,
        command: StockRepaintCommand,
    ) -> Result<StockRepaintOutcome, StockRepaintIoError>;
}

/// Host-testable stock-return policy selected by one resolved device profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockRepaintCore {
    command: StockRepaintCommand,
}

impl StockRepaintCore {
    /// Builds the one unreviewed KOA3 candidate used only by its first live
    /// stock-repaint card.
    ///
    /// This does not promote the candidate into the device profile. A
    /// successful, separately approved live card must be recorded before
    /// [`Self::try_from_runtime`] can select it for normal stock return.
    ///
    /// # Errors
    ///
    /// Returns [`StockRepaintCandidateError`] for another profile or when the
    /// KOA3 profile already contains a reviewed mechanism and the candidate
    /// card must not be repeated.
    pub fn koa3_card_candidate(
        device: &ResolvedRuntimeDevice,
    ) -> Result<Self, StockRepaintCandidateError> {
        if device.profile_id() != "reference-portrait" {
            return Err(StockRepaintCandidateError::WrongProfile);
        }
        if device.stock_repaint().is_some() {
            return Err(StockRepaintCandidateError::AlreadyReviewed);
        }
        Ok(Self {
            command: StockRepaintCommand::for_mechanism(StockRepaintMechanism::XrefreshDisplay0),
        })
    }

    /// Resolves the exact stock-repaint command from reviewed profile policy.
    ///
    /// # Errors
    ///
    /// Returns [`StockRepaintError::Unavailable`] until a live repaint card has
    /// promoted one exact mechanism into the profile.
    pub fn try_from_runtime(device: &ResolvedRuntimeDevice) -> Result<Self, StockRepaintError> {
        let mechanism = device
            .stock_repaint()
            .ok_or(StockRepaintError::Unavailable)?;
        Ok(Self {
            command: StockRepaintCommand::for_mechanism(mechanism),
        })
    }

    /// Returns the immutable command selected from the profile.
    #[must_use]
    pub const fn command(self) -> StockRepaintCommand {
        self.command
    }

    /// Executes the exact command once, with no shell, fallback, or retry.
    ///
    /// # Errors
    ///
    /// Returns [`StockRepaintError`] for process-boundary failures, non-zero
    /// exit, or timeout.
    pub fn repaint(self, process: &mut impl StockRepaintProcess) -> Result<(), StockRepaintError> {
        match process
            .run_exact(self.command)
            .map_err(StockRepaintError::Io)?
        {
            StockRepaintOutcome::Succeeded => Ok(()),
            StockRepaintOutcome::ExitedFailure { code } => {
                Err(StockRepaintError::ExitedFailure { code })
            }
            StockRepaintOutcome::TimedOut => Err(StockRepaintError::TimedOut {
                timeout: self.command.timeout,
            }),
        }
    }
}

/// A profile that cannot enter the one-time KOA3 repaint-candidate card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StockRepaintCandidateError {
    /// The candidate is specific to the recorded KOA3 profile.
    WrongProfile,
    /// A reviewed mechanism is already present, so the first card is consumed.
    AlreadyReviewed,
}

impl std::fmt::Display for StockRepaintCandidateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongProfile => formatter.write_str("stock repaint candidate requires KOA3"),
            Self::AlreadyReviewed => {
                formatter.write_str("stock repaint candidate card is already consumed")
            }
        }
    }
}

impl std::error::Error for StockRepaintCandidateError {}

/// Exact repaint process operation that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockRepaintOperation {
    /// The supposedly exact command differed from the reviewed constant.
    Verify,
    /// The child could not be spawned.
    Spawn,
    /// The child status could not be queried or reaped.
    Wait,
    /// A timed-out or failed child could not be terminated.
    Terminate,
}

/// Bounded process I/O failure with no command-line or environment capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockRepaintIoError {
    operation: StockRepaintOperation,
    errno: Option<NonZeroI32>,
}

impl StockRepaintIoError {
    /// Constructs a redacted process-boundary error.
    #[must_use]
    pub const fn new(operation: StockRepaintOperation, errno: Option<NonZeroI32>) -> Self {
        Self { operation, errno }
    }

    /// Returns the failed operation.
    #[must_use]
    pub const fn operation(self) -> StockRepaintOperation {
        self.operation
    }

    /// Returns a positive operating-system error number when available.
    #[must_use]
    pub const fn errno(self) -> Option<NonZeroI32> {
        self.errno
    }
}

impl std::fmt::Display for StockRepaintIoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "stock repaint {:?} failed{}",
            self.operation,
            ErrnoSuffix(self.errno)
        )
    }
}

impl std::error::Error for StockRepaintIoError {}

/// Failure while selecting or running the reviewed stock repaint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StockRepaintError {
    /// No stock-return mechanism has passed a live card for this profile.
    Unavailable,
    /// The process boundary failed while retaining cleanup ownership.
    Io(StockRepaintIoError),
    /// The exact child exited unsuccessfully.
    ExitedFailure { code: Option<i32> },
    /// The exact child exceeded its reviewed deadline and was terminated.
    TimedOut { timeout: Duration },
}

impl std::fmt::Display for StockRepaintError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => {
                formatter.write_str("no stock repaint mechanism has passed a live card")
            }
            Self::Io(error) => write!(formatter, "stock repaint I/O failed: {error}"),
            Self::ExitedFailure { code } => {
                write!(
                    formatter,
                    "stock repaint child exited unsuccessfully: {code:?}"
                )
            }
            Self::TimedOut { timeout } => write!(
                formatter,
                "stock repaint child exceeded {} ms deadline",
                timeout.as_millis()
            ),
        }
    }
}

impl std::error::Error for StockRepaintError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Unavailable | Self::ExitedFailure { .. } | Self::TimedOut { .. } => None,
        }
    }
}

struct ErrnoSuffix(Option<NonZeroI32>);

impl std::fmt::Display for ErrnoSuffix {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(errno) => write!(formatter, " with errno {}", errno.get()),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrink_platform::{DeviceProfile, ProbeReport};

    const KOA3_REPORT: &str =
        include_str!("../../ferrink-platform/tests/fixtures/probe-reference-portrait.json");
    const KOA3_PROFILE: &str = include_str!("../../../device-profiles/reference-portrait.toml");

    #[derive(Debug)]
    struct FakeProcess {
        calls: Vec<StockRepaintCommand>,
        result: Result<StockRepaintOutcome, StockRepaintIoError>,
    }

    impl StockRepaintProcess for FakeProcess {
        fn run_exact(
            &mut self,
            command: StockRepaintCommand,
        ) -> Result<StockRepaintOutcome, StockRepaintIoError> {
            self.calls.push(command);
            self.result
        }
    }

    fn runtime(with_repaint: bool) -> ResolvedRuntimeDevice {
        let mut profile = DeviceProfile::from_toml(KOA3_PROFILE).unwrap();
        profile.runtime.as_mut().unwrap().stock_repaint =
            with_repaint.then_some(StockRepaintMechanism::XrefreshDisplay0);
        let report = ProbeReport::from_json(KOA3_REPORT).unwrap();
        ResolvedRuntimeDevice::resolve(&profile, &report).unwrap()
    }

    #[test]
    fn profile_without_live_repaint_evidence_stays_closed() {
        assert_eq!(
            StockRepaintCore::try_from_runtime(&runtime(false)),
            Err(StockRepaintError::Unavailable)
        );
    }

    #[test]
    fn one_time_candidate_accepts_only_unpromoted_koa3() {
        let candidate = StockRepaintCore::koa3_card_candidate(&runtime(false)).unwrap();
        assert_eq!(
            candidate.command().mechanism(),
            StockRepaintMechanism::XrefreshDisplay0
        );
        assert_eq!(
            StockRepaintCore::koa3_card_candidate(&runtime(true)),
            Err(StockRepaintCandidateError::AlreadyReviewed)
        );

        let profile = DeviceProfile::from_toml(include_str!(
            "../../../device-profiles/reference-landscape.toml"
        ))
        .unwrap();
        let report = ProbeReport::from_json(include_str!(
            "../../ferrink-platform/tests/fixtures/probe-reference-landscape.json"
        ))
        .unwrap();
        let pw1 = ResolvedRuntimeDevice::resolve(&profile, &report).unwrap();
        assert_eq!(
            StockRepaintCore::koa3_card_candidate(&pw1),
            Err(StockRepaintCandidateError::WrongProfile)
        );
    }

    #[test]
    fn exact_argument_array_runs_once_without_fallback() {
        let core = StockRepaintCore::try_from_runtime(&runtime(true)).unwrap();
        let mut process = FakeProcess {
            calls: Vec::new(),
            result: Ok(StockRepaintOutcome::Succeeded),
        };

        core.repaint(&mut process).unwrap();

        assert_eq!(process.calls, [core.command()]);
        assert_eq!(core.command().executable(), "/usr/bin/xrefresh");
        assert_eq!(core.command().arguments(), ["-d", ":0.0"]);
        assert_eq!(core.command().timeout(), Duration::from_secs(5));
    }

    #[test]
    fn failure_timeout_and_io_error_are_never_retried() {
        let core = StockRepaintCore::try_from_runtime(&runtime(true)).unwrap();
        for (outcome, expected) in [
            (
                Ok(StockRepaintOutcome::ExitedFailure { code: Some(1) }),
                StockRepaintError::ExitedFailure { code: Some(1) },
            ),
            (
                Ok(StockRepaintOutcome::TimedOut),
                StockRepaintError::TimedOut {
                    timeout: Duration::from_secs(5),
                },
            ),
            (
                Err(StockRepaintIoError::new(
                    StockRepaintOperation::Spawn,
                    NonZeroI32::new(2),
                )),
                StockRepaintError::Io(StockRepaintIoError::new(
                    StockRepaintOperation::Spawn,
                    NonZeroI32::new(2),
                )),
            ),
        ] {
            let mut process = FakeProcess {
                calls: Vec::new(),
                result: outcome,
            };
            assert_eq!(core.repaint(&mut process), Err(expected));
            assert_eq!(process.calls.len(), 1);
        }
    }
}
