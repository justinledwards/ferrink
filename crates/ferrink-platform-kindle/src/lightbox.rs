//! Host-tested KOA3 post-framebuffer lightbox policy and request encoding.

#![deny(unsafe_code)]

use ferrink_platform::RefreshRegion;

/// KOA3 legacy lightbox request number with its exact 36-byte payload size.
#[cfg(target_os = "linux")]
pub(crate) const KOA3_APPLY_HALFTONE: u64 = 0x4024_464b;

const KOA3_LIGHTBOX_MODE: u32 = 1;

/// Stable lightbox state owned by the foreground display adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Koa3LightboxState {
    /// No sharp foreground region is configured.
    #[default]
    Clear,
    /// One validated foreground region remains sharp while its exterior is
    /// halftoned.
    Foreground(RefreshRegion),
}

/// Exact 36-byte KOA3 lightbox payload.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Koa3LightboxRequest {
    first_top: u32,
    first_left: u32,
    first_width: u32,
    first_height: u32,
    second_top: u32,
    second_left: u32,
    second_width: u32,
    second_height: u32,
    mode: u32,
}

impl Koa3LightboxRequest {
    /// Encodes one stable lightbox state using the reviewed KOA3 field order.
    #[must_use]
    pub const fn encode(state: Koa3LightboxState) -> Self {
        match state {
            Koa3LightboxState::Clear => Self {
                first_top: 0,
                first_left: 0,
                first_width: 0,
                first_height: 0,
                second_top: 0,
                second_left: 0,
                second_width: 0,
                second_height: 0,
                mode: KOA3_LIGHTBOX_MODE,
            },
            Koa3LightboxState::Foreground(region) => Self {
                first_top: region.y(),
                first_left: region.x(),
                first_width: region.width(),
                first_height: region.height(),
                second_top: 0,
                second_left: 0,
                second_width: 0,
                second_height: 0,
                mode: KOA3_LIGHTBOX_MODE,
            },
        }
    }

    #[cfg(test)]
    const fn words(self) -> [u32; 9] {
        [
            self.first_top,
            self.first_left,
            self.first_width,
            self.first_height,
            self.second_top,
            self.second_left,
            self.second_width,
            self.second_height,
            self.mode,
        ]
    }
}

/// Kernel result accepted for the exact reviewed KOA3 request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Koa3LightboxSubmission {
    /// The request returned success.
    Accepted,
    /// The request returned the physically verified KOA3 `EINVAL` result.
    ExpectedInvalidArgument,
}

/// Device-I/O seam for one already encoded KOA3 lightbox request.
pub trait Koa3LightboxTarget {
    /// Unexpected adapter error.
    type Error;

    /// Submits one request without repainting or retrying.
    fn submit_lightbox(
        &mut self,
        request: Koa3LightboxRequest,
    ) -> Result<Koa3LightboxSubmission, Self::Error>;
}

/// Result of preparing a stable-state transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Koa3LightboxTransition {
    /// Desired and presented states already matched.
    Unchanged,
    /// The request was submitted and requires a full repaint before commit.
    Prepared(Koa3LightboxSubmission),
}

/// Host-testable two-phase lightbox state machine.
#[derive(Debug, Default)]
pub struct Koa3LightboxController {
    presented: Koa3LightboxState,
    pending: Option<Koa3LightboxState>,
}

impl Koa3LightboxController {
    /// Returns the state whose required full repaint completed.
    #[must_use]
    pub const fn presented(&self) -> Koa3LightboxState {
        self.presented
    }

    /// Returns whether a submitted state still awaits its full repaint.
    #[must_use]
    pub const fn has_pending_transition(&self) -> bool {
        self.pending.is_some()
    }

