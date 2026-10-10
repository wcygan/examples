# Rust systems examples

This repository is a virtual Cargo workspace for isolated, runnable systems examples. Add each new example as a crate under `crates/`; the root has no application target.

## Workspace map

| Crate | Purpose | Dependencies |
|---|---|---|
| [`two-phase-commit`](crates/two-phase-commit/README.md) | Small hand-rolled 2PC protocol and deterministic std-thread/channel failure harness | Rust standard library only, including tests |
| [`raft-sharded-tx`](crates/raft-sharded-tx/README.md) | Six child processes running two independent three-node OpenRaft groups | OpenRaft 0.9.25, Tokio, Serde |
| [`topcoat-portal`](crates/topcoat-portal/README.md) | Local Topcoat 0.10 router/SSE interface, coordinator journal, lifecycle controls | Topcoat 0.10.0 router/core, Tokio, plus the first example's runtime dependencies |

The first example stays std-only. Tokio helps the second example handle concurrent local TCP RPC, process supervision, bounded asynchronous waits, and a streaming HTTP interface; it does not provide 2PC safety. OpenRaft supplies Raft replication within each shard only. The coordinator still has no consensus-backed failover.

## Run

Use Rust 1.98.0 (pinned in `rust-toolchain.toml`) for Topcoat 0.10.0.

```sh
# In-process standard-library 2PC examples
cargo +1.98.0 run -p two-phase-commit -- commit
cargo +1.98.0 run -p two-phase-commit -- partial-delivery

# Build the local node binary before starting the portal
cargo +1.98.0 build --workspace --offline
cargo +1.98.0 run -p topcoat-portal -- --port 0 \
  --raft-bin "$PWD/target/debug/raft-sharded-tx" \
  --journal /tmp/two-phase-decisions.jsonl
```

The portal binds an ephemeral loopback port and prints `PORTAL_READY http://127.0.0.1:<port>`. Use the shown URL. It never chooses port 3000 by default. Click the page controls to run transfers, induce a NO or missing vote, pause after prepare, withhold a decision, recover/resend, add/remove voters, or kill one local follower child. Ctrl-C stops the portal and its owned node children. The journal path can be set explicitly as above.

## Verification

Repeatable offline checks from the workspace root:

```sh
cargo +1.98.0 fmt --all -- --check
cargo +1.98.0 clippy --workspace --all-targets --offline -- -D warnings
cargo +1.98.0 test --workspace --offline
```

The tests include the std-only scenario binary's process E2E, a six-child real Raft cluster test, coordinator protocol/failure tests, real learner promotion and membership removal, process-kill quorum survival, the actual Topcoat HTTP binary, and SSE replay headers. The process tests use ephemeral loopback ports and unique temporary directories and stop/reap owned children. In this restricted execution environment, child-process tests may need permission to execute local demo binaries outside the default process sandbox.

## Limits of the sharded example

Each Raft node's OpenRaft log and application state are in memory. The coordinator JSONL journal syncs transaction starts and immutable decisions before phase two; participant prepared promises are not fsync'd. A node killed by the lifecycle control loses its volatile store and is not restarted under the same identity. Adding a replacement uses a fresh ID, learner catch-up, then membership promotion. This example does not claim durable participant recovery, consensus for the coordinator, coordinator failover, distributed transactions across a crash/restart, or production HA. If the coordinator is unavailable while participants are prepared, they block until it returns with a decision. Delivery acknowledgement and durable decision are shown separately from uncertain delivery.

The independent standard-library 2PC example has its own in-memory-only limitations documented in its README.
