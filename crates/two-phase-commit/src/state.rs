//! Shared protocol decisions, states, and errors.

/// A participant's response to a prepare request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Vote {
    /// The participant is prepared to commit.
    Yes,
    /// The participant cannot commit, so the coordinator must abort.
    No,
}

/// The coordinator's final transaction outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    /// All configured participants voted yes.
    Commit,
    /// At least one participant voted no.
    Abort,
}

/// A participant's local progress through the protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParticipantState {
    /// No prepare request has been processed.
    Working,
    /// The participant voted yes and awaits the coordinator's decision.
    Prepared,
    /// The participant applied commit.
    Committed,
    /// The participant voted no or applied abort.
    Aborted,
}

/// The coordinator's progress while collecting votes and after deciding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoordinatorState {
    /// At least one vote is still outstanding and no no-vote has arrived.
    Collecting,
    /// The transaction outcome is fixed and cannot change.
    Decided(Decision),
}

/// An invalid or contradictory protocol message or transition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    /// The vote came from an identity not configured for this transaction.
    UnknownParticipant(String),
    /// The participant sent a different vote from its earlier vote.
    ConflictingVote,
    /// The requested decision contradicts the participant's applied outcome.
    ConflictingDecision,
    /// The participant has not prepared and cannot apply a decision.
    InvalidTransition,
}