    /// Submits a changed desired state without claiming it is visible yet.
    ///
    /// # Errors
    ///
    /// Returns [`Koa3LightboxControllerError::TransitionPending`] if a prior
    /// request has not been committed or cancelled, or wraps the target error.
    pub fn prepare<T: Koa3LightboxTarget>(
        &mut self,
        target: &mut T,
        desired: Koa3LightboxState,
    ) -> Result<Koa3LightboxTransition, Koa3LightboxControllerError<T::Error>> {
        if self.pending.is_some() {
            return Err(Koa3LightboxControllerError::TransitionPending);
        }
        if desired == self.presented {
            return Ok(Koa3LightboxTransition::Unchanged);
        }
        let submission = target
            .submit_lightbox(Koa3LightboxRequest::encode(desired))
            .map_err(Koa3LightboxControllerError::Submit)?;
        self.pending = Some(desired);
        Ok(Koa3LightboxTransition::Prepared(submission))
    }

    /// Commits the pending state after its full repaint was submitted.
    ///
    /// # Errors
    ///
    /// Returns [`Koa3LightboxControllerError::NoPendingTransition`] when no
    /// request is awaiting presentation.
    pub fn commit_presented(&mut self) -> Result<(), Koa3LightboxControllerError<()>> {
        let pending = self
            .pending
            .take()
            .ok_or(Koa3LightboxControllerError::NoPendingTransition)?;
        self.presented = pending;
        Ok(())
    }

    /// Explicitly clears any active or pending foreground state before a
    /// handoff. This does not repaint.
    ///
    /// # Errors
    ///
    /// Wraps an unexpected target error. An already clear controller performs
    /// no ioctl.
    pub fn clear_for_handoff<T: Koa3LightboxTarget>(
        &mut self,
        target: &mut T,
    ) -> Result<Koa3LightboxTransition, Koa3LightboxControllerError<T::Error>> {
        if self.presented == Koa3LightboxState::Clear {
            if self.pending == Some(Koa3LightboxState::Clear) {
                self.pending = None;
            }
            if self.pending.is_none() {
                return Ok(Koa3LightboxTransition::Unchanged);
            }
        }
        let submission = target
            .submit_lightbox(Koa3LightboxRequest::encode(Koa3LightboxState::Clear))
            .map_err(Koa3LightboxControllerError::Submit)?;
        self.presented = Koa3LightboxState::Clear;
        self.pending = None;
        Ok(Koa3LightboxTransition::Prepared(submission))
    }
}

/// Lightbox state-machine failure.
#[derive(Debug, PartialEq, Eq)]
pub enum Koa3LightboxControllerError<E> {
    /// A second transition was attempted before presentation completed.
    TransitionPending,
    /// Presentation completion was reported without a prepared request.
    NoPendingTransition,
    /// The target rejected the request unexpectedly.
    Submit(E),
}

impl<E: std::fmt::Display> std::fmt::Display for Koa3LightboxControllerError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TransitionPending => formatter.write_str("KOA3 lightbox transition is pending"),
            Self::NoPendingTransition => {
                formatter.write_str("KOA3 lightbox has no pending transition")
            }
            Self::Submit(error) => write!(formatter, "KOA3 lightbox submission failed: {error}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for Koa3LightboxControllerError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Submit(error) => Some(error),
            Self::TransitionPending | Self::NoPendingTransition => None,
        }
    }
}

const _: () = {
    assert!(std::mem::size_of::<Koa3LightboxRequest>() == 36);
    assert!(std::mem::align_of::<Koa3LightboxRequest>() == 4);
    assert!(std::mem::offset_of!(Koa3LightboxRequest, first_top) == 0);
    assert!(std::mem::offset_of!(Koa3LightboxRequest, first_left) == 4);
    assert!(std::mem::offset_of!(Koa3LightboxRequest, first_width) == 8);
    assert!(std::mem::offset_of!(Koa3LightboxRequest, first_height) == 12);
    assert!(std::mem::offset_of!(Koa3LightboxRequest, second_top) == 16);
    assert!(std::mem::offset_of!(Koa3LightboxRequest, second_left) == 20);
    assert!(std::mem::offset_of!(Koa3LightboxRequest, second_width) == 24);
    assert!(std::mem::offset_of!(Koa3LightboxRequest, second_height) == 28);
    assert!(std::mem::offset_of!(Koa3LightboxRequest, mode) == 32);
};

