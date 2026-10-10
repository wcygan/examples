use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use two_phase_commit::{Coordinator, Decision, Participant, ParticipantState, Vote};

const PARTICIPANTS: [&str; 2] = ["source-account", "destination-account"];
const INITIAL_BALANCES: [i64; 2] = [100, 20];
type VoteReceiver = mpsc::Receiver<(String, Vote)>;

#[derive(Clone, Copy)]
pub enum Fault {
    None,
    NoVote,
    MissingVote,
    CoordinatorLoss,
    PartialDelivery,
    Recovery,
}

pub struct Scenario;

impl Scenario {
    pub fn new() -> Self {
        Self
    }

    pub fn run(self, fault: Fault) -> Result<(), String> {
        let mut coordinator = Coordinator::new(PARTICIPANTS.map(str::to_owned));
        let (mut participants, votes) = prepare_participants(fault)?;

        if report_undecided_case(fault, &mut coordinator, &participants, &votes)? {
            return Ok(());
        }

        let decision = decide(&mut coordinator, &votes)?;
        verify_immutable_decision(&mut coordinator, decision)?;
        let balances = deliver_decision(fault, &mut participants, decision)?;
        print_result(&participants, balances, decision);
        Ok(())
    }
}

fn prepare_participants(fault: Fault) -> Result<(Vec<Participant>, VoteReceiver), String> {
    let (sender, receiver) = mpsc::channel();
    let handles: Vec<_> = PARTICIPANTS
        .into_iter()
        .map(|id| {
            let sender = sender.clone();
            let can_prepare = !matches!(fault, Fault::NoVote) || id != "destination-account";
            thread::spawn(move || {
                let mut participant = Participant::new(id);
                let vote = participant.prepare(can_prepare);
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
) -> Result<bool, String> {
    if !matches!(fault, Fault::MissingVote | Fault::CoordinatorLoss) {
        return Ok(false);
    }

    let (id, vote) = votes
        .recv_timeout(Duration::from_secs(2))
        .map_err(|error| error.to_string())?;
    record_vote(coordinator, &id, vote)?;
    assert!(coordinator.decision().is_none());
    assert_eq!(participants.len(), PARTICIPANTS.len());
    assert!(participants
        .iter()
        .all(|participant| participant.state() == ParticipantState::Prepared));

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

fn decide(coordinator: &mut Coordinator, votes: &VoteReceiver) -> Result<Decision, String> {
    while let Ok((id, vote)) = votes.try_recv() {
        record_vote(coordinator, &id, vote)?;
    }
    let decision = coordinator.decision().ok_or("expected a decision")?;
    println!("DECISION {decision:?}");
    Ok(decision)
}

fn record_vote(coordinator: &mut Coordinator, id: &str, vote: Vote) -> Result<(), String> {
    coordinator
        .record_vote(id, vote)
        .map(|_| ())
        .map_err(|error| format!("{error:?}"))
}

fn verify_immutable_decision(
    coordinator: &mut Coordinator,
    decision: Decision,
) -> Result<(), String> {
    let late_vote = coordinator
        .record_vote("source-account", Vote::No)
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(late_vote, Some(decision));
    assert_eq!(coordinator.decision(), Some(decision));
    println!("IMMUTABLE decision remains {decision:?} after late NO");
    Ok(())
}

fn deliver_decision(
    fault: Fault,
    participants: &mut [Participant],
    decision: Decision,
) -> Result<[i64; 2], String> {
    let mut balances = INITIAL_BALANCES;
    for (index, participant) in participants.iter_mut().enumerate() {
        if matches!(fault, Fault::PartialDelivery) && index == 1 {
            println!(
                "INJECT partial decision delivery: {} has not received decision",
                participant.id
            );
            continue;
        }
        apply_to_participant(participant, decision, index, &mut balances)?;
    }

    if matches!(fault, Fault::PartialDelivery | Fault::Recovery) {
        print_partial_state(participants, balances);
        let before_retry = balances;
        println!("RECOVERY resend immutable {decision:?} decision");
        for (index, participant) in participants.iter_mut().enumerate() {
            apply_to_participant(participant, decision, index, &mut balances)?;
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
) -> Result<(), String> {
    let was_prepared = participant.state() == ParticipantState::Prepared;
    participant
        .apply(decision)
        .map_err(|error| format!("{error:?}"))?;
    if decision == Decision::Commit && was_prepared {
        balances[account_index] += if account_index == 0 { -10 } else { 10 };
    }
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
