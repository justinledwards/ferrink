//! Persistent, bounded startup-crash accounting for boot recovery.

use std::fmt::{Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::num::NonZeroUsize;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

const STATE_HEADER: &str = "ferrink-crash-budget-v1";
const FAILURE_PREFIX: &str = "unproven_start=";
const MAX_STATE_BYTES: u64 = 1024;
const MAX_RECORDED_STARTS: usize = 32;
const TEMPORARY_ATTEMPTS: usize = 8;

/// Bounded startup-window policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrashBudgetPolicy {
    maximum_unproven_starts: NonZeroUsize,
    window: Duration,
}

impl CrashBudgetPolicy {
    /// Creates a policy with a nonzero count and duration.
    ///
    /// # Errors
    ///
    /// Returns [`CrashBudgetError::InvalidPolicy`] when the count exceeds the
    /// parser bound or the window is zero or cannot be represented in seconds.
    pub fn new(
        maximum_unproven_starts: NonZeroUsize,
        window: Duration,
    ) -> Result<Self, CrashBudgetError> {
        if maximum_unproven_starts.get() > MAX_RECORDED_STARTS
            || window.is_zero()
            || window.as_secs() == 0
        {
            return Err(CrashBudgetError::InvalidPolicy);
        }
        Ok(Self {
            maximum_unproven_starts,
            window,
        })
    }

    /// Returns the number of retained unproven starts that selects fallback.
    #[must_use]
    pub const fn maximum_unproven_starts(self) -> NonZeroUsize {
        self.maximum_unproven_starts
    }

    /// Returns the rolling failure window.
    #[must_use]
    pub const fn window(self) -> Duration {
        self.window
    }
}

/// Strict version-one crash history.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CrashBudgetHistory {
    unproven_starts: Vec<u64>,
}

impl CrashBudgetHistory {
    /// Parses the bounded on-disk representation.
    ///
    /// # Errors
    ///
    /// Rejects unknown headers or fields, malformed timestamps, excessive
    /// entries, and timestamps that are not monotonically ordered.
    pub fn parse(input: &str) -> Result<Self, CrashBudgetError> {
        if input.len() as u64 > MAX_STATE_BYTES {
            return Err(CrashBudgetError::StateTooLarge);
        }
        let mut lines = input.lines();
        if lines.next() != Some(STATE_HEADER) {
            return Err(CrashBudgetError::InvalidHeader);
        }

        let mut unproven_starts = Vec::new();
        for line in lines {
            let value = line
                .strip_prefix(FAILURE_PREFIX)
                .ok_or(CrashBudgetError::UnknownField)?;
            let timestamp = value
                .parse::<u64>()
                .map_err(|_| CrashBudgetError::InvalidTimestamp)?;
            if unproven_starts.last().is_some_and(|last| *last > timestamp) {
                return Err(CrashBudgetError::UnorderedTimestamps);
            }
            if unproven_starts.len() == MAX_RECORDED_STARTS {
                return Err(CrashBudgetError::TooManyEntries);
            }
            unproven_starts.push(timestamp);
        }
        Ok(Self { unproven_starts })
    }

    /// Serializes the strict version-one representation.
    #[must_use]
    pub fn encode(&self) -> String {
        let mut output = String::from(STATE_HEADER);
        output.push('\n');
        for timestamp in &self.unproven_starts {
            output.push_str(FAILURE_PREFIX);
            output.push_str(&timestamp.to_string());
            output.push('\n');
        }
        output
    }

    /// Records a new unproven start or selects fallback without mutation.
    ///
    /// # Errors
    ///
    /// Fails closed when the wall clock moved behind a retained timestamp or
    /// arithmetic for the retry boundary overflows.
    pub fn begin_attempt(
        &mut self,
        policy: CrashBudgetPolicy,
        now_unix_seconds: u64,
    ) -> Result<CrashBudgetDecision, CrashBudgetError> {
        if self
            .unproven_starts
            .last()
            .is_some_and(|last| *last > now_unix_seconds)
        {
            return Err(CrashBudgetError::ClockRegressed);
        }

        let window_seconds = policy.window.as_secs();
        self.unproven_starts
            .retain(|timestamp| now_unix_seconds.saturating_sub(*timestamp) < window_seconds);

        if self.unproven_starts.len() >= policy.maximum_unproven_starts.get() {
            let retry_at_unix_seconds = self.unproven_starts[0]
                .checked_add(window_seconds)
                .ok_or(CrashBudgetError::TimestampOverflow)?;
            return Ok(CrashBudgetDecision::Fallback {
                retry_at_unix_seconds,
            });
        }

        self.unproven_starts.push(now_unix_seconds);
        Ok(CrashBudgetDecision::Permit)
    }

