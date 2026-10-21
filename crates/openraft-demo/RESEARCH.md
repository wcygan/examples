# Raft demo library choice

Researched 2026-10-12. Recommendation: **OpenRaft 0.9.25** for this local
teaching console. This is an integration choice, not a performance ranking.

| Concern | OpenRaft 0.9.25 | raft-rs 0.7.0 |
| --- | --- | --- |
| Application integration | Async Raft handle; application supplies network, log storage and state machine | Application drives RawNode ticks, incoming messages and Ready processing |
| Observation points | RPC hooks, storage mutations, committed application, metrics | Explicit message, persistence and application processing in the caller's loop |
| Best teaching fit | A running cluster with an independent visual replay | A manually stepped consensus engine |
| Work required here | In-memory storage and instrumented transport | Those components plus more of the consensus runtime driver |

OpenRaft's pinned guide describes the network/storage interfaces and committed
state-machine application. Its metrics expose term, leader, role, applied index
and replication progress. That fits the workspace's Tokio and Topcoat console.
[Getting started](https://docs.rs/openraft/0.9.25/openraft/docs/getting_started/index.html),
[metrics](https://docs.rs/openraft/0.9.25/openraft/metrics/struct.RaftMetrics.html).

raft-rs is also a suitable implementation. Its caller owns the tick/step/Ready
loop, including arranging persistence and dispatch. That control is attractive
for a deterministic single-step teaching tool; the extra integration work is
why it is not selected for this first console.
[raft-rs API](https://docs.rs/raft/0.7.0/raft/),
[official five-node example](https://github.com/tikv/raft-rs/blob/v0.7.0/examples/five_mem_node/main.rs).

## Two logs with different purposes

The replicated Raft log is **not immutable**: conflicting uncommitted entries
can be truncated, and older entries can be purged after snapshotting. OpenRaft
explicitly requires these storage operations.
[Storage contracts](https://docs.rs/openraft/0.9.25/openraft/storage/trait.RaftLogStorage.html).

The demo therefore maintains a separate append-only observation stream. Every
event has a collector sequence, actor, elapsed time, typed kind, recorded payload,
RPC correlation ID when applicable, and an observed cluster snapshot. Replay
restores those snapshots without executing consensus again. A send and its actual
receipt share an ID; dropped attempts have no invented receipt.
[Network contracts](https://docs.rs/openraft/0.9.25/openraft/network/trait.RaftNetwork.html).

Metrics watch updates may coalesce; they are observations, not a complete trace
of every internal engine transition. Storage and transport boundaries provide
the detailed trace. Collector order is local observation order, not a global
causal clock for independent machines.

## Initial scope and limitations

Three to five real OpenRaft instances run in one process, with an instrumented in-process
transport and volatile storage. Scenarios exercise replication, leader loss,
quorum loss and follower catch-up. Automatic elections are disabled so the
harness can trigger the election relevant to each lesson; OpenRaft still runs
the election and replication protocol. The animation does not delay consensus.

History is retained only for the server session. Append-only does not mean
tamper-proof, durable, or cryptographically authenticated. A client timeout is
an unknown write outcome, not a transaction abort. Independent network processes,
restart recovery, membership changes and snapshot-transfer scenarios are future
work; the storage snapshot interfaces are implemented.

OpenRaft's current main branch has newer APIs. This crate deliberately uses the
workspace's exact 0.9.25 pin, rather than mixing main-branch examples with that
release. Both libraries leave production durability to their integrations.
[OpenRaft repository](https://github.com/databendlabs/openraft),
[raft-rs repository](https://github.com/tikv/raft-rs).
