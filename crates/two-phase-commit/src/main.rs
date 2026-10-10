mod demo;

use demo::{Fault, Scenario};

fn main() {
    let fault = match std::env::args().nth(1).as_deref().unwrap_or("commit") {
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

    if let Err(error) = Scenario::new().run(fault) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
