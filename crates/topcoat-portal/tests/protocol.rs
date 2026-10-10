use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use topcoat_portal::{Fault, PortalRuntime, TransferRequest, TransferResult};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "raft-twophase-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn raft_binary() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("raft-sharded-tx")
}
async fn wait_prepared(rt: &PortalRuntime, tx: &str) {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let s = rt.status().await;
            if s.prepared_source.contains(&tx.to_owned())
                && s.prepared_destination.contains(&tx.to_owned())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn coordinator_safety_failures_retries_and_membership_are_end_to_end() {
    let temp = Temp::new();
    let bin = raft_binary();
    let rt = PortalRuntime::start(bin.to_str().unwrap(), temp.0.join("decisions.jsonl"))
        .await
        .unwrap();
    let commit = rt
        .transfer(TransferRequest {
            tx_id: Some("commit-1".into()),
            amount: 10,
            fault: Fault::None,
        })
        .await;
    assert_eq!(commit, TransferResult::CommitAcknowledged);
    let before = rt.status().await;
    assert_eq!(
        (before.source_balance, before.destination_balance),
        (90, 30)
    );
    assert_eq!(
        before.decisions.get("commit-1"),
        Some(&raft_sharded_tx::model::Decision::Commit)
    );
    assert_eq!(
        rt.transfer(TransferRequest {
            tx_id: Some("commit-1".into()),
            amount: 10,
            fault: Fault::None
        })
        .await,
        TransferResult::CommitAcknowledged
    );
    let dup = rt.status().await;
    assert_eq!((dup.source_balance, dup.destination_balance), (90, 30));
    assert_eq!(
        rt.transfer(TransferRequest {
            tx_id: Some("no-vote".into()),
            amount: 10,
            fault: Fault::NoVote
        })
        .await,
        TransferResult::AbortAcknowledged
    );
    let abort = rt.status().await;
    assert_eq!(
        abort.decisions.get("no-vote"),
        Some(&raft_sharded_tx::model::Decision::Abort)
    );
    assert_eq!((abort.source_balance, abort.destination_balance), (90, 30));
    assert!(matches!(
        rt.transfer(TransferRequest {
            tx_id: Some("partial".into()),
            amount: 5,
            fault: Fault::PartialDecisionDelivery
        })
        .await,
        TransferResult::PartialDecisionDelivery { .. }
    ));
    let p = rt.status().await;
    assert_eq!((p.source_balance, p.destination_balance), (85, 30));
    assert_eq!(
        p.decisions.get("partial"),
        Some(&raft_sharded_tx::model::Decision::Commit)
    );
    assert_eq!(
        rt.recover("partial").await,
        TransferResult::CommitAcknowledged
    );
    let p = rt.status().await;
    assert_eq!((p.source_balance, p.destination_balance), (85, 35));
    let paused = rt.clone();
    let task = tokio::spawn(async move {
        paused
            .transfer(TransferRequest {
                tx_id: Some("paused".into()),
                amount: 2,
                fault: Fault::PauseAfterPrepare,
            })
            .await
    });
    wait_prepared(&rt, "paused").await;
    let s = rt.status().await;
    assert!(!s.decisions.contains_key("paused"));
    rt.resume().await;
    assert_eq!(task.await.unwrap(), TransferResult::CommitAcknowledged);
    let missing = rt
        .transfer(TransferRequest {
            tx_id: Some("missing".into()),
            amount: 1,
            fault: Fault::MissingVote,
        })
        .await;
    assert!(matches!(missing, TransferResult::InDoubt { .. }));
    let s = rt.status().await;
    assert!(!s.decisions.contains_key("missing"));
    assert!(s.prepared_source.contains(&"missing".to_owned()));
    assert!(s.active.contains_key("missing"));
    assert!(rt.add_node("source").await.is_err());
    assert!(rt.remove_node(1).await.is_err());
    let log = fs::read_to_string(temp.0.join("decisions.jsonl")).unwrap();
    assert!(log.contains("\"Commit\""));
    assert!(log.contains("\"Abort\""));
    let (replay, _, gap) = rt.subscribe(Some(0)).await;
    assert!(!gap);
    assert!(replay.iter().any(|e| e.kind == "decision_durable"));
    assert!(replay.iter().any(|e| e.kind == "participant_applied"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn replayed_commit_is_not_acknowledged_by_fresh_volatile_participants() {
    let temp = Temp::new();
    let bin = raft_binary();
    let journal = temp.0.join("decisions.jsonl");
    let rt = PortalRuntime::start(bin.to_str().unwrap(), journal.clone())
        .await
        .unwrap();
    assert_eq!(
        rt.transfer(TransferRequest {
            tx_id: Some("restart-commit".into()),
            amount: 7,
            fault: Fault::None,
        })
        .await,
        TransferResult::CommitAcknowledged
    );
    rt.stop_all().await;

    // The coordinator journal survives, but participant Raft stores are intentionally volatile.
    let restarted = PortalRuntime::start(bin.to_str().unwrap(), journal)
        .await
        .unwrap();
    assert!(matches!(
        restarted.recover("restart-commit").await,
        TransferResult::DecisionDurableDeliveryUncertain { .. }
    ));
    let status = restarted.status().await;
    assert_eq!(
        status.decisions.get("restart-commit"),
        Some(&raft_sharded_tx::model::Decision::Commit)
    );
    assert_eq!(
        (status.source_balance, status.destination_balance),
        (100, 20)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn learner_addition_and_voter_removal_use_real_membership_changes() {
    let temp = Temp::new();
    let bin = raft_binary();
    let rt = PortalRuntime::start(bin.to_str().unwrap(), temp.0.join("decisions.jsonl"))
        .await
        .unwrap();
    assert_eq!(
        rt.transfer(TransferRequest {
            tx_id: Some("before-membership".into()),
            amount: 2,
            fault: Fault::None
        })
        .await,
        TransferResult::CommitAcknowledged
    );
    let added = rt.add_node("source").await.unwrap();
    assert_eq!(added, 7);
    assert_eq!(rt.status().await.source_nodes.len(), 4);
    rt.remove_node(added).await.unwrap();
    assert_eq!(rt.status().await.source_nodes.len(), 3);
    let id = rt.crash_follower("destination").await.unwrap();
    let tx = format!("after-kill-{id}");
    assert_eq!(
        rt.transfer(TransferRequest {
            tx_id: Some(tx),
            amount: 2,
            fault: Fault::None
        })
        .await,
        TransferResult::CommitAcknowledged
    );
}
