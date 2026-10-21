# Rust systems examples

This repository is a virtual Cargo workspace for isolated, runnable systems examples. Add each new example as a crate under `crates/`; the root has no application target.

## Workspace map

| Crate | Purpose | Dependencies |
|---|---|---|
| [`conceptual-database`](crates/conceptual-database/README.md) | In-memory order table with primary, secondary and composite indexes, query work reports and atomic batches | Axum, Tokio and Serde |
| [`two-phase-commit`](crates/two-phase-commit/README.md) | Small hand-rolled 2PC protocol and deterministic std-thread/channel failure harness | Standard-library protocol harness; Topcoat, Tokio and Serde console |
| [`openraft-demo`](crates/openraft-demo/README.md) | Three-node replicated key/value cluster with elections, failure lessons and audit replay | OpenRaft 0.9.25, Topcoat, Tokio and Serde |