    /// Clears all unproven starts after stable supervision or a clean handoff.
    pub fn mark_stable(&mut self) {
        self.unproven_starts.clear();
    }

    /// Returns the retained unproven-start count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.unproven_starts.len()
    }

    /// Returns whether no unproven start is retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.unproven_starts.is_empty()
    }
}

/// Decision made before any userstore or foreground dependency is touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashBudgetDecision {
    /// This attempt was recorded and may proceed.
    Permit,
    /// Stock fallback must be selected until the oldest record expires.
    Fallback {
        /// Earliest wall-clock second at which another attempt may be made.
        retry_at_unix_seconds: u64,
    },
}

/// Strict history or policy failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashBudgetError {
    /// Policy count or duration is outside the bounded domain.
    InvalidPolicy,
    /// State exceeded its fixed byte budget.
    StateTooLarge,
    /// State did not begin with the exact version header.
    InvalidHeader,
    /// State contained a field not defined by version one.
    UnknownField,
    /// A timestamp was not an unsigned integer.
    InvalidTimestamp,
    /// More timestamps were supplied than the fixed parser bound.
    TooManyEntries,
    /// Timestamps were not monotonically ordered.
    UnorderedTimestamps,
    /// The wall clock moved behind retained durable state.
    ClockRegressed,
    /// A retry boundary could not be represented.
    TimestampOverflow,
}

impl Display for CrashBudgetError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::InvalidPolicy => "invalid crash-budget policy",
            Self::StateTooLarge => "crash-budget state is too large",
            Self::InvalidHeader => "invalid crash-budget state header",
            Self::UnknownField => "unknown crash-budget state field",
            Self::InvalidTimestamp => "invalid crash-budget timestamp",
            Self::TooManyEntries => "too many crash-budget entries",
            Self::UnorderedTimestamps => "crash-budget timestamps are unordered",
            Self::ClockRegressed => "clock regressed behind crash-budget state",
            Self::TimestampOverflow => "crash-budget timestamp overflowed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for CrashBudgetError {}

/// One permitted persistent attempt.
#[derive(Debug)]
pub struct PersistentCrashBudget {
    path: PathBuf,
    history: CrashBudgetHistory,
}

impl PersistentCrashBudget {
    /// Atomically records one attempt before the caller touches userstore or
    /// foreground resources.
    ///
    /// # Errors
    ///
    /// Fails closed for unsafe paths, malformed state, or any persistence
    /// failure.
    pub fn begin(
        path: &Path,
        policy: CrashBudgetPolicy,
        now_unix_seconds: u64,
    ) -> Result<PersistentCrashBudgetStart, PersistentCrashBudgetError> {
        let path = validate_state_path(path)?;
        let mut history = read_history(&path)?;
        match history.begin_attempt(policy, now_unix_seconds)? {
            CrashBudgetDecision::Permit => {
                write_history(&path, &history)?;
                Ok(PersistentCrashBudgetStart::Permit(Self { path, history }))
            }
            CrashBudgetDecision::Fallback {
                retry_at_unix_seconds,
            } => Ok(PersistentCrashBudgetStart::Fallback {
                retry_at_unix_seconds,
            }),
        }
    }

    /// Clears the durable attempt after the guardian is proven stable.
    ///
    /// # Errors
    ///
    /// Returns a persistence error and leaves the in-memory attempt retained
    /// when the atomic write cannot complete.
    pub fn mark_stable(&mut self) -> Result<(), PersistentCrashBudgetError> {
        let mut stable = self.history.clone();
        stable.mark_stable();
        write_history(&self.path, &stable)?;
        self.history = stable;
        Ok(())
    }

