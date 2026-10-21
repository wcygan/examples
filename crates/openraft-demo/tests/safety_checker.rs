//! Mutation checks ensure the invariant checker is capable of finding defects.
mod support;
use openraft_demo::{
    audit::{Audit, Kind},
    cluster::run,
};

#[tokio::test]
async fn safety_checker_rejects_corrupted_traces() {
    let audit = Audit::default();
    run("replication", audit.clone()).await.unwrap();
    let events = audit.events();
    support::safety(&events, 3).unwrap();
    support::outcome(&events, "replication", 3);
    let mut bad = events.clone();
    bad[1].sequence += 1;
    assert!(support::safety(&bad, 3).unwrap_err().contains("sequence"));
    let mut bad = events.clone();
    let receipt = bad.iter_mut().find(|e| e.kind == Kind::Received).unwrap();
    receipt.message_id = Some(u64::MAX);
    assert!(support::safety(&bad, 3)
        .unwrap_err()
        .contains("actual send"));
    let mut bad = events.clone();
    bad.last_mut().unwrap().nodes.get_mut(&1).unwrap().applied = Some(u64::MAX);
    assert!(support::safety(&bad, 3)
        .unwrap_err()
        .contains("application ahead"));
    let mut bad = events.clone();
    bad.last_mut()
        .unwrap()
        .nodes
        .get_mut(&2)
        .unwrap()
        .values
        .insert("lesson".into(), "invented".into());
    assert!(support::safety(&bad, 3)
        .unwrap_err()
        .contains("values changed"));
    let mut bad = events.clone();
    let boundary = bad
        .iter()
        .position(|e| e.kind == Kind::Committed && e.nodes[&e.actor].committed.is_some())
        .unwrap();
    let actor = bad[boundary].actor;
    let index = bad[boundary].nodes[&actor].committed.unwrap();
    for event in &mut bad[..=boundary] {
        for (id, node) in &mut event.nodes {
            if *id != actor {
                node.log.remove(&index);
            }
        }
    }
    assert!(support::safety(&bad, 3).unwrap_err().contains("majority"));
    let mut bad = events.clone();
    let boundary = bad
        .iter()
        .position(|e| e.kind == Kind::ClientAcknowledged)
        .unwrap();
    let index = bad[boundary].nodes[&1].committed.unwrap();
    bad[boundary]
        .nodes
        .get_mut(&1)
        .unwrap()
        .log
        .insert(index, serde_json::json!({"corrupted": true}));
    assert!(support::safety(&bad, 3).unwrap_err().contains("prefix"));
    let mut bad = events.clone();
    let applied = bad
        .iter_mut()
        .find(|e| e.kind == Kind::Applied && e.payload["payload"].get("Normal").is_some())
        .unwrap();
    applied
        .nodes
        .get_mut(&applied.actor)
        .unwrap()
        .values
        .insert("lesson".into(), "wrong result".into());
    assert!(support::safety(&bad, 3)
        .unwrap_err()
        .contains("applied command"));
    conflicting_leader_is_rejected(&events);
}

fn conflicting_leader_is_rejected(events: &[openraft_demo::audit::Event]) {
    let mut bad = events.to_vec();
    let leader = bad
        .iter()
        .position(|e| e.kind == Kind::Metrics && e.payload["state"] == "Leader")
        .unwrap();
    let term = bad[leader].payload["current_term"].clone();
    let other = bad[leader + 1..]
        .iter_mut()
        .find(|e| e.kind == Kind::Metrics && e.actor == 2 && e.payload["current_term"] == term)
        .unwrap();
    other.payload["state"] = serde_json::json!("Leader");
    assert!(support::safety(&bad, 3)
        .unwrap_err()
        .contains("two elected leaders"));
}
