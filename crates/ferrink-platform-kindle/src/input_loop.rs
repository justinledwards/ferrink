//! Bounded, nonblocking input-loop policy with no descriptor implementation.

#![deny(unsafe_code)]

use std::num::{NonZeroI32, NonZeroU32};
use std::time::{Duration, Instant};

use ferrink_platform::TouchContactEvent;

use crate::{L0InputCore, L0InputError, RevalidatedReadOnlySession};

/// Result of one bounded readiness poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputPollStatus {
    /// The descriptor may have bytes available.
    Readable,
    /// The supplied bounded timeout elapsed.
    TimedOut,
    /// A signal interrupted the poll before readiness.
    Interrupted,
}

/// Result of one nonblocking read attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputReadStatus {
    /// The implementation filled this many bytes at the start of the buffer.
    Bytes(usize),
    /// Readiness disappeared before the nonblocking read.
    WouldBlock,
    /// A signal interrupted the read before any bytes were returned.
    Interrupted,
    /// The descriptor reached end-of-file.
    EndOfFile,
}

/// One descriptor operation in the bounded input loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputStreamOperation {
    /// Wait for readable input with a bounded timeout.
    Poll,
    /// Attempt one nonblocking input read.
    Read,
}

impl std::fmt::Display for InputStreamOperation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Poll => formatter.write_str("poll input"),
            Self::Read => formatter.write_str("read input"),
        }
    }
}

/// Sanitized operating-system failure from an input descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputStreamIoError {
    operation: InputStreamOperation,
    errno: Option<NonZeroI32>,
}

impl InputStreamIoError {
    /// Creates an error without retaining input data or a device path.
    #[must_use]
    pub const fn new(operation: InputStreamOperation, errno: Option<NonZeroI32>) -> Self {
        Self { operation, errno }
    }

    /// Returns the failed descriptor operation.
    #[must_use]
    pub const fn operation(self) -> InputStreamOperation {
        self.operation
    }

    /// Returns the positive OS error number when one was available.
    #[must_use]
    pub const fn errno(self) -> Option<NonZeroI32> {
        self.errno
    }
}

impl std::fmt::Display for InputStreamIoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} failed", self.operation)?;
        if let Some(errno) = self.errno {
            write!(formatter, " with errno {}", errno.get())?;
        }
        Ok(())
    }
}

impl std::error::Error for InputStreamIoError {}

/// Minimal nonblocking input seam used by [`BoundedInputPump`].
pub trait NonBlockingInputSource {
    /// Waits no longer than `timeout` for readable input.
    ///
    /// # Errors
    ///
    /// Returns a sanitized descriptor failure. `EINTR` must be represented as
    /// [`InputPollStatus::Interrupted`], not retried internally.
    fn poll_readable(&mut self, timeout: Duration) -> Result<InputPollStatus, InputStreamIoError>;

    /// Attempts one read without blocking.
    ///
    /// # Errors
    ///
    /// Returns a sanitized descriptor failure. `EINTR` and `EAGAIN` must use
    /// their corresponding [`InputReadStatus`] variants.
    fn read_nonblocking(
        &mut self,
        buffer: &mut [u8],
    ) -> Result<InputReadStatus, InputStreamIoError>;
}

/// Fixed limits for one input-pump lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputLoopLimits {
    /// Hard wall-clock lifetime.
    pub maximum_duration: Duration,
    /// Maximum timeout supplied to any one readiness poll.
    pub maximum_poll_slice: Duration,
    /// Maximum number of calls to
    /// [`RevalidatedReadOnlySession::pump_input_at`].
    pub maximum_steps: NonZeroU32,
    /// Maximum cumulative bytes accepted from the descriptor.
    pub maximum_bytes: NonZeroU32,
    /// Size of the reusable ordinary-memory read buffer.
    pub read_buffer_bytes: NonZeroU32,
}

impl InputLoopLimits {
    fn validate(self) -> Result<(), InputLoopConfigError> {
        if self.maximum_duration.is_zero() {
            return Err(InputLoopConfigError::ZeroDuration);
        }
        if self.maximum_poll_slice.is_zero() {
            return Err(InputLoopConfigError::ZeroPollSlice);
        }
        if self.read_buffer_bytes > self.maximum_bytes {
            return Err(InputLoopConfigError::ReadBufferExceedsByteBudget);
        }
        Ok(())
    }
}

