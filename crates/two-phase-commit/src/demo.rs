use crate::events::{EventKind as E, Observer, ProtocolMessage as M, Snapshot};
use crate::{Coordinator, Decision, Participant, ParticipantState, Vote};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

type VoteReceiver = mpsc::Receiver<(String, Vote)>;

#[derive(Clone, Copy, Debug)]
pub enum Fault {
    None,
    NoVote,
    MissingVote,
    CoordinatorLoss,
    PartialDelivery,
    Recovery,
}

pub struct Scenario {
    participant_ids: Vec<String>,
}

impl Default for Scenario {
    fn default() -> Self {
        Self::new()
    }
}

impl Fault {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "commit" => Some(Self::None),
            "no-vote" => Some(Self::NoVote),
            "missing-vote" => Some(Self::MissingVote),
            "coordinator-loss" => Some(Self::CoordinatorLoss),
            "partial-delivery" => Some(Self::PartialDelivery),
            "recovery" => Some(Self::Recovery),
            _ => None,
        }
    }
}

impl Scenario {
    pub fn new() -> Self {
        Self {
            participant_ids: ["source-account", "destination-account"]
                .map(str::to_owned)
                .to_vec(),
        }
    }

    pub fn with_participants(count: usize) -> Result<Self, String> {
        if !(2..=5).contains(&count) {
            return Err("Participant count must be between 2 and 5".into());
        }
        Ok(Self {
            participant_ids: (1..=count).map(|i| format!("participant-{i}")).collect(),
        })
    }

    pub fn run(self, fault: Fault) -> Result<(), String> {
        self.run_observed(fault, Observer::quiet())
    }

    pub fn run_observed(self, fault: Fault, observer: Observer) -> Result<(), String> {
        let initial = Snapshot::for_participants(self.participant_ids.clone());
        let balances = initial.nodes.iter().map(|node| node.balance).collect();
        observer.emit(
            E::Started,
            "coordinator",
            format!(
                "Start {} scenario",
                match fault {
                    Fault::None => "commit",
                    Fault::NoVote => "no-vote",
                    Fault::MissingVote => "missing-vote",
                    Fault::CoordinatorLoss => "coordinator-loss",
                    Fault::PartialDelivery => "partial-delivery",
                    Fault::Recovery => "recovery",
                }
            ),
            None,
            |snapshot| *snapshot = initial,
        );
        let mut coordinator = Coordinator::new(self.participant_ids.clone());
        let (mut participants, votes) =
            prepare_participants(fault, &self.participant_ids, &observer)?;

        if report_undecided_case(fault, &mut coordinator, &participants, &votes, &observer)? {
            observer.emit(
                E::Finished,
                "coordinator",
                "Scenario ended with blocked participants",
                None,
                |_| {},
            );
            return Ok(());
        }

        let decision = decide(&mut coordinator, &votes, &observer)?;
        let balances = deliver_decision(fault, &mut participants, decision, balances, &observer)?;
        print_result(&participants, &balances, decision);
        observer.emit(
            E::Finished,
            "coordinator",
            "Scenario complete",
            None,
            |_| {},
        );
        Ok(())
    }
}

fn prepare_participants(
    fault: Fault,
    ids: &[String],
    observer: &Observer,
) -> Result<(Vec<Participant>, VoteReceiver), String> {
    let (sender, receiver) = mpsc::channel();
    let handles: Vec<_> = ids
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, id)| {
            let sender = sender.clone();
            let observer = observer.clone();
            let prepare_id = format!("prepare-{id}");
            observer.emit(
                E::PrepareSent,
                "coordinator",
                "Send PREPARE",
                Some((&prepare_id, &id, M::Prepare)),
                |_| {},
            );
            let can_prepare = !matches!(fault, Fault::NoVote) || index != ids.len() - 1;
            thread::spawn(move || {
                let mut participant = Participant::new(id.clone());
                let vote = participant.prepare(can_prepare);
                observer.emit(
                    E::Prepared,
                    &id,
                    format!("Prepare received; vote {vote:?}"),
                    Some((&prepare_id, "coordinator", M::Prepare)),
                    |s| {
                        let node = s.nodes.iter_mut().find(|n| n.id == id).unwrap();
                        node.state = participant.state();
                        node.vote = Some(vote);
                    },
                );
                let message_id = format!("vote-{id}");
                observer.emit(
                    E::VoteSent,
                    &id,
                    format!("Send {vote:?}"),
                    Some((&message_id, "coordinator", M::Vote(vote))),
                    |_| {},
                );
                sender
                    .send((id.to_owned(), vote))
                    .map_err(|error| error.to_string())?;
                Ok::<_, String>(participant)
            })
        })
        .collect();
    drop(sender);

    let participants = handles
        .into_iter()
        .map(|handle| {
            handle
                .join()
                .map_err(|_| "participant thread panicked".to_owned())?
        })
        .collect::<Result<_, _>>()?;
    Ok((participants, receiver))
}

