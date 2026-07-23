//! Host-testable foreground ownership and stock restoration policy.
//!
//! Default builds contain no process, LIPC, device, or command implementation.
//! An outer, separately reviewed adapter must implement [`ForegroundSystem`].

#![deny(unsafe_code)]

use std::collections::BTreeSet;
use std::num::{NonZeroU32, NonZeroU64};
use std::time::Duration;

mod crash_budget;

pub use crash_budget::{
    CrashBudgetDecision, CrashBudgetError, CrashBudgetHistory, CrashBudgetPolicy,
    PersistentCrashBudget, PersistentCrashBudgetError, PersistentCrashBudgetStart,
};

#[cfg(all(target_os = "linux", feature = "linux-kindle-foreground"))]
mod linux_kindle;

#[cfg(all(target_os = "linux", feature = "linux-kindle-foreground"))]
pub use linux_kindle::*;

/// Delay between pausing the last stock process and starting foreground work.
pub const FOREGROUND_QUIESCENCE: Duration = Duration::from_millis(300);

const MAX_IDENTITIES_PER_PROCESS: usize = 4;

/// Stock processes owned by the proven KOA3 foreground handoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StockProcess {
    /// The stock Awesome window manager.
    Awesome,
    /// The stock content-view manager.
    Cvm,
}

/// Stable identity used to avoid resuming a replacement process with a reused PID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProcessIdentity {
    process: StockProcess,
    pid: NonZeroU32,
    start_time_ticks: NonZeroU64,
}

impl ProcessIdentity {
    /// Creates one process identity captured before foreground acquisition.
    #[must_use]
    pub const fn new(process: StockProcess, pid: NonZeroU32, start_time_ticks: NonZeroU64) -> Self {
        Self {
            process,
            pid,
            start_time_ticks,
        }
    }

    /// Returns the expected stock process kind.
    #[must_use]
    pub const fn process(self) -> StockProcess {
        self.process
    }

    /// Returns the captured process identifier.
    #[must_use]
    pub const fn pid(self) -> NonZeroU32 {
        self.pid
    }

    /// Returns the captured Linux `/proc/<pid>/stat` start-time field.
    #[must_use]
    pub const fn start_time_ticks(self) -> NonZeroU64 {
        self.start_time_ticks
    }
}

/// Requested Pillow presentation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PillowState {
    /// Permit the stock Pillow presentation layer.
    Enabled,
    /// Suppress the stock Pillow presentation layer for foreground ownership.
    Disabled,
}

/// Exact signal transition requested for a captured stock identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessTransition {
    /// Pause the captured identity.
    Stop,
    /// Resume the same captured identity.
    Continue,
}

/// Side-effect seam required by the foreground ownership transaction.
///
/// Implementations must revalidate [`ProcessIdentity::start_time_ticks`] before
/// every signal and fail instead of signaling a reused PID.
pub trait ForegroundSystem {
    /// Bounded adapter failure.
    type Error;

    /// Reads the current `preventScreenSaver` value.
    fn prevent_screen_saver(&mut self) -> Result<bool, Self::Error>;

    /// Reads the current Pillow presentation state.
    fn pillow_state(&mut self) -> Result<PillowState, Self::Error>;

    /// Lists the exact running identities for one required stock process.
    fn running_processes(
        &mut self,
        process: StockProcess,
    ) -> Result<Vec<ProcessIdentity>, Self::Error>;

    /// Writes the screensaver inhibitor.
    fn set_prevent_screen_saver(&mut self, prevent: bool) -> Result<(), Self::Error>;

    /// Changes the known stock Pillow state.
    fn set_pillow(&mut self, state: PillowState) -> Result<(), Self::Error>;

    /// Sends an exact transition to one still-matching process identity.
    fn transition_process(
        &mut self,
        identity: ProcessIdentity,
        transition: ProcessTransition,
    ) -> Result<(), Self::Error>;

    /// Waits for the exact bounded quiescence interval.
    fn quiesce(&mut self, duration: Duration) -> Result<(), Self::Error>;

    /// Performs the separately reviewed stock repaint after restoration.
    fn repaint_stock(&mut self) -> Result<(), Self::Error>;
}

