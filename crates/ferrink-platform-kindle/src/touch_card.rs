//! Host-testable semantics for one bounded, coordinate-redacted touch read.

#![deny(unsafe_code)]

use ferrink_platform::{LogicalTouchPhase, ResolvedRuntimeDevice, TouchContactEvent};

/// Single-use identifier for the first KOA3 touch-read card.
pub const KOA3_TOUCH_READ_CARD_ID: &str = "koa3-touch-read-v1";

const MAXIMUM_MOVES: u32 = 1_024;

/// Bounded evidence from one complete primary touch sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TouchReadPass {
    move_count: u32,
}

impl TouchReadPass {
    /// Returns the number of synchronized move classifications observed.
    #[must_use]
    pub const fn move_count(self) -> u32 {
        self.move_count
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TouchReadState {
    Waiting,
    Active { move_count: u32 },
    Complete { move_count: u32 },
    Finished,
}

/// One exact, single-use primary touch-sequence classifier.
///
/// Coordinates are deliberately inspected only by the already validated
/// transform and are not retained by this card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Koa3TouchReadCard {
    state: TouchReadState,
}

impl Koa3TouchReadCard {
    /// Constructs the card only for the reviewed KOA3 runtime profile.
    ///
    /// # Errors
    ///
    /// Rejects every other profile before accepting a contact.
    pub fn try_from_runtime(device: &ResolvedRuntimeDevice) -> Result<Self, TouchReadCardError> {
        if device.profile_id() != "reference-portrait" {
            return Err(TouchReadCardError::WrongProfile);
        }
        Ok(Self {
            state: TouchReadState::Waiting,
        })
    }

    /// Applies one already decoded and transformed contact batch atomically.
    ///
    /// # Errors
    ///
    /// Rejects malformed phase order, a second contact sequence, excessive
    /// move classifications, or use after finalization without changing state.
    pub fn observe(&mut self, contacts: &[TouchContactEvent]) -> Result<(), TouchReadCardError> {
        let mut state = self.state;
        for contact in contacts {
            state = match (state, contact.phase) {
                (TouchReadState::Waiting, LogicalTouchPhase::Pressed) => {
                    TouchReadState::Active { move_count: 0 }
                }
                (TouchReadState::Active { move_count }, LogicalTouchPhase::Moved) => {
                    let move_count = move_count
                        .checked_add(1)
                        .ok_or(TouchReadCardError::TooManyMoves)?;
                    if move_count > MAXIMUM_MOVES {
                        return Err(TouchReadCardError::TooManyMoves);
                    }
                    TouchReadState::Active { move_count }
                }
                (TouchReadState::Active { move_count }, LogicalTouchPhase::Released) => {
                    TouchReadState::Complete { move_count }
                }
                (TouchReadState::Complete { .. }, _) => {
                    return Err(TouchReadCardError::ExtraContact);
                }
                (TouchReadState::Finished, _) => {
                    return Err(TouchReadCardError::AlreadyFinished);
                }
                (
                    TouchReadState::Waiting,
                    LogicalTouchPhase::Moved | LogicalTouchPhase::Released,
                )
                | (TouchReadState::Active { .. }, LogicalTouchPhase::Pressed) => {
                    return Err(TouchReadCardError::InvalidPhaseOrder);
                }
            };
        }
        self.state = state;
        Ok(())
    }

    /// Returns whether exactly one press/release sequence is complete.
    #[must_use]
    pub const fn is_complete(self) -> bool {
        matches!(self.state, TouchReadState::Complete { .. })
    }

    /// Finalizes the card exactly once without exposing touch coordinates.
    ///
    /// # Errors
    ///
    /// Returns if no complete sequence exists or the card was already
    /// finalized.
    pub fn finish(&mut self) -> Result<TouchReadPass, TouchReadCardError> {
        let TouchReadState::Complete { move_count } = self.state else {
            return Err(match self.state {
                TouchReadState::Finished => TouchReadCardError::AlreadyFinished,
                TouchReadState::Waiting | TouchReadState::Active { .. } => {
                    TouchReadCardError::Incomplete
                }
                TouchReadState::Complete { .. } => unreachable!(),
            });
        };
        self.state = TouchReadState::Finished;
        Ok(TouchReadPass { move_count })
    }
}

/// Failure in the single-touch, coordinate-redacted card policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TouchReadCardError {
    /// The runtime is not the exact reviewed KOA3 profile.
    WrongProfile,
    /// A move/release preceded press, or a second press preceded release.
    InvalidPhaseOrder,
    /// More than one contact sequence appeared in the bounded stream.
    ExtraContact,
    /// The sequence exceeded its fixed synchronized-move bound.
    TooManyMoves,
    /// The card ended before one press/release sequence completed.
    Incomplete,
    /// Finalization or observation was attempted after finalization.
    AlreadyFinished,
}

