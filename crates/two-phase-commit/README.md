# Two-phase commit, in miniature

This crate models one coordinator and two participants (`source-account` and `destination-account`) for an atomic 10-unit transfer (100 to 90 at the source; 20 to 30 at the destination). Its purpose is to make the protocol's state transitions and failure tradeoffs visible, not to provide a reusable transaction manager.

## Structure

- `src/lib.rs`: pure coordinator and participant state machines, explicit states, immutable decisions, idempotent duplicate messages, and unit tests.
- `src/main.rs`: runnable scenario and std thread/channel transport/failure harness.
- `tests/e2e.rs`: starts the compiled executable as a child process for end-to-end scenario checks.

## Phases and safety

1. **Prepare / vote:** coordinator asks participants to prepare. A participant votes YES only if it can prepare its local update; otherwise NO. Any NO fixes the coordinator decision to ABORT.
2. **Decision:** coordinator chooses COMMIT only after YES from every distinct configured participant. A repeated YES from one identity counts once. Missing votes leave the coordinator undecided.
3. **Apply:** participants apply the immutable decision idempotently. Duplicate COMMIT delivery remains COMMIT; contradictory decisions are rejected.

A prepared participant cannot infer ABORT from timeout. It remains prepared and blocked because the coordinator may already have committed. The demo's `coordinator-loss` scenario injects coordinator loss after a YES and shows this blocking condition. `partial-delivery` withholds one decision delivery; `recovery` resends the same decision. These are deterministic in-memory message-boundary injections, not process crashes. Participants can temporarily display different local states while delivery is incomplete, but a decision never changes to the opposite outcome. The coordinator decision remains COMMIT even when a participant has not received it; until delivery is complete the outcome is uncertain at that participant. This demo does not implement acknowledgment messages or a production acknowledgment protocol.

## Run scenarios

```sh
cargo run -p two-phase-commit -- commit
cargo run -p two-phase-commit -- no-vote
cargo run -p two-phase-commit -- missing-vote
cargo run -p two-phase-commit -- coordinator-loss
cargo run -p two-phase-commit -- partial-delivery
cargo run -p two-phase-commit -- recovery
```

## Test layers

- Unit tests cover all-YES commit, NO abort, distinct-identity vote counting, missing-vote timeout, prepared blocking, idempotent retries, immutable outcome, and rejection of unknown participants.
- Integration tests run the actual executable for commit and abort.
- E2E tests invoke scenario binaries with a 3-second deadline and kill/reap cleanup on timeout, reject bad CLI arguments, and repeat success/failure scenarios three times. They assert every participant state, exact balances for commit and abort, intermediate partial-delivery state/balances, recovered state/balances, and that duplicate COMMIT delivery leaves already-updated balances unchanged. No temporary directories or ports are needed.

Repeatable offline verification from workspace root:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo test --workspace --offline
```

## Limitations

All state is in memory. A real crash/restart implementation would require fsync'd participant prepared promises before YES, and a fsync'd coordinator decision before broadcasting it; it would also need recovery and retransmission logic, message identity/epoch handling, and acknowledgment tracking. None is implemented, and no durability is implied. Two-phase commit can block when the coordinator is unavailable after participants prepare. There is one coordinator, no consensus or failover, no real network, no authentication, and no production HA claim. A local std channel is adequate here; Tokio becomes useful for a later networked example with many concurrent connections, but would not solve those protocol limitations.
