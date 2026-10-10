use raft_sharded_tx::{
    cluster::{NodeProcess, initialize_group, leader, metrics, write},
    model::{ApplyResult, Command},
};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn six_child_processes_form_two_real_three_node_raft_groups() {
    let bin = env!("CARGO_BIN_EXE_raft-sharded-tx");
    let mut source = vec![];
    let mut destination = vec![];
    for id in 1..=3 {
        source.push(NodeProcess::spawn(bin, id, "source").await.unwrap());
    }
    for id in 4..=6 {
        destination.push(NodeProcess::spawn(bin, id, "destination").await.unwrap());
    }
    initialize_group(&source).await.unwrap();
    initialize_group(&destination).await.unwrap();
    let s_leader = leader(&source).await.unwrap();
    let d_leader = leader(&destination).await.unwrap();
    let tx = "integration-transfer".to_owned();
    assert_eq!(
        write(
            &s_leader,
            Command::Prepare {
                tx_id: tx.clone(),
                account: "source-account".into(),
                delta: -15
            }
        )
        .await
        .unwrap(),
        ApplyResult::Prepared {
            yes: true,
            reason: None
        }
    );
    assert_eq!(
        write(
            &d_leader,
            Command::Prepare {
                tx_id: tx.clone(),
                account: "destination-account".into(),
                delta: 15
            }
        )
        .await
        .unwrap(),
        ApplyResult::Prepared {
            yes: true,
            reason: None
        }
    );
    let sm = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let m = metrics(&source).await;
            if m.len() == 3 && m.values().all(|x| x.last_applied.is_some()) {
                break m;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(sm.len(), 3);
}
