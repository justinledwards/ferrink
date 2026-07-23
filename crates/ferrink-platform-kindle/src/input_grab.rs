//! Explicit exclusive-input ownership with descriptor-close crash fallback.

#![deny(unsafe_code)]

use std::num::NonZeroI32;
use std::time::Instant;

use crate::{
    BoundedInputPump, InputLoopError, InputPumpOutcome, L0InputCore, NonBlockingInputSource,
    RevalidatedReadOnlySession,
};

/// Exact exclusive-input transition being attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputGrabOperation {
    /// Request exclusive delivery with `EVIOCGRAB(1)`.
    Acquire,
    /// Return shared delivery with `EVIOCGRAB(0)`.
    Release,
}

/// Sanitized failure from one exclusive-input transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputGrabIoError {
    operation: InputGrabOperation,
    errno: Option<NonZeroI32>,
}

impl InputGrabIoError {
    /// Creates a bounded error without retaining a device path or input data.
    #[must_use]
    pub const fn new(operation: InputGrabOperation, errno: Option<NonZeroI32>) -> Self {
        Self { operation, errno }
    }

    /// Returns the exact failed transition.
    #[must_use]
    pub const fn operation(self) -> InputGrabOperation {
        self.operation
    }

    /// Returns the positive platform error number when available.
    #[must_use]
    pub const fn errno(self) -> Option<NonZeroI32> {
        self.errno
    }
}

impl std::fmt::Display for InputGrabIoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let action = match self.operation {
            InputGrabOperation::Acquire => "acquire input grab",
            InputGrabOperation::Release => "release input grab",
        };
        formatter.write_str(action)?;
        if let Some(errno) = self.errno {
            write!(formatter, " failed with errno {}", errno.get())
        } else {
            formatter.write_str(" failed")
        }
    }
}

impl std::error::Error for InputGrabIoError {}

/// Minimal seam for the exact `EVIOCGRAB` transition.
pub trait ExclusiveInputSource {
    /// Requests or releases exclusive delivery exactly once.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error without an internal retry or alternate path.
    fn set_exclusive(&mut self, exclusive: bool) -> Result<(), InputGrabIoError>;
}

/// Revalidated session that currently owns exclusive input delivery.
///
/// Normal callers must invoke [`Self::release`]. If the process exits or the
/// explicit release fails, dropping the retained descriptor is the kernel-level
/// fallback. Drop deliberately performs no ioctl and no retry.
#[derive(Debug)]
#[must_use = "exclusive input must be explicitly released"]
pub struct ExclusiveInputSession<I, F> {
    session: Option<RevalidatedReadOnlySession<I, F>>,
}

impl<I: ExclusiveInputSource, F> ExclusiveInputSession<I, F> {
    /// Acquires exclusive delivery on an already revalidated exact descriptor.
    ///
    /// # Errors
    ///
    /// On failure, the supplied session drops and closes its descriptor.
    pub fn acquire(
        mut session: RevalidatedReadOnlySession<I, F>,
    ) -> Result<Self, InputGrabIoError> {
        session.input_mut().set_exclusive(true)?;
        Ok(Self {
            session: Some(session),
        })
    }

    /// Attempts one bounded readiness/read/decode step while grabbed.
    ///
    /// # Errors
    ///
    /// Preserves the bounded pump's structured error without releasing or
    /// retrying implicitly; the caller still owns explicit cleanup.
    pub fn pump_input_at(
        &mut self,
        pump: &mut BoundedInputPump,
        now: Instant,
        input: &mut L0InputCore,
    ) -> Result<InputPumpOutcome, InputLoopError>
    where
        I: NonBlockingInputSource,
    {
        self.session
            .as_mut()
            .expect("BUG: exclusive input used after release")
            .pump_input_at(pump, now, input)
    }

