use openraft_demo::{
    audit::Audit,
    cluster::{run, SCENARIOS},
    console::Console,
};
use std::time::Duration;

mod support;
#[tokio::test]
async fn all_counts_and_scenarios_preserve_safety_and_replay() {
    for count in 3..=5 {
        for scenario in SCENARIOS {
            let audit = Audit::with_nodes(0, count).unwrap();
            run(scenario, audit.clone())
                .await
                .unwrap_or_else(|e| panic!("{scenario}/{count}: {e}"));
            let events = audit.events();
            support::safety(&events, count).unwrap_or_else(|e| panic!("{scenario}/{count}: {e}"));
            support::outcome(&events, scenario, count);
            let replay: Vec<openraft_demo::audit::Event> =
                serde_json::from_str(&serde_json::to_string(&events).unwrap()).unwrap();
            assert_eq!(
                serde_json::to_value(replay).unwrap(),
                serde_json::to_value(&events).unwrap()
            );
            let sealed = events.len();
            tokio::time::sleep(Duration::from_millis(30)).await;
            assert_eq!(audit.events().len(), sealed);
        }
    }
}
#[tokio::test]
async fn history_validates_starts_and_retains_completed_runs() {
    let console = Console::default();
    assert!(console.start("unknown").is_err());
    for count in [0, 2, 6, usize::MAX] {
        assert!(console.start_with_nodes("replication", count).is_err());
    }
    let first = console.start("quorum-loss").unwrap();
    assert!(console.start("replication").is_err());
    tokio::time::timeout(Duration::from_secs(8), async {
        while console.run(first).unwrap().status == "running" {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(console.run(first).unwrap().status, "finished");
    let second = console.start("replication").unwrap();
    assert_ne!(first, second);
    assert_eq!(console.runs().len(), 2);
    assert!(!console.run(first).unwrap().events.is_empty());
}