fn report_undecided_case(
    fault: Fault,
    coordinator: &mut Coordinator,
    participants: &[Participant],
    votes: &VoteReceiver,
    observer: &Observer,
) -> Result<bool, String> {
    if !matches!(fault, Fault::MissingVote | Fault::CoordinatorLoss) {
        return Ok(false);
    }

    let received = if matches!(fault, Fault::MissingVote) {
        participants.len() - 1
    } else {
        1
    };
    for _ in 0..received {
        let (id, vote) = votes
            .recv_timeout(Duration::from_secs(2))
            .map_err(|error| error.to_string())?;
        record_vote(coordinator, &id, vote, observer)?;
    }
    assert!(coordinator.decision().is_none());
    assert!(participants
        .iter()
        .all(|participant| participant.state() == ParticipantState::Prepared));

    // The harness intercepts remaining queued votes before coordinator processing.
    for (index, (id, vote)) in votes.try_iter().enumerate() {
        withhold_vote(fault, &id, vote, index == 0, observer);
    }
    let prepared_after_timeout = participants
        .iter()
        .filter(|participant| participant.on_timeout() == ParticipantState::Prepared)
        .count();
    assert_eq!(prepared_after_timeout, participants.len());
    observer.emit(
        E::TimeoutObserved,
        "coordinator",
        "Simulated timeout; no decision, so prepared participants remain blocked",
        None,
        |s| {
            for node in &mut s.nodes {
                node.blocked = node.state == ParticipantState::Prepared;
            }
        },
    );
    if matches!(fault, Fault::CoordinatorLoss) {
        println!("INJECT coordinator loss after one YES; votes=1/{}; prepared_after_timeout={prepared_after_timeout}; decision=None", participants.len());
    } else {
        println!("INJECT missing vote before decision; votes={received}/{}; decision=None; prepared={prepared_after_timeout}", participants.len());
    }
    Ok(true)
}

fn withhold_vote(fault: Fault, id: &str, vote: Vote, first: bool, observer: &Observer) {
    let message_id = format!("vote-{id}");
    let loss = matches!(fault, Fault::CoordinatorLoss);
    observer.emit(
        E::FaultInjected,
        "coordinator",
        if loss && first {
            format!("Simulated coordinator loss after one YES; vote from {id} not delivered")
        } else {
            format!(
                "Withhold YES from {id}; coordinator {}",
                if loss {
                    "unavailable"
                } else {
                    "remains available"
                }
            )
        },
        Some((&message_id, id, M::Vote(vote))),
        |snapshot| snapshot.coordinator_available = !loss,
    );
}

fn decide(
    coordinator: &mut Coordinator,
    votes: &VoteReceiver,
    observer: &Observer,
) -> Result<Decision, String> {
    while let Ok((id, vote)) = votes.try_recv() {
        record_vote(coordinator, &id, vote, observer)?;
    }
    let decision = coordinator.decision().ok_or("expected a decision")?;
    println!("DECISION {decision:?}");
    Ok(decision)
}

fn record_vote(
    coordinator: &mut Coordinator,
    id: &str,
    vote: Vote,
    observer: &Observer,
) -> Result<(), String> {
    let before = coordinator.decision();
    coordinator
        .record_vote(id, vote)
        .map_err(|error| format!("{error:?}"))?;
    let message_id = format!("vote-{id}");
    observer.emit(
        E::VoteReceived,
        "coordinator",
        format!("Receive {vote:?} from {id}"),
        Some((&message_id, id, M::Vote(vote))),
        |s| {
            s.votes_received += 1;
            s.coordinator = coordinator.state();
        },
    );
    if before != coordinator.decision() {
        observer.emit(
            E::DecisionMade,
            "coordinator",
            format!("Decision fixed: {:?}", coordinator.decision().unwrap()),
            None,
            |s| {
                s.coordinator = coordinator.state();
            },
        );
    }
    Ok(())
}

