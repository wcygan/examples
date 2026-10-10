//! Small, in-memory two-phase commit state machines for teaching.
//! This crate deliberately makes no durability or high-availability claim.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Vote {
    Yes,
    No,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    Commit,
    Abort,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParticipantState {
    Working,
    Prepared,
    Committed,
    Aborted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoordinatorState {
    Collecting,
    Decided(Decision),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    UnknownParticipant(String),
    ConflictingVote,
    ConflictingDecision,
    InvalidTransition,
}

#[derive(Debug)]
pub struct Participant {
    pub id: String,
    state: ParticipantState,
    vote: Option<Vote>,
}
impl Participant {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            state: ParticipantState::Working,
            vote: None,
        }
    }
    pub fn state(&self) -> ParticipantState {
        self.state
    }
    pub fn vote(&self) -> Option<Vote> {
        self.vote
    }
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

#[derive(Debug)]
pub struct Coordinator {
    participants: Vec<String>,
    votes: BTreeMap<String, Vote>,
    state: CoordinatorState,
}
impl Coordinator {
    pub fn new(participants: impl IntoIterator<Item = String>) -> Self {
        let mut distinct = Vec::new();
        for participant in participants {
            if !distinct.contains(&participant) {
                distinct.push(participant);
            }
        }
        Self {
            participants: distinct,
            votes: BTreeMap::new(),
            state: CoordinatorState::Collecting,
        }
    }
    pub fn state(&self) -> CoordinatorState {
        self.state
    }
    pub fn record_vote(&mut self, id: &str, vote: Vote) -> Result<Option<Decision>, ProtocolError> {
        if !self.participants.iter().any(|p| p == id) {
            return Err(ProtocolError::UnknownParticipant(id.to_owned()));
        }
        if self.state != CoordinatorState::Collecting {
            return match self.state {
                CoordinatorState::Decided(d) => Ok(Some(d)),
                CoordinatorState::Collecting => unreachable!(),
            };
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
        if self.votes.len() == self.participants.len()
            && self.votes.values().all(|v| *v == Vote::Yes)
        {
            self.state = CoordinatorState::Decided(Decision::Commit);
            return Ok(Some(Decision::Commit));
        }
        Ok(None)
    }
    /// Missing votes and coordinator loss leave outcome undecided; no timeout-based commit.
    pub fn timeout(&self) -> CoordinatorState {
        self.state
    }
    pub fn decision(&self) -> Option<Decision> {
        match self.state {
            CoordinatorState::Collecting => None,
            CoordinatorState::Decided(d) => Some(d),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn coordinator() -> Coordinator {
        Coordinator::new(["a".into(), "b".into()])
    }
    #[test]
    fn all_yes_commits_only_after_last_vote() {
        let mut c = coordinator();
        assert_eq!(c.record_vote("a", Vote::Yes).unwrap(), None);
        assert_eq!(c.decision(), None);
        assert_eq!(
            c.record_vote("b", Vote::Yes).unwrap(),
            Some(Decision::Commit)
        );
    }
    #[test]
    fn no_vote_aborts_immediately_and_immutably() {
        let mut c = coordinator();
        assert_eq!(c.record_vote("a", Vote::No).unwrap(), Some(Decision::Abort));
        assert_eq!(
            c.record_vote("b", Vote::Yes).unwrap(),
            Some(Decision::Abort)
        );
        assert_eq!(c.decision(), Some(Decision::Abort));
    }
    #[test]
    fn duplicate_participant_configuration_is_treated_as_one_identity() {
        let mut c = Coordinator::new(["a".into(), "a".into(), "b".into()]);
        assert_eq!(c.record_vote("a", Vote::Yes).unwrap(), None);
        assert_eq!(
            c.record_vote("b", Vote::Yes).unwrap(),
            Some(Decision::Commit)
        );
    }
    #[test]
    fn duplicate_yes_is_counted_once_and_cannot_fabricate_unanimity() {
        let mut c = coordinator();
        assert_eq!(c.record_vote("a", Vote::Yes).unwrap(), None);
        assert_eq!(c.record_vote("a", Vote::Yes).unwrap(), None);
        assert_eq!(c.decision(), None);
        assert_eq!(
            c.record_vote("b", Vote::Yes).unwrap(),
            Some(Decision::Commit)
        );
    }
    #[test]
    fn missing_vote_does_not_decide_on_timeout() {
        let mut c = coordinator();
        c.record_vote("a", Vote::Yes).unwrap();
        assert_eq!(c.timeout(), CoordinatorState::Collecting);
        assert_eq!(c.decision(), None);
    }
    #[test]
    fn prepared_participant_blocks_on_timeout_and_retries_idempotently() {
        let mut p = Participant::new("a");
        assert_eq!(p.prepare(true), Vote::Yes);
        assert_eq!(p.on_timeout(), ParticipantState::Prepared);
        assert_eq!(p.prepare(false), Vote::Yes);
        p.apply(Decision::Commit).unwrap();
        p.apply(Decision::Commit).unwrap();
        assert_eq!(p.state(), ParticipantState::Committed);
        assert_eq!(
            p.apply(Decision::Abort),
            Err(ProtocolError::ConflictingDecision)
        );
    }
    #[test]
    fn only_listed_participants_can_vote() {
        assert_eq!(
            coordinator().record_vote("intruder", Vote::Yes),
            Err(ProtocolError::UnknownParticipant("intruder".into()))
        );
    }
}