/// Configuration failure before a bounded input pump starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputLoopConfigError {
    /// The hard deadline duration was zero.
    ZeroDuration,
    /// The per-poll timeout was zero.
    ZeroPollSlice,
    /// One read could exceed the complete byte budget.
    ReadBufferExceedsByteBudget,
    /// Adding the duration to the supplied monotonic start time overflowed.
    DeadlineOverflow,
    /// The read buffer length did not fit the current architecture.
    BufferLengthUnsupported,
    /// Ordinary read-buffer memory could not be reserved.
    AllocationFailed,
}

impl std::fmt::Display for InputLoopConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroDuration => formatter.write_str("input deadline duration must be non-zero"),
            Self::ZeroPollSlice => formatter.write_str("input poll slice must be non-zero"),
            Self::ReadBufferExceedsByteBudget => {
                formatter.write_str("input read buffer exceeds the total byte budget")
            }
            Self::DeadlineOverflow => formatter.write_str("input deadline overflowed"),
            Self::BufferLengthUnsupported => {
                formatter.write_str("input read buffer length is unsupported")
            }
            Self::AllocationFailed => formatter.write_str("input read-buffer allocation failed"),
        }
    }
}

impl std::error::Error for InputLoopConfigError {}

/// Host-testable state for a single bounded nonblocking input lifetime.
#[derive(Debug)]
pub struct BoundedInputPump {
    deadline: Instant,
    maximum_poll_slice: Duration,
    remaining_steps: u32,
    remaining_bytes: u32,
    buffer: Vec<u8>,
    stopped: bool,
}

impl BoundedInputPump {
    /// Allocates the fixed read buffer and computes the hard monotonic deadline.
    ///
    /// # Errors
    ///
    /// Returns [`InputLoopConfigError`] for invalid limits, deadline overflow,
    /// an unsupported buffer length, or allocation failure.
    pub fn try_new(start: Instant, limits: InputLoopLimits) -> Result<Self, InputLoopConfigError> {
        limits.validate()?;
        let deadline = start
            .checked_add(limits.maximum_duration)
            .ok_or(InputLoopConfigError::DeadlineOverflow)?;
        let buffer_len = usize::try_from(limits.read_buffer_bytes.get())
            .map_err(|_| InputLoopConfigError::BufferLengthUnsupported)?;
        let mut buffer = Vec::new();
        buffer
            .try_reserve_exact(buffer_len)
            .map_err(|_| InputLoopConfigError::AllocationFailed)?;
        buffer.resize(buffer_len, 0);
        Ok(Self {
            deadline,
            maximum_poll_slice: limits.maximum_poll_slice,
            remaining_steps: limits.maximum_steps.get(),
            remaining_bytes: limits.maximum_bytes.get(),
            buffer,
            stopped: false,
        })
    }

    /// Permanently stops this pump. Descriptor closure remains owned by the
    /// surrounding read-only session.
    pub fn stop(&mut self) {
        self.stopped = true;
    }

    /// Returns whether the pump has permanently stopped.
    #[must_use]
    pub const fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// Returns the remaining cumulative byte allowance.
    #[must_use]
    pub const fn remaining_bytes(&self) -> u32 {
        self.remaining_bytes
    }

    /// Returns the remaining call allowance.
    #[must_use]
    pub const fn remaining_steps(&self) -> u32 {
        self.remaining_steps
    }

