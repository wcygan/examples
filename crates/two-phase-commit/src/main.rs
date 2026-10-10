use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use two_phase_commit::{Coordinator, Decision, Participant, ParticipantState};

#[derive(Clone, Copy)]
enum Fault {
    None,
    NoVote,
    MissingVote,
    CoordinatorLoss,
    PartialDelivery,
    Recovery,
}

fn deliver_decision(
    participant: &mut Participant,
    decision: Decision,
    account_index: usize,
    balances: &mut [i64; 2],
) -> Result<(), String> {
    let before = participant.state();
    participant
        .apply(decision)
        .map_err(|error| format!("{error:?}"))?;
    // Each participant owns one staged side of the transfer. Apply its delta only
    // on the first Prepared -> Committed transition, making duplicate delivery safe.
    if decision == Decision::Commit && before == ParticipantState::Prepared {
        balances[account_index] += if account_index == 0 { -10 } else { 10 };
    }
    Ok(())
}

fn run(fault: Fault) -> Result<(), String> {
    let ids = ["source-account", "destination-account"];
    let mut coordinator = Coordinator::new(ids.iter().map(|s| (*s).to_owned()));
    let (tx, rx) = mpsc::channel();
    let mut joins = Vec::new();
    for id in ids {
        let id = id.to_owned();
        let tx = tx.clone();
        let fail_no = matches!(fault, Fault::NoVote) && id == "destination-account";
        joins.push(thread::spawn(move || {
            let mut p = Participant::new(id.clone());
            let vote = p.prepare(!fail_no);
            tx.send((id, vote)).map_err(|e| e.to_string())?;
            Ok::<_, String>(p)
        }));
    }
    drop(tx);
    let mut participants = Vec::new();
    for handle in joins {
        let participant = handle
            .join()
            .map_err(|_| "participant thread panicked".to_owned())??;
        participants.push(participant);
    }
    if matches!(fault, Fault::MissingVote | Fault::CoordinatorLoss) {
        // Keep both YES participants prepared, but deliver only one vote to the coordinator.
        let (id, vote) = rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|e| e.to_string())?;
        coordinator
            .record_vote(&id, vote)
            .map_err(|e| format!("{e:?}"))?;
        assert!(coordinator.decision().is_none());
        assert_eq!(participants.len(), 2);
        assert!(participants
            .iter()
            .all(|p| p.state() == ParticipantState::Prepared));
        if matches!(fault, Fault::CoordinatorLoss) {
            let still_prepared = participants
                .iter()
                .filter(|p| p.on_timeout() == ParticipantState::Prepared)
                .count();
            assert_eq!(still_prepared, 2);
            println!("INJECT coordinator loss after one YES; votes=1/2; prepared_after_timeout={still_prepared}; decision=None");
        } else {
            println!("INJECT missing vote before decision; votes=1/2; decision=None; prepared=2");
        }
        return Ok(());
    }
    // All normal queued votes are processed; decisions are immutable after first choice.
    while let Ok((id, vote)) = rx.try_recv() {
        coordinator
            .record_vote(&id, vote)
            .map_err(|e| format!("{e:?}"))?;
    }
    let decision = coordinator.decision().ok_or("expected a decision")?;
    println!("DECISION {decision:?}");
    // A delayed message cannot rewrite the coordinator's already fixed outcome.
    let late_vote = coordinator
        .record_vote("source-account", two_phase_commit::Vote::No)
        .map_err(|e| format!("{e:?}"))?;
    assert_eq!(late_vote, Some(decision));
    assert_eq!(coordinator.decision(), Some(decision));
    println!("IMMUTABLE decision remains {decision:?} after late NO");
    // The demo transaction transfers 10 units between two account-like resources.
    // Staged values remain invisible until a COMMIT decision is delivered.
    let mut balances = [100_i64, 20_i64];
    for (idx, p) in participants.iter_mut().enumerate() {
        if matches!(fault, Fault::PartialDelivery) && idx == 1 {
            println!(
                "INJECT partial decision delivery: {} has not received decision",
                p.id
            );
            continue;
        }
        deliver_decision(p, decision, idx, &mut balances)?;
    }
    if matches!(fault, Fault::PartialDelivery | Fault::Recovery) {
        println!(
            "PARTIAL STATES {}",
            participants
                .iter()
                .map(|p| format!("{}={:?}", p.id, p.state()))
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!(
            "PARTIAL BALANCES source={} destination={}",
            balances[0], balances[1]
        );
        let before_retry = balances;
        println!("RECOVERY resend immutable {decision:?} decision");
        for (idx, p) in participants.iter_mut().enumerate() {
            deliver_decision(p, decision, idx, &mut balances)?;
        }
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
    println!(
        "STATES {}",
        participants
            .iter()
            .map(|p| format!("{}={:?}", p.id, p.state()))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "BALANCES source={} destination={}",
        balances[0], balances[1]
    );
    match decision {
        Decision::Commit => println!("RESULT committed atomically across demo participants"),
        Decision::Abort => println!("RESULT aborted"),
    }
    Ok(())
}

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| "commit".into());
    let fault = match arg.as_str() {
        "commit" => Fault::None,
        "no-vote" => Fault::NoVote,
        "missing-vote" => Fault::MissingVote,
        "coordinator-loss" => Fault::CoordinatorLoss,
        "partial-delivery" => Fault::PartialDelivery,
        "recovery" => Fault::Recovery,
        _ => {
            eprintln!("usage: two-phase-commit [commit|no-vote|missing-vote|coordinator-loss|partial-delivery|recovery]");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(fault) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