    /// Attempts exactly one explicit release and then closes both descriptors.
    ///
    /// Descriptor close occurs after the release attempt whether it succeeds
    /// or fails, so a failed normal release still reaches the kernel fallback.
    ///
    /// # Errors
    ///
    /// Returns the one release failure after descriptor closure. No retry is
    /// attempted.
    pub fn release(mut self) -> Result<(), InputGrabIoError> {
        let mut session = self
            .session
            .take()
            .expect("BUG: exclusive input session missing before release");
        let result = session.input_mut().set_exclusive(false);
        drop(session);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Action {
        Acquire,
        Release,
        DropInput,
        DropFramebuffer,
    }

    #[derive(Debug)]
    struct FakeInput {
        log: Rc<RefCell<Vec<Action>>>,
        fail_acquire: bool,
        fail_release: bool,
    }

    impl ExclusiveInputSource for FakeInput {
        fn set_exclusive(&mut self, exclusive: bool) -> Result<(), InputGrabIoError> {
            let operation = if exclusive {
                self.log.borrow_mut().push(Action::Acquire);
                InputGrabOperation::Acquire
            } else {
                self.log.borrow_mut().push(Action::Release);
                InputGrabOperation::Release
            };
            if (exclusive && self.fail_acquire) || (!exclusive && self.fail_release) {
                Err(InputGrabIoError::new(operation, NonZeroI32::new(5)))
            } else {
                Ok(())
            }
        }
    }

    impl Drop for FakeInput {
        fn drop(&mut self) {
            self.log.borrow_mut().push(Action::DropInput);
        }
    }

    #[derive(Debug)]
    struct FakeFramebuffer(Rc<RefCell<Vec<Action>>>);

    impl Drop for FakeFramebuffer {
        fn drop(&mut self) {
            self.0.borrow_mut().push(Action::DropFramebuffer);
        }
    }

    fn session(
        fail_acquire: bool,
        fail_release: bool,
    ) -> (
        RevalidatedReadOnlySession<FakeInput, FakeFramebuffer>,
        Rc<RefCell<Vec<Action>>>,
    ) {
        let log = Rc::new(RefCell::new(Vec::new()));
        (
            RevalidatedReadOnlySession::from_parts(
                FakeInput {
                    log: Rc::clone(&log),
                    fail_acquire,
                    fail_release,
                },
                FakeFramebuffer(Rc::clone(&log)),
            ),
            log,
        )
    }

    #[test]
    fn normal_path_grabs_releases_then_closes_in_reverse_order() {
        let (session, log) = session(false, false);
        let exclusive = ExclusiveInputSession::acquire(session).unwrap();
        assert_eq!(&*log.borrow(), &[Action::Acquire]);

        exclusive.release().unwrap();
        assert_eq!(
            &*log.borrow(),
            &[
                Action::Acquire,
                Action::Release,
                Action::DropInput,
                Action::DropFramebuffer,
            ]
        );
    }

    #[test]
    fn acquisition_failure_closes_without_a_release_ioctl() {
        let (session, log) = session(true, false);
        assert!(ExclusiveInputSession::acquire(session).is_err());
        assert_eq!(
            &*log.borrow(),
            &[Action::Acquire, Action::DropInput, Action::DropFramebuffer]
        );
    }

    #[test]
    fn release_failure_still_closes_once_without_retry() {
        let (session, log) = session(false, true);
        let exclusive = ExclusiveInputSession::acquire(session).unwrap();
        assert!(exclusive.release().is_err());
        assert_eq!(
            &*log.borrow(),
            &[
                Action::Acquire,
                Action::Release,
                Action::DropInput,
                Action::DropFramebuffer,
            ]
        );
    }

    #[test]
    fn drop_uses_only_descriptor_close_as_crash_fallback() {
        let (session, log) = session(false, false);
        let exclusive = ExclusiveInputSession::acquire(session).unwrap();
        drop(exclusive);
        assert_eq!(
            &*log.borrow(),
            &[Action::Acquire, Action::DropInput, Action::DropFramebuffer]
        );
    }
}