/// One stage in foreground acquisition or restoration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForegroundStage {
    /// Read the original screensaver inhibitor.
    InspectPreventScreenSaver,
    /// Read the original Pillow state.
    InspectPillow,
    /// Enumerate one required stock process.
    InspectProcesses(StockProcess),
    /// Write the screensaver inhibitor.
    SetPreventScreenSaver(bool),
    /// Change the Pillow state.
    SetPillow(PillowState),
    /// Pause or resume one exact identity.
    TransitionProcess(ProcessIdentity, ProcessTransition),
    /// Wait for the stock stack to quiesce.
    Quiesce,
    /// Repaint stock after restoration.
    RepaintStock,
}

/// An adapter failure tied to the exact attempted stage.
#[derive(Debug, PartialEq, Eq)]
pub struct StageFailure<E> {
    /// Stage that failed.
    pub stage: ForegroundStage,
    /// Adapter failure.
    pub error: E,
}

/// Fail-closed process-set validation before the first mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForegroundPolicyError {
    /// A required stock process was absent.
    MissingProcess(StockProcess),
    /// More identities were returned than the bounded contract permits.
    TooManyProcesses(StockProcess),
    /// The adapter returned an identity for the wrong requested process.
    ProcessKindMismatch {
        /// Requested process kind.
        expected: StockProcess,
        /// Identity's reported process kind.
        actual: StockProcess,
    },
    /// The same identity appeared more than once.
    DuplicateIdentity(ProcessIdentity),
}

/// Operational acquisition failure plus every best-effort rollback failure.
#[derive(Debug, PartialEq, Eq)]
pub struct ForegroundOperationError<E> {
    /// First acquisition operation that failed.
    pub cause: StageFailure<E>,
    /// Failures encountered while reversing already completed mutations.
    pub rollback_failures: Vec<StageFailure<E>>,
}

/// Failure to acquire foreground ownership.
#[derive(Debug, PartialEq, Eq)]
pub enum ForegroundAcquireError<E> {
    /// Required process identities did not pass policy before mutation.
    Policy(ForegroundPolicyError),
    /// Inspection or mutation failed; completed mutations were rolled back.
    Operation(ForegroundOperationError<E>),
}

/// Fail-closed validation for the measured KOA3 early-boot process state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EarlyBootPolicyError {
    /// Pillow was not confirmed disabled by the boot integration.
    PillowNotDisabled,
    /// The required early Awesome process set was invalid.
    InvalidAwesome(ForegroundPolicyError),
    /// A stock process that must not exist yet was already running.
    StockProcessRunning(StockProcess),
}

/// Failure to acquire foreground ownership while the stock UI is starting.
#[derive(Debug, PartialEq, Eq)]
pub enum EarlyBootAcquireError<E> {
    /// The observed boot state contradicted the measured early-boot contract.
    Policy(EarlyBootPolicyError),
    /// Inspection or mutation failed with bounded rollback evidence.
    Operation(ForegroundOperationError<E>),
}

/// Restoration failure after every cleanup stage was attempted once.
#[derive(Debug, PartialEq, Eq)]
pub struct ForegroundRestoreError<E> {
    /// Cleanup failures in attempted order.
    pub failures: Vec<StageFailure<E>>,
}

#[derive(Debug)]
struct AcquiredState {
    original_prevent_screen_saver: bool,
    original_pillow: PillowState,
    prevent_screen_saver_changed: bool,
    pillow_changed: bool,
    stopped_awesome: Vec<ProcessIdentity>,
    stopped_cvm: Vec<ProcessIdentity>,
}

impl Default for AcquiredState {
    fn default() -> Self {
        Self {
            original_prevent_screen_saver: false,
            original_pillow: PillowState::Enabled,
            prevent_screen_saver_changed: false,
            pillow_changed: false,
            stopped_awesome: Vec::new(),
            stopped_cvm: Vec::new(),
        }
    }
}

/// Explicit foreground lease that must be restored by the outer supervisor.
///
/// Drop intentionally performs no process signal, LIPC write, or repaint.
#[derive(Debug)]
#[must_use = "foreground ownership must be explicitly restored"]
pub struct ForegroundLease {
    state: AcquiredState,
}

/// Foreground ownership for the measured KOA3 partial stock boot.
///
/// Awesome exists before the vendor `lab126_gui` job starts, while Pillow and
/// CVM remain absent. This lease owns the screensaver inhibitor and the exact
/// Awesome identities it pauses. It never changes Pillow or repaints stock.
/// Drop performs no cleanup; the outer supervisor must call [`Self::restore`].
#[derive(Debug)]
#[must_use = "early-boot foreground ownership must be explicitly restored"]
pub struct EarlyBootForegroundLease {
    original_prevent_screen_saver: bool,
    prevent_screen_saver_changed: bool,
    stopped_awesome: Vec<ProcessIdentity>,
}

