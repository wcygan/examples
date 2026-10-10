//! Participant state machine for two-phase commit.

use crate::state::{Decision, ParticipantState, ProtocolError, Vote};

/// In-memory state for one participant in a two-phase commit transaction.
#[derive(Debug)]
pub struct Participant {
    /// The participant identity used by the coordinator to validate votes.
    pub id: String,
    state: ParticipantState,
    vote: Option<Vote>,
}
impl Participant {
    /// Creates a participant that has not yet received a prepare request.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            state: ParticipantState::Working,
            vote: None,
        }
    }
    /// Returns this participant's local protocol state.
    pub fn state(&self) -> ParticipantState {
        self.state
    }
    /// Returns the vote cast during preparation, if any.
    pub fn vote(&self) -> Option<Vote> {
        self.vote
    }
    /// Votes on whether this participant can prepare its local update.
    ///
    /// Repeated calls return the outcome implied by the existing state.
    pub fn prepare(&mut self, can_prepare: bool) -> Vote {
        match self.state {
            ParticipantState::Working => {
                let vote = if can_prepare { Vote::Yes } else { Vote::No };
                self.vote = Some(vote);
                self.state = if vote == Vote::Yes {
                    ParticipantState::Prepared
                } else {
                    ParticipantState::Aborted
                };
                vote
            }
            ParticipantState::Prepared => Vote::Yes,
            ParticipantState::Aborted => Vote::No,
            ParticipantState::Committed => Vote::Yes,
        }
    }
    /// A timeout is not a decision. A prepared participant stays prepared and blocked.
    pub fn on_timeout(&self) -> ParticipantState {
        self.state
    }
    /// Applies a coordinator decision, allowing duplicate matching delivery.
    ///
    /// Returns an error if the participant is still working or the decision
    /// conflicts with an outcome it has already applied.
    pub fn apply(&mut self, decision: Decision) -> Result<(), ProtocolError> {
        let next = match (self.state, decision) {
            (ParticipantState::Prepared, Decision::Commit) => ParticipantState::Committed,
            (ParticipantState::Prepared, Decision::Abort) => ParticipantState::Aborted,
            (ParticipantState::Aborted, Decision::Abort) => ParticipantState::Aborted,
            (ParticipantState::Committed, Decision::Commit) => ParticipantState::Committed,
            (ParticipantState::Aborted, Decision::Commit)
            | (ParticipantState::Committed, Decision::Abort) => {
                return Err(ProtocolError::ConflictingDecision)
            }
            (ParticipantState::Working, _) => return Err(ProtocolError::InvalidTransition),
        };
        self.state = next;
        Ok(())
    }
}
