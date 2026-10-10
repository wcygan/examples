# Rust systems examples

This repository is a virtual Cargo workspace for isolated, runnable systems examples. Add each new example as a crate under `crates/`; the root has no application target.

## Workspace map

| Crate | Purpose | Dependencies |
|---|---|---|
| [`two-phase-commit`](crates/two-phase-commit/README.md) | Small hand-rolled 2PC protocol and deterministic std-thread/channel failure harness | Rust standard library only, including tests |