impl EarlyBootForegroundLease {
    /// Returns how many exact early Awesome identities Ferrink paused.
    #[must_use]
    pub fn paused_process_count(&self) -> usize {
        self.stopped_awesome.len()
    }

    /// Resumes the captured Awesome identities and restores the inhibitor.
    ///
    /// # Errors
    ///
    /// Returns the exact failed cleanup stage.
    pub fn restore<S: ForegroundSystem>(
        mut self,
        system: &mut S,
    ) -> Result<(), ForegroundRestoreError<S::Error>> {
        let failures = restore_early_boot_state(
            system,
            self.original_prevent_screen_saver,
            &mut self.prevent_screen_saver_changed,
            &mut self.stopped_awesome,
        );
        if failures.is_empty() {
            Ok(())
        } else {
            Err(ForegroundRestoreError { failures })
        }
    }
}

impl ForegroundLease {
    /// Returns how many exact process identities Ferrink paused.
    #[must_use]
    pub fn paused_process_count(&self) -> usize {
        self.state.stopped_awesome.len() + self.state.stopped_cvm.len()
    }

    /// Restores only mutations recorded by this lease, then repaints stock.
    ///
    /// Every cleanup stage is attempted once even after an earlier failure.
    ///
    /// # Errors
    ///
    /// Returns all bounded cleanup failures in attempted order.
    pub fn restore<S: ForegroundSystem>(
        mut self,
        system: &mut S,
    ) -> Result<(), ForegroundRestoreError<S::Error>> {
        let failures = restore_state(system, &mut self.state);
        if failures.is_empty() {
            Ok(())
        } else {
            Err(ForegroundRestoreError { failures })
        }
    }
}

/// Acquires the proven KOA3 foreground ownership boundary transactionally.
///
/// All process identities, the original screensaver-inhibitor value, and the
/// original Pillow state are captured before the first mutation. Restoration
/// intent is recorded before each external mutation so even a reported partial
/// failure takes the inverse path and performs the stock repaint once.
///
/// # Errors
///
/// Returns a policy failure before mutation, or an operational failure with
/// every rollback failure retained.
pub fn acquire_foreground<S: ForegroundSystem>(
    system: &mut S,
) -> Result<ForegroundLease, ForegroundAcquireError<S::Error>> {
    let original_prevent_screen_saver = system.prevent_screen_saver().map_err(|error| {
        operation_error(
            ForegroundStage::InspectPreventScreenSaver,
            error,
            Vec::new(),
        )
    })?;
    let original_pillow = system
        .pillow_state()
        .map_err(|error| operation_error(ForegroundStage::InspectPillow, error, Vec::new()))?;
    let awesome = inspect_processes(system, StockProcess::Awesome)?;
    let cvm = inspect_processes(system, StockProcess::Cvm)?;
    validate_processes(StockProcess::Awesome, &awesome).map_err(ForegroundAcquireError::Policy)?;
    validate_processes(StockProcess::Cvm, &cvm).map_err(ForegroundAcquireError::Policy)?;
    validate_unique_identities(awesome.iter().chain(cvm.iter()).copied())
        .map_err(ForegroundAcquireError::Policy)?;

    let mut state = AcquiredState {
        original_prevent_screen_saver,
        original_pillow,
        ..AcquiredState::default()
    };

    if !original_prevent_screen_saver {
        state.prevent_screen_saver_changed = true;
        mutate_or_rollback(
            system,
            &mut state,
            ForegroundStage::SetPreventScreenSaver(true),
            |system| system.set_prevent_screen_saver(true),
        )?;
    }

    if original_pillow != PillowState::Disabled {
        state.pillow_changed = true;
        mutate_or_rollback(
            system,
            &mut state,
            ForegroundStage::SetPillow(PillowState::Disabled),
            |system| system.set_pillow(PillowState::Disabled),
        )?;
    }

    for identity in awesome {
        state.stopped_awesome.push(identity);
        mutate_or_rollback(
            system,
            &mut state,
            ForegroundStage::TransitionProcess(identity, ProcessTransition::Stop),
            |system| system.transition_process(identity, ProcessTransition::Stop),
        )?;
    }
    for identity in cvm {
        state.stopped_cvm.push(identity);
        mutate_or_rollback(
            system,
            &mut state,
            ForegroundStage::TransitionProcess(identity, ProcessTransition::Stop),
            |system| system.transition_process(identity, ProcessTransition::Stop),
        )?;
    }

    mutate_or_rollback(system, &mut state, ForegroundStage::Quiesce, |system| {
        system.quiesce(FOREGROUND_QUIESCENCE)
    })?;

    Ok(ForegroundLease { state })
}