impl std::fmt::Display for TouchReadCardError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongProfile => {
                formatter.write_str("touch-read card requires exact KOA3 profile")
            }
            Self::InvalidPhaseOrder => formatter.write_str("touch phases were out of order"),
            Self::ExtraContact => formatter.write_str("touch-read card observed a second contact"),
            Self::TooManyMoves => formatter.write_str("touch-read move bound was exceeded"),
            Self::Incomplete => formatter.write_str("touch sequence did not complete"),
            Self::AlreadyFinished => formatter.write_str("touch-read card is already finished"),
        }
    }
}

impl std::error::Error for TouchReadCardError {}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrink_platform::{DeviceProfile, DisplayPoint, ProbeReport};

    const KOA3_REPORT: &str =
        include_str!("../../ferrink-platform/tests/fixtures/probe-reference-portrait.json");
    const KOA3_PROFILE: &str = include_str!("../../../device-profiles/reference-portrait.toml");
    const PW1_REPORT: &str =
        include_str!("../../ferrink-platform/tests/fixtures/probe-reference-landscape.json");
    const PW1_PROFILE: &str = include_str!("../../../device-profiles/reference-landscape.toml");

    fn runtime(profile: &str, report: &str) -> ResolvedRuntimeDevice {
        let profile = DeviceProfile::from_toml(profile).unwrap();
        let report = ProbeReport::from_json(report).unwrap();
        ResolvedRuntimeDevice::resolve(&profile, &report).unwrap()
    }

    fn contact(phase: LogicalTouchPhase) -> TouchContactEvent {
        TouchContactEvent {
            phase,
            point: DisplayPoint { x: 632, y: 840 },
        }
    }

    #[test]
    fn one_complete_sequence_records_only_a_move_count() {
        let mut card =
            Koa3TouchReadCard::try_from_runtime(&runtime(KOA3_PROFILE, KOA3_REPORT)).unwrap();
        card.observe(&[
            contact(LogicalTouchPhase::Pressed),
            contact(LogicalTouchPhase::Moved),
            contact(LogicalTouchPhase::Moved),
            contact(LogicalTouchPhase::Released),
        ])
        .unwrap();

        assert!(card.is_complete());
        assert_eq!(card.finish().unwrap().move_count(), 2);
        assert_eq!(card.finish(), Err(TouchReadCardError::AlreadyFinished));
    }

    #[test]
    fn invalid_or_extra_phases_fail_atomically() {
        let runtime = runtime(KOA3_PROFILE, KOA3_REPORT);
        let mut card = Koa3TouchReadCard::try_from_runtime(&runtime).unwrap();
        assert_eq!(
            card.observe(&[contact(LogicalTouchPhase::Released)]),
            Err(TouchReadCardError::InvalidPhaseOrder)
        );
        assert!(!card.is_complete());

        card.observe(&[
            contact(LogicalTouchPhase::Pressed),
            contact(LogicalTouchPhase::Released),
        ])
        .unwrap();
        assert_eq!(
            card.observe(&[contact(LogicalTouchPhase::Pressed)]),
            Err(TouchReadCardError::ExtraContact)
        );
        assert!(card.is_complete());
    }

    #[test]
    fn incomplete_and_wrong_profile_stay_closed() {
        let mut card =
            Koa3TouchReadCard::try_from_runtime(&runtime(KOA3_PROFILE, KOA3_REPORT)).unwrap();
        card.observe(&[contact(LogicalTouchPhase::Pressed)])
            .unwrap();
        assert_eq!(card.finish(), Err(TouchReadCardError::Incomplete));
        assert_eq!(
            Koa3TouchReadCard::try_from_runtime(&runtime(PW1_PROFILE, PW1_REPORT)),
            Err(TouchReadCardError::WrongProfile)
        );
    }
}
