# Agent guidance

This repository is a collection of data infrastructure experiments, written entirely in Rust. Keep each experiment isolated and runnable under `crates/`.

The experiments explore concepts from [Database Internals](https://www.databass.dev/) and [Designing Data-Intensive Applications](https://dataintensive.net/).

## Stack and conventions

- **Rust** is the implementation language. The toolchain is pinned in `rust-toolchain.toml`; rustfmt and Clippy are included.
- **Tokio** provides asynchronous tasks, networking, and runtime support where needed. The two-phase commit example deliberately uses only standard-library threads and channels.
- **OpenRaft** provides Raft replication for the sharded transaction example. It does not make the coordinator consensus-backed.
- **bbtree** is the conceptual database's alternative B-tree. Its secondary indexes run on std's `BTreeMap`/`BTreeSet` or on bbtree's `unsync` types, swappable at runtime.
- **Serde and serde_json** handle serialization, including JSON protocol data.
- **Topcoat 0.10** provides the portal's router, HTTP serving, and server-sent events (SSE).
- **futures-util** supports asynchronous stream handling, including the SSE interface.
- **Standard-library process and concurrency APIs** support the examples' local child processes, threads, and channels.
- **Tests** use Rust's built-in test framework, with integration and process-level end-to-end coverage. Tests exercise local processes and ephemeral loopback ports.
- **Builds and tests** are designed to run offline with Cargo after dependencies are available locally.
- **Persistence is intentionally limited in the sharded example:** Raft state is in memory, and the coordinator writes decisions to a JSONL journal.

There are no current Axum, SQLx, or Testcontainers dependencies; add them only when an experiment needs them.