/// Acquires foreground ownership during the measured KOA3 partial stock boot.
///
/// Pillow must be independently confirmed disabled, Awesome must already be
/// running, and CVM must remain absent. These checks complete before enabling
/// the screensaver inhibitor and pausing the captured Awesome identities.
///
/// # Errors
///
/// Returns a policy failure for contradictory stock state, or an operational
/// failure with rollback evidence for an inspection or mutation error.
pub fn acquire_early_boot_foreground<S: ForegroundSystem>(
    system: &mut S,
) -> Result<EarlyBootForegroundLease, EarlyBootAcquireError<S::Error>> {
    let original_prevent_screen_saver = system.prevent_screen_saver().map_err(|error| {
        early_boot_operation_error(
            ForegroundStage::InspectPreventScreenSaver,
            error,
            Vec::new(),
        )
    })?;
    let pillow = system.pillow_state().map_err(|error| {
        early_boot_operation_error(ForegroundStage::InspectPillow, error, Vec::new())
    })?;
    let awesome = inspect_early_boot_processes(system, StockProcess::Awesome)?;
    let cvm = inspect_early_boot_processes(system, StockProcess::Cvm)?;

    if pillow != PillowState::Disabled {
        return Err(EarlyBootAcquireError::Policy(
            EarlyBootPolicyError::PillowNotDisabled,
        ));
    }
    if !cvm.is_empty() {
        return Err(EarlyBootAcquireError::Policy(
            EarlyBootPolicyError::StockProcessRunning(StockProcess::Cvm),
        ));
    }
    validate_processes(StockProcess::Awesome, &awesome).map_err(|error| {
        EarlyBootAcquireError::Policy(EarlyBootPolicyError::InvalidAwesome(error))
    })?;
    validate_unique_identities(awesome.iter().copied()).map_err(|error| {
        EarlyBootAcquireError::Policy(EarlyBootPolicyError::InvalidAwesome(error))
    })?;

    let mut lease = EarlyBootForegroundLease {
        original_prevent_screen_saver,
        prevent_screen_saver_changed: false,
        stopped_awesome: Vec::new(),
    };
    if !original_prevent_screen_saver {
        lease.prevent_screen_saver_changed = true;
        early_boot_mutate_or_rollback(
            system,
            &mut lease,
            ForegroundStage::SetPreventScreenSaver(true),
            |system| system.set_prevent_screen_saver(true),
        )?;
    }
    for identity in awesome {
        lease.stopped_awesome.push(identity);
        early_boot_mutate_or_rollback(
            system,
            &mut lease,
            ForegroundStage::TransitionProcess(identity, ProcessTransition::Stop),
            |system| system.transition_process(identity, ProcessTransition::Stop),
        )?;
    }
    early_boot_mutate_or_rollback(system, &mut lease, ForegroundStage::Quiesce, |system| {
        system.quiesce(FOREGROUND_QUIESCENCE)
    })?;
    Ok(lease)
}

fn inspect_early_boot_processes<S: ForegroundSystem>(
    system: &mut S,
    process: StockProcess,
) -> Result<Vec<ProcessIdentity>, EarlyBootAcquireError<S::Error>> {
    system.running_processes(process).map_err(|error| {
        early_boot_operation_error(
            ForegroundStage::InspectProcesses(process),
            error,
            Vec::new(),
        )
    })
}

fn early_boot_operation_error<E>(
    stage: ForegroundStage,
    error: E,
    rollback_failures: Vec<StageFailure<E>>,
) -> EarlyBootAcquireError<E> {
    EarlyBootAcquireError::Operation(ForegroundOperationError {
        cause: StageFailure { stage, error },
        rollback_failures,
    })
}

fn early_boot_mutate_or_rollback<S: ForegroundSystem>(
    system: &mut S,
    lease: &mut EarlyBootForegroundLease,
    stage: ForegroundStage,
    mutation: impl FnOnce(&mut S) -> Result<(), S::Error>,
) -> Result<(), EarlyBootAcquireError<S::Error>> {
    if let Err(error) = mutation(system) {
        let rollback_failures = restore_early_boot_state(
            system,
            lease.original_prevent_screen_saver,
            &mut lease.prevent_screen_saver_changed,
            &mut lease.stopped_awesome,
        );
        return Err(early_boot_operation_error(stage, error, rollback_failures));
    }
    Ok(())
}