    /// Returns the exact durable state path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Persistent decision made before a guarded launch.
#[derive(Debug)]
pub enum PersistentCrashBudgetStart {
    /// Attempt was durably recorded.
    Permit(PersistentCrashBudget),
    /// Budget is exhausted; caller must select stock without launching Ferrink.
    Fallback {
        /// Earliest wall-clock second at which another attempt may be made.
        retry_at_unix_seconds: u64,
    },
}

/// Durable-state validation or I/O failure.
#[derive(Debug)]
pub enum PersistentCrashBudgetError {
    /// State path is not an exact absolute regular-file destination beneath an
    /// existing canonical directory.
    UnsafePath(&'static str),
    /// Strict state parsing or policy evaluation failed.
    History(CrashBudgetError),
    /// A bounded filesystem operation failed.
    Io(std::io::Error),
}

impl Display for PersistentCrashBudgetError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsafePath(reason) => write!(formatter, "unsafe crash-budget path: {reason}"),
            Self::History(error) => Display::fmt(error, formatter),
            Self::Io(error) => write!(formatter, "crash-budget I/O failed: {error}"),
        }
    }
}

impl std::error::Error for PersistentCrashBudgetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::History(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::UnsafePath(_) => None,
        }
    }
}

impl From<CrashBudgetError> for PersistentCrashBudgetError {
    fn from(error: CrashBudgetError) -> Self {
        Self::History(error)
    }
}

impl From<std::io::Error> for PersistentCrashBudgetError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

fn validate_state_path(path: &Path) -> Result<PathBuf, PersistentCrashBudgetError> {
    if !path.is_absolute() {
        return Err(PersistentCrashBudgetError::UnsafePath(
            "path must be absolute",
        ));
    }
    let parent = path.parent().ok_or(PersistentCrashBudgetError::UnsafePath(
        "path must have a parent",
    ))?;
    let canonical_parent = parent.canonicalize()?;
    if canonical_parent != parent {
        return Err(PersistentCrashBudgetError::UnsafePath(
            "parent must be canonical and must not traverse a symlink",
        ));
    }
    let filename = path
        .file_name()
        .ok_or(PersistentCrashBudgetError::UnsafePath(
            "path must name a file",
        ))?;
    let validated = canonical_parent.join(filename);
    if let Ok(metadata) = path.symlink_metadata() {
        if !metadata.file_type().is_file() {
            return Err(PersistentCrashBudgetError::UnsafePath(
                "existing state must be a regular file",
            ));
        }
        if path.canonicalize()? != validated {
            return Err(PersistentCrashBudgetError::UnsafePath(
                "existing state must not be a symlink",
            ));
        }
    }
    Ok(validated)
}

fn read_history(path: &Path) -> Result<CrashBudgetHistory, PersistentCrashBudgetError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CrashBudgetHistory::default());
        }
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    if metadata.len() > MAX_STATE_BYTES {
        return Err(CrashBudgetError::StateTooLarge.into());
    }
    let mut input = String::new();
    file.take(MAX_STATE_BYTES + 1).read_to_string(&mut input)?;
    CrashBudgetHistory::parse(&input).map_err(Into::into)
}

