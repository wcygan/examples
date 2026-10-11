use crate::events::{EventKind as E, Observer, ProtocolMessage as M};
use crate::{Coordinator, Decision, Participant, ParticipantState, Vote};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const PARTICIPANTS: [&str; 2] = ["source-account", "destination-account"];
const INITIAL_BALANCES: [i64; 2] = [100, 20];
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

pub struct Scenario;

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
        Self
    }

    pub fn run(self, fault: Fault) -> Result<(), String> {
        self.run_observed(fault, Observer::quiet())
    }

    pub fn run_observed(self, fault: Fault, observer: Observer) -> Result<(), String> {
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
            |_| {},
        );
        let mut coordinator = Coordinator::new(PARTICIPANTS.map(str::to_owned));
        let (mut participants, votes) = prepare_participants(fault, &observer)?;

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
        verify_immutable_decision(&mut coordinator, decision, &observer)?;
        let balances = deliver_decision(fault, &mut participants, decision, &observer)?;
        print_result(&participants, balances, decision);
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
    observer: &Observer,
) -> Result<(Vec<Participant>, VoteReceiver), String> {
    let (sender, receiver) = mpsc::channel();
    let handles: Vec<_> = PARTICIPANTS
        .into_iter()
        .map(|id| {
            let sender = sender.clone();
            let observer = observer.clone();
            let prepare_id = format!("prepare-{id}");
            observer.emit(
                E::PrepareSent,
                "coordinator",
                "Send PREPARE",
                Some((&prepare_id, id, M::Prepare)),
                |_| {},
            );
            let can_prepare = !matches!(fault, Fault::NoVote) || id != "destination-account";
            thread::spawn(move || {
                let mut participant = Participant::new(id);
                let vote = participant.prepare(can_prepare);
                observer.emit(
                    E::Prepared,
                    id,
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
                    id,
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

    let (id, vote) = votes
        .recv_timeout(Duration::from_secs(2))
        .map_err(|error| error.to_string())?;
    record_vote(coordinator, &id, vote, observer)?;
    assert!(coordinator.decision().is_none());
    assert_eq!(participants.len(), PARTICIPANTS.len());
    assert!(participants
        .iter()
        .all(|participant| participant.state() == ParticipantState::Prepared));

    observer.emit(
        E::FaultInjected,
        "coordinator",
        if matches!(fault, Fault::CoordinatorLoss) {
            "Simulated coordinator loss after one YES"
        } else {
            "Withhold second vote from coordinator"
        },
        None,
        |s| {
            s.coordinator_available = !matches!(fault, Fault::CoordinatorLoss);
        },
    );
    observer.emit(
        E::TimeoutObserved,
        "coordinator",
        "No decision; prepared participants remain blocked",
        None,
        |s| {
            for node in &mut s.nodes {
                node.blocked = node.state == ParticipantState::Prepared;
            }
        },
    );
    if matches!(fault, Fault::CoordinatorLoss) {
        let prepared_after_timeout = participants
            .iter()
            .filter(|participant| participant.on_timeout() == ParticipantState::Prepared)
            .count();
        assert_eq!(prepared_after_timeout, PARTICIPANTS.len());
        println!("INJECT coordinator loss after one YES; votes=1/2; prepared_after_timeout={prepared_after_timeout}; decision=None");
    } else {
        println!("INJECT missing vote before decision; votes=1/2; decision=None; prepared=2");
    }
    Ok(true)
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

fn verify_immutable_decision(
    coordinator: &mut Coordinator,
    decision: Decision,
    observer: &Observer,
) -> Result<(), String> {
    observer.emit(
        E::VoteSent,
        "source-account",
        "Synthetic late NO to verify immutable decision",
        Some(("late-vote-source-account", "coordinator", M::Vote(Vote::No))),
        |_| {},
    );
    let late_vote = coordinator
        .record_vote("source-account", Vote::No)
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(late_vote, Some(decision));
    assert_eq!(coordinator.decision(), Some(decision));
    observer.emit(
        E::VoteReceived,
        "coordinator",
        format!("Late NO ignored; decision remains {decision:?}"),
        Some((
            "late-vote-source-account",
            "source-account",
            M::Vote(Vote::No),
        )),
        |_| {},
    );
    println!("IMMUTABLE decision remains {decision:?} after late NO");
    Ok(())
}

fn deliver_decision(
    fault: Fault,
    participants: &mut [Participant],
    decision: Decision,
    observer: &Observer,
) -> Result<[i64; 2], String> {
    let mut balances = INITIAL_BALANCES;
    for (index, participant) in participants.iter_mut().enumerate() {
        if matches!(fault, Fault::PartialDelivery) && index == 1 {
            observer.emit(
                E::FaultInjected,
                "coordinator",
                "Withhold decision delivery to destination-account",
                Some((
                    "decision-destination-account",
                    "destination-account",
                    M::Decision(decision),
                )),
                |s| {
                    s.nodes[1].blocked = true;
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
        print_partial_state(participants, balances);
        let before_retry = balances;
        println!("RECOVERY resend immutable {decision:?} decision");
        for (index, participant) in participants.iter_mut().enumerate() {
            apply_to_participant(participant, decision, index, &mut balances, observer, true)?;
        }
        print_recovery_result(fault, before_retry, balances);
    }
    Ok(balances)
}

fn apply_to_participant(
    participant: &mut Participant,
    decision: Decision,
    account_index: usize,
    balances: &mut [i64; 2],
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
        balances[account_index] += if account_index == 0 { -10 } else { 10 };
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

fn print_partial_state(participants: &[Participant], balances: [i64; 2]) {
    println!("PARTIAL STATES {}", format_states(participants));
    print_balances("PARTIAL BALANCES", balances);
}

fn print_recovery_result(fault: Fault, before_retry: [i64; 2], balances: [i64; 2]) {
    if matches!(fault, Fault::Recovery) {
        assert_eq!(balances, before_retry);
        println!(
            "IDEMPOTENT balances unchanged by duplicate decision: source={} destination={}",
            balances[0], balances[1]
        );
    } else {
        assert_eq!(balances, [90, 30]);
        println!(
            "RECOVERED balances source={} destination={}",
            balances[0], balances[1]
        );
    }
}

fn print_result(participants: &[Participant], balances: [i64; 2], decision: Decision) {
    println!("STATES {}", format_states(participants));
    print_balances("BALANCES", balances);
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

fn print_balances(label: &str, balances: [i64; 2]) {
    println!("{label} source={} destination={}", balances[0], balances[1]);
}