fn restore_early_boot_state<S: ForegroundSystem>(
    system: &mut S,
    original_prevent_screen_saver: bool,
    changed: &mut bool,
    stopped_awesome: &mut Vec<ProcessIdentity>,
) -> Vec<StageFailure<S::Error>> {
    let mut failures = Vec::new();
    while let Some(identity) = stopped_awesome.pop() {
        attempt_cleanup(
            &mut failures,
            ForegroundStage::TransitionProcess(identity, ProcessTransition::Continue),
            system.transition_process(identity, ProcessTransition::Continue),
        );
    }
    if *changed {
        *changed = false;
        attempt_cleanup(
            &mut failures,
            ForegroundStage::SetPreventScreenSaver(original_prevent_screen_saver),
            system.set_prevent_screen_saver(original_prevent_screen_saver),
        );
    }
    failures
}

fn inspect_processes<S: ForegroundSystem>(
    system: &mut S,
    process: StockProcess,
) -> Result<Vec<ProcessIdentity>, ForegroundAcquireError<S::Error>> {
    system.running_processes(process).map_err(|error| {
        operation_error(
            ForegroundStage::InspectProcesses(process),
            error,
            Vec::new(),
        )
    })
}

fn validate_processes(
    expected: StockProcess,
    identities: &[ProcessIdentity],
) -> Result<(), ForegroundPolicyError> {
    if identities.is_empty() {
        return Err(ForegroundPolicyError::MissingProcess(expected));
    }
    if identities.len() > MAX_IDENTITIES_PER_PROCESS {
        return Err(ForegroundPolicyError::TooManyProcesses(expected));
    }
    for identity in identities {
        if identity.process() != expected {
            return Err(ForegroundPolicyError::ProcessKindMismatch {
                expected,
                actual: identity.process(),
            });
        }
    }
    Ok(())
}

fn validate_unique_identities(
    identities: impl IntoIterator<Item = ProcessIdentity>,
) -> Result<(), ForegroundPolicyError> {
    let mut seen = BTreeSet::new();
    for identity in identities {
        if !seen.insert((identity.pid(), identity.start_time_ticks())) {
            return Err(ForegroundPolicyError::DuplicateIdentity(identity));
        }
    }
    Ok(())
}

fn mutate_or_rollback<S: ForegroundSystem>(
    system: &mut S,
    state: &mut AcquiredState,
    stage: ForegroundStage,
    mutation: impl FnOnce(&mut S) -> Result<(), S::Error>,
) -> Result<(), ForegroundAcquireError<S::Error>> {
    if let Err(error) = mutation(system) {
        let rollback_failures = restore_state(system, state);
        return Err(operation_error(stage, error, rollback_failures));
    }
    Ok(())
}

fn operation_error<E>(
    stage: ForegroundStage,
    error: E,
    rollback_failures: Vec<StageFailure<E>>,
) -> ForegroundAcquireError<E> {
    ForegroundAcquireError::Operation(ForegroundOperationError {
        cause: StageFailure { stage, error },
        rollback_failures,
    })
}

fn restore_state<S: ForegroundSystem>(
    system: &mut S,
    state: &mut AcquiredState,
) -> Vec<StageFailure<S::Error>> {
    let had_mutation = state.prevent_screen_saver_changed
        || state.pillow_changed
        || !state.stopped_awesome.is_empty()
        || !state.stopped_cvm.is_empty();
    let mut failures = Vec::new();

    for identity in state.stopped_cvm.drain(..).rev() {
        attempt_cleanup(
            &mut failures,
            ForegroundStage::TransitionProcess(identity, ProcessTransition::Continue),
            system.transition_process(identity, ProcessTransition::Continue),
        );
    }
    for identity in state.stopped_awesome.drain(..).rev() {
        attempt_cleanup(
            &mut failures,
            ForegroundStage::TransitionProcess(identity, ProcessTransition::Continue),
            system.transition_process(identity, ProcessTransition::Continue),
        );
    }
    if state.pillow_changed {
        state.pillow_changed = false;
        attempt_cleanup(
            &mut failures,
            ForegroundStage::SetPillow(state.original_pillow),
            system.set_pillow(state.original_pillow),
        );
    }
    if state.prevent_screen_saver_changed {
        state.prevent_screen_saver_changed = false;
        attempt_cleanup(
            &mut failures,
            ForegroundStage::SetPreventScreenSaver(state.original_prevent_screen_saver),
            system.set_prevent_screen_saver(state.original_prevent_screen_saver),
        );
    }
    if had_mutation {
        attempt_cleanup(
            &mut failures,
            ForegroundStage::RepaintStock,
            system.repaint_stock(),
        );
    }

    failures
}