fn write_history(
    path: &Path,
    history: &CrashBudgetHistory,
) -> Result<(), PersistentCrashBudgetError> {
    let parent = path.parent().ok_or(PersistentCrashBudgetError::UnsafePath(
        "path must have a parent",
    ))?;
    let filename = path.file_name().and_then(|value| value.to_str()).ok_or(
        PersistentCrashBudgetError::UnsafePath("filename must be Unicode"),
    )?;
    let encoded = history.encode();
    let mut last_collision = None;

    for suffix in 0..TEMPORARY_ATTEMPTS {
        let temporary = parent.join(format!(".{filename}.tmp.{}.{suffix}", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = match options.open(&temporary) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                last_collision = Some(error);
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        let result = (|| -> Result<(), std::io::Error> {
            file.write_all(encoded.as_bytes())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, path)?;
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return result.map_err(Into::into);
    }

    Err(last_collision
        .unwrap_or_else(|| std::io::Error::other("no crash-budget temporary name available"))
        .into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let base = std::env::temp_dir().canonicalize().unwrap();
            for _ in 0..TEMPORARY_ATTEMPTS {
                let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
                let path = base.join(format!("ferrink-{label}-{}-{nonce}", std::process::id()));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path.canonicalize().unwrap()),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => panic!("cannot create test directory: {error}"),
                }
            }
            panic!("cannot allocate a unique test directory");
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn policy() -> CrashBudgetPolicy {
        CrashBudgetPolicy::new(NonZeroUsize::new(3).unwrap(), Duration::from_secs(600)).unwrap()
    }

    #[test]
    fn fourth_rapid_start_selects_fallback_without_mutating_history() {
        let mut history = CrashBudgetHistory::default();
        for now in [100, 101, 102] {
            assert_eq!(
                history.begin_attempt(policy(), now).unwrap(),
                CrashBudgetDecision::Permit
            );
        }
        let encoded = history.encode();
        assert_eq!(
            history.begin_attempt(policy(), 103).unwrap(),
            CrashBudgetDecision::Fallback {
                retry_at_unix_seconds: 700
            }
        );
        assert_eq!(history.encode(), encoded);
    }

    #[test]
    fn expired_attempts_and_stable_runtime_restore_the_budget() {
        let mut history = CrashBudgetHistory::default();
        for now in [100, 101, 102] {
            assert_eq!(
                history.begin_attempt(policy(), now).unwrap(),
                CrashBudgetDecision::Permit
            );
        }
        assert_eq!(
            history.begin_attempt(policy(), 700).unwrap(),
            CrashBudgetDecision::Permit
        );
        assert_eq!(history.len(), 3);
        history.mark_stable();
        assert!(history.is_empty());
        assert_eq!(
            history.begin_attempt(policy(), 701).unwrap(),
            CrashBudgetDecision::Permit
        );
    }

    #[test]
    fn malformed_or_regressed_state_fails_closed() {
        for invalid in [
            "",
            "ferrink-crash-budget-v2\n",
            "ferrink-crash-budget-v1\nunknown=1\n",
            "ferrink-crash-budget-v1\nunproven_start=no\n",
            "ferrink-crash-budget-v1\nunproven_start=2\nunproven_start=1\n",
        ] {
            assert!(
                CrashBudgetHistory::parse(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
        let mut history =
            CrashBudgetHistory::parse("ferrink-crash-budget-v1\nunproven_start=200\n").unwrap();
        assert_eq!(
            history.begin_attempt(policy(), 199),
            Err(CrashBudgetError::ClockRegressed)
        );
    }

    #[test]
    fn persistent_attempt_is_atomic_and_stable_clear_is_durable() {
        let directory = TestDirectory::new("crash-budget-atomic");
        let path = directory.join("state-v1");

        let mut budget = match PersistentCrashBudget::begin(&path, policy(), 100).unwrap() {
            PersistentCrashBudgetStart::Permit(budget) => budget,
            PersistentCrashBudgetStart::Fallback { .. } => panic!("fresh budget denied"),
        };
        assert_eq!(
            CrashBudgetHistory::parse(&fs::read_to_string(&path).unwrap())
                .unwrap()
                .len(),
            1
        );
        budget.mark_stable().unwrap();
        assert!(
            CrashBudgetHistory::parse(&fs::read_to_string(&path).unwrap())
                .unwrap()
                .is_empty()
        );

        assert!(fs::read_dir(&directory.0).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp.")
        }));
    }

    #[test]
    fn persistent_budget_survives_repeated_stable_cycles_without_temp_debris() {
        let directory = TestDirectory::new("crash-budget-stress");
        let path = directory.join("state-v1");

        for now in 1..=256 {
            let mut budget = match PersistentCrashBudget::begin(&path, policy(), now).unwrap() {
                PersistentCrashBudgetStart::Permit(budget) => budget,
                PersistentCrashBudgetStart::Fallback { .. } => panic!("stable cycle denied"),
            };
            budget.mark_stable().unwrap();
        }

        assert!(
            CrashBudgetHistory::parse(&fs::read_to_string(path).unwrap())
                .unwrap()
                .is_empty()
        );
        assert!(fs::read_dir(&directory.0).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp.")
        }));
    }

    #[test]
    fn persistent_fallback_and_corrupt_state_never_overwrite_evidence() {
        let directory = TestDirectory::new("crash-budget-fail-closed");
        let path = directory.join("state-v1");

        for now in [100, 101, 102] {
            assert!(matches!(
                PersistentCrashBudget::begin(&path, policy(), now).unwrap(),
                PersistentCrashBudgetStart::Permit(_)
            ));
        }
        let exhausted = fs::read_to_string(&path).unwrap();
        assert!(matches!(
            PersistentCrashBudget::begin(&path, policy(), 103).unwrap(),
            PersistentCrashBudgetStart::Fallback {
                retry_at_unix_seconds: 700
            }
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), exhausted);

        fs::write(&path, "corrupt\n").unwrap();
        assert!(PersistentCrashBudget::begin(&path, policy(), 104).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "corrupt\n");
    }
}
