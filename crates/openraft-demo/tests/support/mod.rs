//! Safety checks over immutable observations, independent of scenario outcomes.
use openraft_demo::audit::{Event, Kind};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

// A function, rather than a macro, keeps failures readable in generated cases.
fn check(condition: bool, sequence: u64, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(format!("observation {sequence}: {message}"))
    }
}
pub fn safety(events: &[Event], count: usize) -> Result<(), String> {
    let mut committed = BTreeMap::<u64, Value>::new();
    let mut elected = BTreeMap::<u64, u64>::new();
    let mut applied = BTreeMap::<u64, Value>::new();
    for (index, event) in events.iter().enumerate() {
        check(
            event.sequence == index as u64 + 1,
            event.sequence,
            "collector sequence gap",
        )?;
        check(
            event.nodes.len() == count
                && (1..=count as u64).all(|id| event.nodes.contains_key(&id)),
            event.sequence,
            "wrong cluster membership",
        )?;
        check(
            event.nodes.contains_key(&event.actor),
            event.sequence,
            "unknown actor",
        )?;
        transport(events, index)?;
        monotonic(events, index)?;
        application(events, index)?;
        log_matching(event)?;
        leader(event, &mut elected, &committed)?;
        if matches!(event.kind, Kind::Committed | Kind::ClientAcknowledged) {
            quorum(event, count)?;
            let actor = &event.nodes[&event.actor];
            if let Some(last) = actor.committed {
                for (index, entry) in actor.log.range(..=last) {
                    if let Some(previous) = committed.insert(*index, entry.clone()) {
                        check(
                            previous == *entry,
                            event.sequence,
                            "committed prefix changed",
                        )?;
                    }
                }
            }
        }
        if event.kind == Kind::Applied {
            let index = event.payload["log_id"]["index"]
                .as_u64()
                .ok_or("missing applied log ID")?;
            if let Some(previous) = applied.insert(index, event.payload.clone()) {
                check(
                    previous == event.payload,
                    event.sequence,
                    "nodes applied different entries at one index",
                )?;
            }
            check(
                event.nodes[&event.actor].log.get(&index) == Some(&event.payload),
                event.sequence,
                "applied entry does not match local log",
            )?;
        }
        for node in event.nodes.values() {
            check(
                node.applied <= node.committed,
                event.sequence,
                "application ahead of commit",
            )?;
            for (index, entry) in &committed {
                if node.committed.is_some_and(|last| last >= *index) {
                    check(
                        node.log.get(index) == Some(entry),
                        event.sequence,
                        "committed logs disagree",
                    )?;
                }
            }
        }
    }
    Ok(())
}
fn leader(
    event: &Event,
    elected: &mut BTreeMap<u64, u64>,
    committed: &BTreeMap<u64, Value>,
) -> Result<(), String> {
    if event.kind == Kind::Metrics && event.payload["state"] == "Leader" {
        let term = event.payload["current_term"]
            .as_u64()
            .ok_or("missing election term")?;
        let leader = elected.entry(term).or_insert(event.actor);
        check(
            *leader == event.actor,
            event.sequence,
            "two elected leaders in one term",
        )?;
        for (index, entry) in committed {
            check(
                event.nodes[&event.actor].log.get(index) == Some(entry),
                event.sequence,
                "new leader lacks previously committed entry",
            )?;
        }
    }
    Ok(())
}
fn application(events: &[Event], index: usize) -> Result<(), String> {
    let event = &events[index];
    if event.kind != Kind::Applied {
        return Ok(());
    }
    let mut expected = if index > 0 {
        events[index - 1].nodes[&event.actor].values.clone()
    } else {
        BTreeMap::new()
    };
    if let Some(command) = event.payload["payload"].get("Normal") {
        let key = command["key"].as_str().ok_or("missing applied key")?;
        let value = command["value"].as_str().ok_or("missing applied value")?;
        expected.insert(key.into(), value.into());
    }
    check(
        event.nodes[&event.actor].values == expected,
        event.sequence,
        "state machine differs from applied command",
    )
}
fn log_matching(event: &Event) -> Result<(), String> {
    for (id, a) in &event.nodes {
        for b in event.nodes.range((id + 1)..).map(|(_, node)| node) {
            for (index, entry) in &a.log {
                if b.log
                    .get(index)
                    .is_some_and(|other| other["log_id"] == entry["log_id"])
                {
                    check(
                        a.log.range(..=index).eq(b.log.range(..=index)),
                        event.sequence,
                        "matching log IDs have different prefixes",
                    )?;
                }
            }
        }
    }
    Ok(())
}
fn quorum(event: &Event, count: usize) -> Result<(), String> {
    let actor = &event.nodes[&event.actor];
    let index = if event.kind == Kind::ClientAcknowledged {
        Some(
            event.payload["log_id"]["index"]
                .as_u64()
                .ok_or("acknowledgement lacks log ID")?,
        )
    } else {
        actor.committed
    };
    let Some(index) = index else {
        return Ok(());
    };
    let entry = actor.log.get(&index).ok_or("committed entry absent")?;
    let replicas = event
        .nodes
        .values()
        .filter(|n| n.log.get(&index) == Some(entry))
        .count();
    check(
        replicas > count / 2,
        event.sequence,
        "commit/ack without majority replication",
    )?;
    if event.kind == Kind::ClientAcknowledged {
        check(
            actor.applied.is_some_and(|last| last >= index),
            event.sequence,
            "acknowledgement before application",
        )?;
    }
    Ok(())
}
fn monotonic(events: &[Event], index: usize) -> Result<(), String> {
    let event = &events[index];
    if event.kind == Kind::VoteSaved {
        let term = event.payload["leader_id"]["term"]
            .as_u64()
            .ok_or("missing saved vote term")?;
        if let Some(previous) = events[..index]
            .iter()
            .rev()
            .find(|e| e.kind == Kind::VoteSaved && e.actor == event.actor)
        {
            check(
                Some(term) >= previous.payload["leader_id"]["term"].as_u64(),
                event.sequence,
                "saved vote term regressed",
            )?;
        }
    }
    if index == 0 {
        return Ok(());
    }
    for (id, node) in &event.nodes {
        let before = &events[index - 1].nodes[id];
        check(
            node.committed >= before.committed,
            event.sequence,
            "commit index regressed",
        )?;
        check(
            node.applied >= before.applied,
            event.sequence,
            "applied index regressed",
        )?;
        if event.kind == Kind::Applied && *id == event.actor {
            let expected = before.applied.map_or(0, |i| i + 1);
            check(
                node.applied == Some(expected),
                event.sequence,
                "application skipped or repeated an index",
            )?;
        }
        if node.values != before.values {
            check(
                *id == event.actor && matches!(event.kind, Kind::Applied | Kind::SnapshotInstalled),
                event.sequence,
                "values changed outside committed application",
            )?;
        }
        if let Some(last) = before.committed {
            for (i, entry) in before.log.range(..=last) {
                check(
                    node.log.get(i) == Some(entry),
                    event.sequence,
                    "local committed prefix overwritten",
                )?;
            }
        }
    }
    Ok(())
}
fn transport(events: &[Event], index: usize) -> Result<(), String> {
    let event = &events[index];
    if !matches!(event.kind, Kind::Received | Kind::ResponseReceived) {
        return Ok(());
    }
    let kind = if event.kind == Kind::Received {
        Kind::Sent
    } else {
        Kind::ResponseSent
    };
    let matches: Vec<_> = events[..index]
        .iter()
        .filter(|e| {
            e.kind == kind
                && e.message_id == event.message_id
                && e.peer == Some(event.actor)
                && Some(e.actor) == event.peer
        })
        .collect();
    check(
        event.message_id.is_some() && matches.len() == 1,
        event.sequence,
        "receipt without exactly one actual send",
    )?;
    check(
        matches[0].payload == event.payload,
        event.sequence,
        "RPC payload changed during delivery",
    )?;
    check(
        !events[..index]
            .iter()
            .any(|e| e.kind == event.kind && e.message_id == event.message_id),
        event.sequence,
        "duplicate receipt",
    )
}
pub fn outcome(events: &[Event], scenario: &str, count: usize) {
    assert_eq!(events.last().unwrap().kind, Kind::Finished);
    let nodes = &events.last().unwrap().nodes;
    let expected = match scenario {
        "leader-loss" => "after election",
        "catch-up" => "while isolated",
        _ => "first",
    };
    for (id, node) in nodes {
        assert_eq!(
            node.values["lesson"],
            if scenario == "leader-loss" && *id == 1 {
                "first"
            } else {
                expected
            }
        );
    }
    if scenario == "quorum-loss" {
        assert!(events.iter().any(|e| e.kind == Kind::ClientTimeout));
        assert!(events.iter().all(|e| e
            .nodes
            .values()
            .all(|n| n.values.get("lesson").is_none_or(|v| v != "without quorum"))));
        assert!(events
            .iter()
            .filter(|e| e.kind == Kind::ClientAcknowledged)
            .all(|e| e.nodes[&e.actor].values["lesson"] == "first"));
    }
    if scenario == "leader-loss" {
        assert!(!nodes[&1].available);
        assert_eq!(nodes[&2].role, "Leader");
        assert!(nodes[&2].term > nodes[&1].term);
    }
    if scenario == "catch-up" {
        assert!(events.iter().any(|e| e.nodes[&(count as u64)].isolated
            && e.nodes[&1]
                .values
                .get("lesson")
                .is_some_and(|v| v == expected)
            && e.nodes[&(count as u64)]
                .values
                .get("lesson")
                .is_some_and(|v| v == "first")));
    }
    let ids: BTreeSet<_> = events
        .iter()
        .filter(|e| e.kind == Kind::Sent)
        .map(|e| e.message_id.unwrap())
        .collect();
    assert_eq!(
        ids.len(),
        events.iter().filter(|e| e.kind == Kind::Sent).count()
    );
}