fn attempt_cleanup<E>(
    failures: &mut Vec<StageFailure<E>>,
    stage: ForegroundStage,
    result: Result<(), E>,
) {
    if let Err(error) = result {
        failures.push(StageFailure { stage, error });
    }
}

#[cfg(any(test, all(target_os = "linux", feature = "linux-kindle-foreground")))]
fn parse_proc_stat_start_time(input: &str) -> Option<NonZeroU64> {
    let command_end = input.rfind(')')?;
    let mut fields_after_command = input.get(command_end.checked_add(1)?..)?.split_whitespace();
    // Fields after `comm` begin at field 3 (`state`); starttime is field 22.
    let start_time = fields_after_command.nth(19)?.parse().ok()?;
    NonZeroU64::new(start_time)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(process: StockProcess, pid: u32, start_time: u64) -> ProcessIdentity {
        ProcessIdentity::new(
            process,
            NonZeroU32::new(pid).unwrap(),
            NonZeroU64::new(start_time).unwrap(),
        )
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FakeError {
        Injected,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Action {
        ReadPrevent,
        ReadPillow,
        List(StockProcess),
        SetPrevent(bool),
        SetPillow(PillowState),
        Transition(ProcessIdentity, ProcessTransition),
        Quiesce(Duration),
        Repaint,
    }

    #[derive(Debug)]
    struct FakeSystem {
        prevent: bool,
        pillow: PillowState,
        awesome: Vec<ProcessIdentity>,
        cvm: Vec<ProcessIdentity>,
        actions: Vec<Action>,
        fail_at: Option<Action>,
    }

    impl Default for FakeSystem {
        fn default() -> Self {
            Self {
                prevent: false,
                pillow: PillowState::Enabled,
                awesome: vec![identity(StockProcess::Awesome, 10, 100)],
                cvm: vec![identity(StockProcess::Cvm, 20, 200)],
                actions: Vec::new(),
                fail_at: None,
            }
        }
    }

    impl FakeSystem {
        fn record(&mut self, action: Action) -> Result<(), FakeError> {
            self.actions.push(action);
            if self.fail_at == Some(action) {
                Err(FakeError::Injected)
            } else {
                Ok(())
            }
        }
    }

    impl ForegroundSystem for FakeSystem {
        type Error = FakeError;

        fn prevent_screen_saver(&mut self) -> Result<bool, Self::Error> {
            self.record(Action::ReadPrevent)?;
            Ok(self.prevent)
        }

        fn pillow_state(&mut self) -> Result<PillowState, Self::Error> {
            self.record(Action::ReadPillow)?;
            Ok(self.pillow)
        }

        fn running_processes(
            &mut self,
            process: StockProcess,
        ) -> Result<Vec<ProcessIdentity>, Self::Error> {
            self.record(Action::List(process))?;
            Ok(match process {
                StockProcess::Awesome => self.awesome.clone(),
                StockProcess::Cvm => self.cvm.clone(),
            })
        }

        fn set_prevent_screen_saver(&mut self, prevent: bool) -> Result<(), Self::Error> {
            self.record(Action::SetPrevent(prevent))?;
            self.prevent = prevent;
            Ok(())
        }

        fn set_pillow(&mut self, state: PillowState) -> Result<(), Self::Error> {
            self.record(Action::SetPillow(state))?;
            self.pillow = state;
            Ok(())
        }

        fn transition_process(
            &mut self,
            identity: ProcessIdentity,
            transition: ProcessTransition,
        ) -> Result<(), Self::Error> {
            self.record(Action::Transition(identity, transition))
        }

        fn quiesce(&mut self, duration: Duration) -> Result<(), Self::Error> {
            self.record(Action::Quiesce(duration))
        }

        fn repaint_stock(&mut self) -> Result<(), Self::Error> {
            self.record(Action::Repaint)
        }
    }

    #[test]
    fn acquire_and_restore_follow_proven_order_and_restore_only_changed_state() {
        let mut system = FakeSystem::default();
        let awesome = system.awesome[0];
        let cvm = system.cvm[0];

        let lease = acquire_foreground(&mut system).unwrap();
        assert_eq!(lease.paused_process_count(), 2);
        lease.restore(&mut system).unwrap();

        assert_eq!(
            system.actions,
            [
                Action::ReadPrevent,
                Action::ReadPillow,
                Action::List(StockProcess::Awesome),
                Action::List(StockProcess::Cvm),
                Action::SetPrevent(true),
                Action::SetPillow(PillowState::Disabled),
                Action::Transition(awesome, ProcessTransition::Stop),
                Action::Transition(cvm, ProcessTransition::Stop),
                Action::Quiesce(FOREGROUND_QUIESCENCE),
                Action::Transition(cvm, ProcessTransition::Continue),
                Action::Transition(awesome, ProcessTransition::Continue),
                Action::SetPillow(PillowState::Enabled),
                Action::SetPrevent(false),
                Action::Repaint,
            ]
        );
    }

    #[test]
    fn existing_screensaver_inhibitor_is_neither_set_nor_cleared() {
        let mut system = FakeSystem {
            prevent: true,
            ..FakeSystem::default()
        };

        let lease = acquire_foreground(&mut system).unwrap();
        lease.restore(&mut system).unwrap();

        assert!(
            !system
                .actions
                .iter()
                .any(|action| matches!(action, Action::SetPrevent(_)))
        );
    }

    #[test]
    fn existing_disabled_pillow_is_neither_disabled_nor_enabled() {
        let mut system = FakeSystem {
            pillow: PillowState::Disabled,
            ..FakeSystem::default()
        };

        let lease = acquire_foreground(&mut system).unwrap();
        lease.restore(&mut system).unwrap();

        assert!(
            !system
                .actions
                .iter()
                .any(|action| matches!(action, Action::SetPillow(_)))
        );
    }

    #[test]
    fn partial_acquisition_failure_rolls_back_every_completed_mutation() {
        let mut system = FakeSystem::default();
        let awesome = system.awesome[0];
        let cvm = system.cvm[0];
        system.fail_at = Some(Action::Transition(cvm, ProcessTransition::Stop));

        let error = acquire_foreground(&mut system).unwrap_err();

        assert!(matches!(
            error,
            ForegroundAcquireError::Operation(ForegroundOperationError {
                cause: StageFailure {
                    stage: ForegroundStage::TransitionProcess(
                        identity,
                        ProcessTransition::Stop
                    ),
                    error: FakeError::Injected,
                },
                ref rollback_failures,
            }) if identity == cvm && rollback_failures.is_empty()
        ));
        assert_eq!(
            &system.actions[system.actions.len() - 5..],
            [
                Action::Transition(cvm, ProcessTransition::Continue),
                Action::Transition(awesome, ProcessTransition::Continue),
                Action::SetPillow(PillowState::Enabled),
                Action::SetPrevent(false),
                Action::Repaint,
            ]
        );
    }

    #[test]
    fn restoration_attempts_every_stage_and_retains_all_failures() {
        let mut system = FakeSystem::default();
        let lease = acquire_foreground(&mut system).unwrap();
        let cvm = system.cvm[0];
        system.fail_at = Some(Action::Transition(cvm, ProcessTransition::Continue));

        let error = lease.restore(&mut system).unwrap_err();

        assert_eq!(error.failures.len(), 1);
        assert_eq!(
            error.failures[0].stage,
            ForegroundStage::TransitionProcess(cvm, ProcessTransition::Continue)
        );
        assert_eq!(system.actions.last(), Some(&Action::Repaint));
    }

    #[test]
    fn invalid_process_sets_fail_before_the_first_mutation() {
        let mut system = FakeSystem {
            awesome: Vec::new(),
            ..FakeSystem::default()
        };

        assert!(matches!(
            acquire_foreground(&mut system),
            Err(ForegroundAcquireError::Policy(
                ForegroundPolicyError::MissingProcess(StockProcess::Awesome)
            ))
        ));
        assert!(system.actions.iter().all(|action| matches!(
            action,
            Action::ReadPrevent
                | Action::ReadPillow
                | Action::List(StockProcess::Awesome)
                | Action::List(StockProcess::Cvm)
        )));
    }

    #[test]
    fn early_boot_pauses_only_awesome_and_restores_exactly() {
        let mut system = FakeSystem {
            pillow: PillowState::Disabled,
            cvm: Vec::new(),
            ..FakeSystem::default()
        };
        let awesome = system.awesome[0];

        let lease = acquire_early_boot_foreground(&mut system).unwrap();
        assert_eq!(lease.paused_process_count(), 1);
        lease.restore(&mut system).unwrap();

        assert_eq!(
            system.actions,
            [
                Action::ReadPrevent,
                Action::ReadPillow,
                Action::List(StockProcess::Awesome),
                Action::List(StockProcess::Cvm),
                Action::SetPrevent(true),
                Action::Transition(awesome, ProcessTransition::Stop),
                Action::Quiesce(FOREGROUND_QUIESCENCE),
                Action::Transition(awesome, ProcessTransition::Continue),
                Action::SetPrevent(false),
            ]
        );
    }

    #[test]
    fn early_boot_rejects_contradictory_state_before_mutation() {
        for (system, expected) in [
            (
                FakeSystem {
                    cvm: Vec::new(),
                    ..FakeSystem::default()
                },
                EarlyBootPolicyError::PillowNotDisabled,
            ),
            (
                FakeSystem {
                    pillow: PillowState::Disabled,
                    awesome: Vec::new(),
                    cvm: Vec::new(),
                    ..FakeSystem::default()
                },
                EarlyBootPolicyError::InvalidAwesome(ForegroundPolicyError::MissingProcess(
                    StockProcess::Awesome,
                )),
            ),
            (
                FakeSystem {
                    pillow: PillowState::Disabled,
                    ..FakeSystem::default()
                },
                EarlyBootPolicyError::StockProcessRunning(StockProcess::Cvm),
            ),
        ] {
            let mut system = system;
            assert_eq!(
                acquire_early_boot_foreground(&mut system).unwrap_err(),
                EarlyBootAcquireError::Policy(expected)
            );
            assert!(
                system.actions.iter().all(|action| !matches!(
                    action,
                    Action::SetPrevent(_) | Action::Transition(..)
                ))
            );
        }
    }

    #[test]
    fn early_boot_inhibitor_failure_attempts_exact_rollback() {
        let mut system = FakeSystem {
            pillow: PillowState::Disabled,
            cvm: Vec::new(),
            fail_at: Some(Action::SetPrevent(true)),
            ..FakeSystem::default()
        };

        let error = acquire_early_boot_foreground(&mut system).unwrap_err();

        assert!(matches!(
            error,
            EarlyBootAcquireError::Operation(ForegroundOperationError {
                cause: StageFailure {
                    stage: ForegroundStage::SetPreventScreenSaver(true),
                    error: FakeError::Injected,
                },
                ref rollback_failures,
            }) if rollback_failures.is_empty()
        ));
        assert_eq!(
            &system.actions[system.actions.len() - 2..],
            [Action::SetPrevent(true), Action::SetPrevent(false)]
        );
    }

    #[test]
    fn early_boot_awesome_failure_resumes_identity_and_clears_inhibitor() {
        let mut system = FakeSystem {
            pillow: PillowState::Disabled,
            cvm: Vec::new(),
            ..FakeSystem::default()
        };
        let awesome = system.awesome[0];
        system.fail_at = Some(Action::Transition(awesome, ProcessTransition::Stop));

        let error = acquire_early_boot_foreground(&mut system).unwrap_err();

        assert!(matches!(
            error,
            EarlyBootAcquireError::Operation(ForegroundOperationError {
                cause: StageFailure {
                    stage: ForegroundStage::TransitionProcess(
                        identity,
                        ProcessTransition::Stop
                    ),
                    error: FakeError::Injected,
                },
                ref rollback_failures,
            }) if identity == awesome && rollback_failures.is_empty()
        ));
        assert_eq!(
            &system.actions[system.actions.len() - 3..],
            [
                Action::Transition(awesome, ProcessTransition::Stop),
                Action::Transition(awesome, ProcessTransition::Continue),
                Action::SetPrevent(false),
            ]
        );
    }

    #[test]
    fn proc_stat_parser_handles_spaces_and_parentheses_in_command_name() {
        let stat = "42 (name with ) paren) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 98765 20";
        assert_eq!(parse_proc_stat_start_time(stat), NonZeroU64::new(98_765));
        assert_eq!(parse_proc_stat_start_time("42 malformed"), None);
        assert_eq!(
            parse_proc_stat_start_time(
                "42 (zero) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 0"
            ),
            None
        );
    }
}