    fn step_at<S: NonBlockingInputSource>(
        &mut self,
        now: Instant,
        source: &mut S,
        input: &mut L0InputCore,
    ) -> Result<InputPumpOutcome, InputLoopError> {
        if self.stopped {
            return Err(InputLoopError::Stopped);
        }
        let Some(remaining_duration) = self.deadline.checked_duration_since(now) else {
            self.stopped = true;
            return Err(InputLoopError::DeadlineReached);
        };
        if remaining_duration.is_zero() {
            self.stopped = true;
            return Err(InputLoopError::DeadlineReached);
        }
        if self.remaining_steps == 0 {
            self.stopped = true;
            return Err(InputLoopError::StepBudgetExhausted);
        }
        self.remaining_steps -= 1;

        let poll_timeout = remaining_duration.min(self.maximum_poll_slice);
        let poll = match source.poll_readable(poll_timeout) {
            Ok(poll) => poll,
            Err(error) => return self.stop_with(InputLoopError::Io(error)),
        };
        match poll {
            InputPollStatus::TimedOut => return Ok(InputPumpOutcome::TimedOut),
            InputPollStatus::Interrupted => return Ok(InputPumpOutcome::Interrupted),
            InputPollStatus::Readable => {}
        }

        let read = match source.read_nonblocking(self.buffer.as_mut_slice()) {
            Ok(read) => read,
            Err(error) => return self.stop_with(InputLoopError::Io(error)),
        };
        match read {
            InputReadStatus::WouldBlock => Ok(InputPumpOutcome::WouldBlock),
            InputReadStatus::Interrupted => Ok(InputPumpOutcome::Interrupted),
            InputReadStatus::EndOfFile => self.stop_with(InputLoopError::InputClosed),
            InputReadStatus::Bytes(observed) => {
                if observed == 0 || observed > self.buffer.len() {
                    return self.stop_with(InputLoopError::InvalidReadCount {
                        observed,
                        buffer_length: self.buffer.len(),
                    });
                }
                let observed_u32 = u32::try_from(observed).map_err(|_| {
                    self.stopped = true;
                    InputLoopError::InvalidReadCount {
                        observed,
                        buffer_length: self.buffer.len(),
                    }
                })?;
                if observed_u32 > self.remaining_bytes {
                    return self.stop_with(InputLoopError::ByteBudgetExhausted {
                        observed: observed_u32,
                        remaining: self.remaining_bytes,
                    });
                }
                self.remaining_bytes -= observed_u32;
                let contacts = match input.push_bytes(&self.buffer[..observed]) {
                    Ok(contacts) => contacts,
                    Err(error) => return self.stop_with(InputLoopError::Input(error)),
                };
                Ok(InputPumpOutcome::Read {
                    bytes: observed_u32,
                    contacts,
                })
            }
        }
    }

    fn stop_with<T>(&mut self, error: InputLoopError) -> Result<T, InputLoopError> {
        self.stopped = true;
        Err(error)
    }
}

impl<I: NonBlockingInputSource, F> RevalidatedReadOnlySession<I, F> {
    /// Performs one bounded readiness/read/decode step using the retained exact
    /// input descriptor.
    ///
    /// This method performs no internal retry. The caller regains control after
    /// every timeout, interrupt, would-block result, read, or failure.
    ///
    /// # Errors
    ///
    /// Returns [`InputLoopError`] and permanently stops the pump for deadline,
    /// budget, descriptor, end-of-file, invalid-count, or decode failures.
    pub fn pump_input_at(
        &mut self,
        pump: &mut BoundedInputPump,
        now: Instant,
        input: &mut L0InputCore,
    ) -> Result<InputPumpOutcome, InputLoopError> {
        pump.step_at(now, self.input_mut(), input)
    }
}

/// Observable outcome of exactly one bounded pump step.
#[derive(Debug, PartialEq, Eq)]
pub enum InputPumpOutcome {
    /// The bounded readiness timeout elapsed.
    TimedOut,
    /// Poll or read was interrupted; the outer loop may choose another step.
    Interrupted,
    /// The nonblocking read found no bytes after readiness.
    WouldBlock,
    /// One byte chunk was decoded atomically.
    Read {
        /// Number of raw bytes accepted against the cumulative budget.
        bytes: u32,
        /// Logical contact transitions emitted by the incremental tracker.
        contacts: Vec<TouchContactEvent>,
    },
}

/// Permanent or caller-visible failure from the bounded input pump.
#[derive(Debug)]
#[non_exhaustive]
pub enum InputLoopError {
    /// The pump was explicitly stopped or failed previously.
    Stopped,
    /// The hard monotonic deadline was reached.
    DeadlineReached,
    /// The maximum number of outer-loop steps was consumed.
    StepBudgetExhausted,
    /// A read would exceed the cumulative byte allowance.
    ByteBudgetExhausted {
        /// Bytes reported by this read.
        observed: u32,
        /// Bytes still permitted before this read.
        remaining: u32,
    },
    /// The source claimed an impossible buffer fill count.
    InvalidReadCount {
        /// Claimed byte count.
        observed: usize,
        /// Supplied buffer length.
        buffer_length: usize,
    },
    /// The input descriptor reached end-of-file.
    InputClosed,
    /// A true descriptor failure occurred.
    Io(InputStreamIoError),
    /// Raw record decoding or touch tracking failed atomically.
    Input(L0InputError),
}