fn deliver_decision(
    fault: Fault,
    participants: &mut [Participant],
    decision: Decision,
    mut balances: Vec<i64>,
    observer: &Observer,
) -> Result<Vec<i64>, String> {
    let last = participants.len() - 1;
    for (index, participant) in participants.iter_mut().enumerate() {
        if matches!(fault, Fault::PartialDelivery) && index == last {
            let message_id = format!("decision-{}", participant.id);
            observer.emit(
                E::FaultInjected,
                "coordinator",
                format!("Withhold decision delivery to {}", participant.id),
                Some((&message_id, &participant.id, M::Decision(decision))),
                |s| {
                    s.nodes[index].blocked = true;
                },
            );
            println!(
                "INJECT partial decision delivery: {} has not received decision",
                participant.id
            );
            continue;
        }
        apply_to_participant(participant, decision, index, &mut balances, observer, false)?;
    }

    if matches!(fault, Fault::PartialDelivery | Fault::Recovery) {
        print_partial_state(participants, &balances);
        let before_retry = balances.clone();
        println!("RECOVERY resend immutable {decision:?} decision");
        for (index, participant) in participants.iter_mut().enumerate() {
            apply_to_participant(participant, decision, index, &mut balances, observer, true)?;
        }
        print_recovery_result(fault, participants, &before_retry, &balances);
    }
    Ok(balances)
}

fn apply_to_participant(
    participant: &mut Participant,
    decision: Decision,
    account_index: usize,
    balances: &mut [i64],
    observer: &Observer,
    retry: bool,
) -> Result<(), String> {
    let message_id = format!("decision-{}", participant.id);
    let duplicate = retry && participant.state() != ParticipantState::Prepared;
    observer.emit(
        if duplicate {
            E::DuplicateDelivery
        } else {
            E::DecisionSent
        },
        "coordinator",
        format!("{} {decision:?}", if retry { "Resend" } else { "Send" }),
        Some((&message_id, &participant.id, M::Decision(decision))),
        |_| {},
    );
    let was_prepared = participant.state() == ParticipantState::Prepared;
    participant
        .apply(decision)
        .map_err(|error| format!("{error:?}"))?;
    if decision == Decision::Commit && was_prepared {
        balances[account_index] += if account_index == 0 {
            -10 * (balances.len() as i64 - 1)
        } else {
            10
        };
    }
    observer.emit(
        E::DecisionApplied,
        &participant.id,
        format!(
            "Apply {decision:?}{}",
            if duplicate { " (idempotent)" } else { "" }
        ),
        Some((&message_id, "coordinator", M::Decision(decision))),
        |s| {
            s.nodes[account_index].state = participant.state();
            s.nodes[account_index].balance = balances[account_index];
            s.nodes[account_index].blocked = false;
        },
    );
    Ok(())
}

fn print_partial_state(participants: &[Participant], balances: &[i64]) {
    println!("PARTIAL STATES {}", format_states(participants));
    print_balances("PARTIAL BALANCES", participants, balances);
}

fn print_recovery_result(
    fault: Fault,
    participants: &[Participant],
    before_retry: &[i64],
    balances: &[i64],
) {
    if matches!(fault, Fault::Recovery) {
        assert_eq!(balances, before_retry);
        println!(
            "IDEMPOTENT balances unchanged by duplicate decision: {}",
            format_balances(participants, balances)
        );
    } else {
        println!(
            "RECOVERED balances {}",
            format_balances(participants, balances)
        );
    }
}

fn print_result(participants: &[Participant], balances: &[i64], decision: Decision) {
    println!("STATES {}", format_states(participants));
    print_balances("BALANCES", participants, balances);
    match decision {
        Decision::Commit => println!("RESULT committed atomically across demo participants"),
        Decision::Abort => println!("RESULT aborted"),
    }
}

fn format_states(participants: &[Participant]) -> String {
    participants
        .iter()
        .map(|participant| format!("{}={:?}", participant.id, participant.state()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_balances(participants: &[Participant], balances: &[i64]) -> String {
    participants
        .iter()
        .zip(balances)
        .map(|(participant, balance)| {
            let name = match participant.id.as_str() {
                "source-account" => "source",
                "destination-account" => "destination",
                id => id,
            };
            format!("{name}={balance}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}
fn print_balances(label: &str, participants: &[Participant], balances: &[i64]) {
    println!("{label} {}", format_balances(participants, balances));
}