#[cfg(test)]
mod tests {
    use super::*;
    use ferrink_platform::{DisplayExtent, RefreshRegion};

    #[derive(Debug, Default)]
    struct FakeTarget {
        requests: Vec<Koa3LightboxRequest>,
        fail: bool,
    }

    impl Koa3LightboxTarget for FakeTarget {
        type Error = &'static str;

        fn submit_lightbox(
            &mut self,
            request: Koa3LightboxRequest,
        ) -> Result<Koa3LightboxSubmission, Self::Error> {
            if self.fail {
                return Err("rejected");
            }
            self.requests.push(request);
            Ok(Koa3LightboxSubmission::ExpectedInvalidArgument)
        }
    }

    fn drawer() -> RefreshRegion {
        RefreshRegion::try_new(
            0,
            0,
            1_264,
            732,
            DisplayExtent::try_new(1_264, 1_680).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn request_layout_matches_the_captured_stock_field_order() {
        assert_eq!(KOA3_APPLY_HALFTONE, 0x4024_464b);
        assert_eq!(
            Koa3LightboxRequest::encode(Koa3LightboxState::Foreground(drawer())).words(),
            [0, 0, 1_264, 732, 0, 0, 0, 0, 1]
        );
        assert_eq!(
            Koa3LightboxRequest::encode(Koa3LightboxState::Clear).words(),
            [0, 0, 0, 0, 0, 0, 0, 0, 1]
        );
    }

    #[test]
    fn transition_commits_only_after_the_full_frame() {
        let mut controller = Koa3LightboxController::default();
        let mut target = FakeTarget::default();
        let desired = Koa3LightboxState::Foreground(drawer());

        assert_eq!(
            controller.prepare(&mut target, desired),
            Ok(Koa3LightboxTransition::Prepared(
                Koa3LightboxSubmission::ExpectedInvalidArgument
            ))
        );
        assert_eq!(controller.presented(), Koa3LightboxState::Clear);
        assert!(controller.has_pending_transition());
        assert_eq!(
            controller.prepare(&mut target, desired),
            Err(Koa3LightboxControllerError::TransitionPending)
        );

        controller.commit_presented().unwrap();
        assert_eq!(controller.presented(), desired);
        assert!(!controller.has_pending_transition());
        assert_eq!(
            controller.prepare(&mut target, desired),
            Ok(Koa3LightboxTransition::Unchanged)
        );
        assert_eq!(target.requests.len(), 1);
    }

    #[test]
    fn explicit_handoff_clear_is_idempotent() {
        let mut controller = Koa3LightboxController::default();
        let mut target = FakeTarget::default();
        controller
            .prepare(&mut target, Koa3LightboxState::Foreground(drawer()))
            .unwrap();
        controller.commit_presented().unwrap();

        assert!(matches!(
            controller.clear_for_handoff(&mut target),
            Ok(Koa3LightboxTransition::Prepared(_))
        ));
        assert_eq!(controller.presented(), Koa3LightboxState::Clear);
        assert_eq!(
            controller.clear_for_handoff(&mut target),
            Ok(Koa3LightboxTransition::Unchanged)
        );
        assert_eq!(target.requests.len(), 2);
    }

    #[test]
    fn unexpected_target_error_does_not_create_pending_state() {
        let mut controller = Koa3LightboxController::default();
        let mut target = FakeTarget {
            fail: true,
            ..FakeTarget::default()
        };
        assert_eq!(
            controller.prepare(&mut target, Koa3LightboxState::Foreground(drawer())),
            Err(Koa3LightboxControllerError::Submit("rejected"))
        );
        assert_eq!(controller.presented(), Koa3LightboxState::Clear);
        assert!(!controller.has_pending_transition());
    }
}
