//! Coordinator state machine for two-phase commit.

use crate::state::{CoordinatorState, Decision, ProtocolError, Vote};
use std::collections::{BTreeMap, BTreeSet};

/// In-memory coordinator that collects one vote per configured participant.
#[derive(Debug)]
pub struct Coordinator {
    participants: BTreeSet<String>,
    votes: BTreeMap<String, Vote>,
    state: CoordinatorState,
}
impl Coordinator {
    /// Creates a coordinator; duplicate participant identities count only once.
    pub fn new(participants: impl IntoIterator<Item = String>) -> Self {
        Self {
            participants: participants.into_iter().collect(),
            votes: BTreeMap::new(),
            state: CoordinatorState::Collecting,
        }
    }
    /// Returns the current coordinator state.
    pub fn state(&self) -> CoordinatorState {
        self.state
    }
    /// Records a participant's vote and returns a decision once one is fixed.
    ///
    /// Duplicate matching votes are harmless. A no-vote aborts immediately;
    /// commit requires a yes-vote from every distinct configured identity.
    /// Votes from unknown identities and contradictory duplicate votes fail.
    pub fn record_vote(&mut self, id: &str, vote: Vote) -> Result<Option<Decision>, ProtocolError> {
        if !self.participants.contains(id) {
            return Err(ProtocolError::UnknownParticipant(id.to_owned()));
        }
        if let CoordinatorState::Decided(decision) = self.state {
            return Ok(Some(decision));
        }
        if let Some(previous) = self.votes.get(id) {
            if *previous != vote {
                return Err(ProtocolError::ConflictingVote);
            }
        } else {
            self.votes.insert(id.to_owned(), vote);
        }
        if vote == Vote::No {
            self.state = CoordinatorState::Decided(Decision::Abort);
            return Ok(Some(Decision::Abort));
        }
        // A no-vote decides abort immediately, so a full vote set is unanimous.
        if self.votes.len() == self.participants.len() {
            self.state = CoordinatorState::Decided(Decision::Commit);
            return Ok(Some(Decision::Commit));
        }
        Ok(None)
    }
    /// Reports state after a timeout; missing votes never imply a decision.
    ///
    /// Coordinator loss therefore leaves the outcome undecided.
    pub fn timeout(&self) -> CoordinatorState {
        self.state
    }
    /// Returns the immutable decision, or `None` while votes are outstanding.
    pub fn decision(&self) -> Option<Decision> {
        match self.state {
            CoordinatorState::Collecting => None,
            CoordinatorState::Decided(d) => Some(d),
        }
    }
}
