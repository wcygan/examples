use std::sync::{Arc, Mutex};
use std::time::Duration;
use two_phase_commit::demo::{Fault, Scenario};
use two_phase_commit::events::{EventKind, Observer, ProtocolEvent, ProtocolMessage};
use two_phase_commit::{CoordinatorState, Decision, ParticipantState};

fn trace(name: &str) -> Vec<ProtocolEvent> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    Scenario::new()
        .run_observed(
            Fault::parse(name).unwrap(),
            Observer::new(42, Duration::ZERO, move |event| {
                sink.lock().unwrap().push(event)
            }),
        )
        .unwrap();
    Arc::try_unwrap(events).unwrap().into_inner().unwrap()
}
fn assert_message_payload(event: &ProtocolEvent) {
    assert_eq!(event.message_id.is_some(), event.message.is_some());
    match event.kind {
        EventKind::PrepareSent | EventKind::Prepared => {
            assert_eq!(event.message, Some(ProtocolMessage::Prepare))
        }
        EventKind::VoteSent | EventKind::VoteReceived => {
            assert!(matches!(event.message, Some(ProtocolMessage::Vote(_))))
        }
        EventKind::DecisionSent | EventKind::DecisionApplied | EventKind::DuplicateDelivery => {
            assert!(matches!(event.message, Some(ProtocolMessage::Decision(_))))
        }
        _ => {}
    }
}

#[test]
fn all_scenarios_have_ordered_correlated_replayable_observations() {
    for name in [
        "commit",
        "no-vote",
        "missing-vote",
        "coordinator-loss",
        "partial-delivery",
        "recovery",
    ] {
        let events = trace(name);
        assert_eq!(events.first().unwrap().kind, EventKind::Started);
        assert_eq!(events.last().unwrap().kind, EventKind::Finished);
        for (i, event) in events.iter().enumerate() {
            assert_eq!(event.run_id, 42);
            assert_message_payload(event);
            assert_eq!(event.sequence, i as u64 + 1);
            if i > 0 {
                assert!(event.elapsed_ms >= events[i - 1].elapsed_ms);
            }
            if event.kind == EventKind::VoteReceived {
                assert!(events[..i]
                    .iter()
                    .any(|sent| sent.kind == EventKind::VoteSent
                        && sent.message_id == event.message_id));
            }
            let decoded: ProtocolEvent =
                serde_json::from_str(&serde_json::to_string(event).unwrap()).unwrap();
            assert_eq!(
                decoded.snapshot.nodes[0].balance,
                event.snapshot.nodes[0].balance
            );
        }
        let final_state = &events.last().unwrap().snapshot;
        match name {
            "no-vote" => {
                assert_eq!(
                    final_state.coordinator,
                    CoordinatorState::Decided(Decision::Abort)
                );
                assert!(final_state
                    .nodes
                    .iter()
                    .all(|node| node.state == ParticipantState::Aborted));
                assert_eq!(
                    [final_state.nodes[0].balance, final_state.nodes[1].balance],
                    [100, 20]
                );
            }
            "missing-vote" | "coordinator-loss" => {
                assert_eq!(final_state.coordinator, CoordinatorState::Collecting);
                assert_eq!(final_state.votes_received, 1);
                assert!(final_state
                    .nodes
                    .iter()
                    .all(|node| node.blocked && node.state == ParticipantState::Prepared));
                assert_eq!(
                    final_state.coordinator_available,
                    name != "coordinator-loss"
                );
            }
            _ => {
                assert_eq!(
                    final_state.coordinator,
                    CoordinatorState::Decided(Decision::Commit)
                );
                assert_eq!(
                    [final_state.nodes[0].balance, final_state.nodes[1].balance],
                    [90, 30]
                );
                assert!(final_state
                    .nodes
                    .iter()
                    .all(|node| node.state == ParticipantState::Committed));
            }
        }
    }
}
#[test]
fn replay_exposes_partial_delivery_and_idempotent_recovery() {
    let partial = trace("partial-delivery");
    let withheld = partial
        .iter()
        .find(|e| e.kind == EventKind::FaultInjected)
        .unwrap();
    assert_eq!(
        withheld.snapshot.coordinator,
        CoordinatorState::Decided(Decision::Commit)
    );
    assert_eq!(
        withheld.snapshot.nodes[0].state,
        ParticipantState::Committed
    );
    assert_eq!(withheld.snapshot.nodes[1].state, ParticipantState::Prepared);
    assert!(withheld.snapshot.nodes[1].blocked);
    assert_eq!(withheld.snapshot.nodes[1].balance, 20);
    let recovery = trace("recovery");
    let duplicates: Vec<_> = recovery
        .iter()
        .filter(|e| e.kind == EventKind::DuplicateDelivery)
        .collect();
    assert_eq!(duplicates.len(), 2);
    for event in duplicates {
        assert_eq!(
            [
                event.snapshot.nodes[0].balance,
                event.snapshot.nodes[1].balance
            ],
            [90, 30]
        );
    }
}
