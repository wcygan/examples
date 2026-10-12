use two_phase_commit::{
    Coordinator, CoordinatorState, Decision, Participant, ParticipantState, ProtocolError, Vote,
};

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
fn commit_remains_immutable_after_a_contradictory_late_vote() {
    let mut c = coordinator();
    c.record_vote("a", Vote::Yes).unwrap();
    c.record_vote("b", Vote::Yes).unwrap();
    assert_eq!(
        c.record_vote("a", Vote::No).unwrap(),
        Some(Decision::Commit)
    );
    assert_eq!(c.decision(), Some(Decision::Commit));
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