impl std::fmt::Display for InputLoopError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped => formatter.write_str("input pump is stopped"),
            Self::DeadlineReached => formatter.write_str("input pump deadline was reached"),
            Self::StepBudgetExhausted => {
                formatter.write_str("input pump step budget was exhausted")
            }
            Self::ByteBudgetExhausted {
                observed,
                remaining,
            } => write!(
                formatter,
                "input read returned {observed} bytes with {remaining} bytes remaining"
            ),
            Self::InvalidReadCount {
                observed,
                buffer_length,
            } => write!(
                formatter,
                "input source reported {observed} bytes for a {buffer_length}-byte buffer"
            ),
            Self::InputClosed => formatter.write_str("input descriptor reached end-of-file"),
            Self::Io(error) => write!(formatter, "input descriptor failed: {error}"),
            Self::Input(error) => write!(formatter, "input decoding failed: {error}"),
        }
    }
}

impl std::error::Error for InputLoopError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Input(error) => Some(error),
            Self::Stopped
            | Self::DeadlineReached
            | Self::StepBudgetExhausted
            | Self::ByteBudgetExhausted { .. }
            | Self::InvalidReadCount { .. }
            | Self::InputClosed => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrink_platform::{DeviceProfile, LogicalTouchPhase, ProbeReport, ResolvedRuntimeDevice};
    use std::collections::VecDeque;

    const KOA3_REPORT: &str =
        include_str!("../../ferrink-platform/tests/fixtures/probe-reference-portrait.json");
    const KOA3_PROFILE: &str = include_str!("../../../device-profiles/reference-portrait.toml");

    #[derive(Debug)]
    enum ReadAction {
        Status(InputReadStatus),
        Bytes(Vec<u8>),
    }

    #[derive(Debug)]
    struct FakeSource {
        polls: VecDeque<Result<InputPollStatus, InputStreamIoError>>,
        reads: VecDeque<Result<ReadAction, InputStreamIoError>>,
        timeouts: Vec<Duration>,
    }

    impl NonBlockingInputSource for FakeSource {
        fn poll_readable(
            &mut self,
            timeout: Duration,
        ) -> Result<InputPollStatus, InputStreamIoError> {
            self.timeouts.push(timeout);
            self.polls.pop_front().expect("scripted poll result")
        }

        fn read_nonblocking(
            &mut self,
            buffer: &mut [u8],
        ) -> Result<InputReadStatus, InputStreamIoError> {
            match self.reads.pop_front().expect("scripted read result")? {
                ReadAction::Status(status) => Ok(status),
                ReadAction::Bytes(bytes) => {
                    let copied = bytes.len().min(buffer.len());
                    buffer[..copied].copy_from_slice(&bytes[..copied]);
                    Ok(InputReadStatus::Bytes(bytes.len()))
                }
            }
        }
    }

    fn runtime() -> ResolvedRuntimeDevice {
        let profile = DeviceProfile::from_toml(KOA3_PROFILE).unwrap();
        let report = ProbeReport::from_json(KOA3_REPORT).unwrap();
        ResolvedRuntimeDevice::resolve(&profile, &report).unwrap()
    }

    fn input() -> L0InputCore {
        L0InputCore::try_from_runtime(&runtime(), NonZeroU32::new(16).unwrap()).unwrap()
    }

    fn limits(maximum_steps: u32, maximum_bytes: u32, read_buffer_bytes: u32) -> InputLoopLimits {
        InputLoopLimits {
            maximum_duration: Duration::from_secs(5),
            maximum_poll_slice: Duration::from_millis(250),
            maximum_steps: NonZeroU32::new(maximum_steps).unwrap(),
            maximum_bytes: NonZeroU32::new(maximum_bytes).unwrap(),
            read_buffer_bytes: NonZeroU32::new(read_buffer_bytes).unwrap(),
        }
    }

    #[test]
    fn interrupts_and_would_block_return_without_internal_retry() {
        let start = Instant::now();
        let mut pump = BoundedInputPump::try_new(start, limits(3, 64, 64)).unwrap();
        let mut source = FakeSource {
            polls: VecDeque::from([
                Ok(InputPollStatus::Interrupted),
                Ok(InputPollStatus::Readable),
            ]),
            reads: VecDeque::from([Ok(ReadAction::Status(InputReadStatus::WouldBlock))]),
            timeouts: Vec::new(),
        };
        let mut input = input();

        assert_eq!(
            pump.step_at(start, &mut source, &mut input).unwrap(),
            InputPumpOutcome::Interrupted
        );
        assert_eq!(source.polls.len(), 1);
        assert_eq!(
            pump.step_at(start, &mut source, &mut input).unwrap(),
            InputPumpOutcome::WouldBlock
        );
        assert_eq!(source.polls.len(), 0);
        assert_eq!(source.timeouts, [Duration::from_millis(250); 2]);
        assert_eq!(pump.remaining_steps(), 1);
        assert!(!pump.is_stopped());
    }

    #[test]
    fn one_chunk_reaches_incremental_touch_tracking() {
        let start = Instant::now();
        let bytes = touch_press_records();
        let mut pump = BoundedInputPump::try_new(start, limits(1, 64, 64)).unwrap();
        let mut source = FakeSource {
            polls: VecDeque::from([Ok(InputPollStatus::Readable)]),
            reads: VecDeque::from([Ok(ReadAction::Bytes(bytes))]),
            timeouts: Vec::new(),
        };
        let mut input = input();

        let outcome = pump.step_at(start, &mut source, &mut input).unwrap();
        let InputPumpOutcome::Read { bytes, contacts } = outcome else {
            panic!("expected one decoded read");
        };
        assert_eq!(bytes, 64);
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].phase, LogicalTouchPhase::Pressed);
        assert_eq!(contacts[0].point.x, 100);
        assert_eq!(contacts[0].point.y, 200);
        assert_eq!(pump.remaining_bytes(), 0);
    }

    #[test]
    fn deadline_and_step_budget_stop_before_an_extra_poll() {
        let start = Instant::now();
        let mut deadline_pump = BoundedInputPump::try_new(start, limits(1, 64, 64)).unwrap();
        let mut no_calls = FakeSource {
            polls: VecDeque::new(),
            reads: VecDeque::new(),
            timeouts: Vec::new(),
        };
        let mut input = input();
        assert!(matches!(
            deadline_pump.step_at(start + Duration::from_secs(5), &mut no_calls, &mut input),
            Err(InputLoopError::DeadlineReached)
        ));
        assert!(no_calls.timeouts.is_empty());
        assert!(deadline_pump.is_stopped());

        let mut step_pump = BoundedInputPump::try_new(start, limits(1, 64, 64)).unwrap();
        let mut source = FakeSource {
            polls: VecDeque::from([Ok(InputPollStatus::Interrupted)]),
            reads: VecDeque::new(),
            timeouts: Vec::new(),
        };
        assert_eq!(
            step_pump.step_at(start, &mut source, &mut input).unwrap(),
            InputPumpOutcome::Interrupted
        );
        assert!(matches!(
            step_pump.step_at(start, &mut source, &mut input),
            Err(InputLoopError::StepBudgetExhausted)
        ));
        assert_eq!(source.timeouts.len(), 1);
        assert!(step_pump.is_stopped());
    }

    #[test]
    fn byte_budget_failure_does_not_advance_decoder() {
        let start = Instant::now();
        let mut pump = BoundedInputPump::try_new(start, limits(2, 80, 64)).unwrap();
        let mut source = FakeSource {
            polls: VecDeque::from([Ok(InputPollStatus::Readable), Ok(InputPollStatus::Readable)]),
            reads: VecDeque::from([
                Ok(ReadAction::Bytes(touch_press_records())),
                Ok(ReadAction::Bytes(vec![0; 32])),
            ]),
            timeouts: Vec::new(),
        };
        let mut input = input();

        assert!(matches!(
            pump.step_at(start, &mut source, &mut input),
            Ok(InputPumpOutcome::Read { bytes: 64, .. })
        ));
        assert_eq!(input.decoded_records(), 4);
        assert!(matches!(
            pump.step_at(start, &mut source, &mut input),
            Err(InputLoopError::ByteBudgetExhausted {
                observed: 32,
                remaining: 16
            })
        ));
        assert_eq!(input.decoded_records(), 4);
        assert!(pump.is_stopped());
    }

    fn touch_press_records() -> Vec<u8> {
        let mut records = Vec::new();
        records.extend_from_slice(&record32(1, 0, 3, 53, 100));
        records.extend_from_slice(&record32(1, 1, 3, 54, 200));
        records.extend_from_slice(&record32(1, 2, 3, 57, 7));
        records.extend_from_slice(&record32(1, 3, 0, 0, 0));
        records
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
