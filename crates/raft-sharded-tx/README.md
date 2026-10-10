# Two shards with real local Raft groups

This crate contains a small educational sharded transaction participant and a child-process Raft node executable. The [Topcoat portal](../topcoat-portal/README.md) launches six children: source nodes 1–3 and destination nodes 4–6. Each group is an independent, real OpenRaft 0.9.25 three-voter cluster using bounded, length-prefixed JSON RPC on its own OS-assigned `127.0.0.1` port.

## Modules

- `src/model.rs`: Prepare/Decide commands, participant reply types, transaction outcomes, and OpenRaft type configuration.
- `src/storage.rs`: OpenRaft 0.9.25 storage-v2 log and state-machine traits, snapshots, account balances, prepared account locks, idempotent outcomes, and applied-state events. This store is deliberately **volatile**.
- `src/client.rs`: bounded JSON admin and Raft request framing shared with the supervisor.
- `src/cluster.rs`: test/supervisor helpers for child launch, group initialization, leader discovery, and writes.
- `src/main.rs`: one node process, TCP listener, and OpenRaft peer/admin RPC handling.
- `tests/real_cluster.rs`: six actual child processes form both groups, elect leaders, and replicate Prepare entries to follower state machines.

## Protocol surface

Each shard owns one account, initialized to 100 units at the source or 20 at the destination. A Prepare reserves that account for one transaction and checks the proposed post-update balance. YES is returned only after OpenRaft commits and applies the command through a quorum. Any actual NO vote leads the coordinator to ABORT. A timeout or missing vote does not lead to ABORT: the coordinator remains undecided and any prior YES stays prepared. A transaction COMMIT command is rejected unless that participant has a replicated prepared intent. Opposite decisions are rejected, repeated matching decisions are idempotent, and duplicate Prepare messages do not double-apply a transfer.

OpenRaft guarantees quorum replication for a shard while that shard has a majority. It does not make a 2PC transaction atomic across the two independent shards by itself. The coordinator's immutable phase-two decision is written by the portal's separate sync'd journal before either shard receives it.

## Run / tests

The portal starts the node processes; for normal operation run the workspace portal as documented in the root README. For a lower-level one-group E2E test:

```sh
cargo +1.98.0 test -p raft-sharded-tx --offline real_cluster -- --nocapture
```

`real_cluster` verifies six separate child processes (not simulated in-memory nodes), two initialized groups, elected leaders, two Prepare quorum writes, and follower application in both groups. Portal tests additionally verify commit/abort/recovery, membership promotion/removal, and a genuine process kill that leaves two voters able to commit.

## Limitations

All OpenRaft storage is an in-process memory store. Killing a node discards its log and state; this project intentionally does not restart it under the same node ID or imply recovery. Replacements are new IDs that join as learners and are promoted only after catch-up. State machine snapshots are transmitted through real Raft RPCs but snapshots are also in memory. There is no persistent Raft store, authentication, external network exposure, coordinator consensus, sharded transaction consensus, or production availability guarantee. Keep the portal local to the machine running the demo.
