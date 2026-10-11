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

## Realtime console

Run the Topcoat console from the workspace root:

```sh
cargo run -p two-phase-commit --bin two-phase-commit-console --offline
# Use another port, or zero to choose an available loopback port:
cargo run -p two-phase-commit --bin two-phase-commit-console --offline -- --port 0
```

Open the printed URL (default `http://127.0.0.1:3000`). Assets are embedded in the
binary; there is no frontend build step, remote asset dependency, or database.
The CLI remains the default binary, so the earlier scenario commands still work.

Select any of the six scenarios and choose **Run scenario**. The audit log collects
at protocol speed while the visual demo plays its buffered events independently.
At 1×, each receiving event animates a message for 900 ms, followed by a 650 ms
reading pause. Node cards show the coordinator's decision
separately from participant state, vote, balance, and blocking. The three-lane
timeline correlates message observations by message ID. The late NO in decided
scenarios is explicitly synthetic and demonstrates decision immutability.

**Pause** freezes the viewed history while the actual scenario keeps running.
Use previous/next, the replay slider, or **Play** with 0.5×/1×/2× speed to inspect
recorded events. **Follow live** drains the buffered observations at the selected
speed without jumping ahead. Selecting a
run from **Session history** begins at its initial state. Only one scenario can
execute at a time, including when multiple browser tabs are connected.

The append-only event log lives entirely in memory for the server session.
Each typed event includes its run ID, collector sequence, elapsed time, actor,
optional message ID, typed protocol payload, and peer, and a snapshot of observed node states and
balances. A single collector orders participant-thread observations; timestamps
are display metadata rather than evidence of distributed ordering. A fresh
server starts with no history. Browser reloads retain server history, and SSE
reconnections replay events after `Last-Event-ID` before following new events.

Local HTTP interfaces:

| Endpoint | Behavior |
| --- | --- |
| `GET /api/runs` | Run summaries, newest first |
| `POST /api/runs` | Start with JSON `{"scenario":"commit"}`; 201 with run ID, 400 for invalid input, 409 while a run is active |
| `GET /api/runs/{id}` | Run metadata and all retained events; 404 for unknown runs |
| `GET /api/runs/{id}/events` | SSE JSON events with sequence IDs; accepts `Last-Event-ID`; ends after execution completes |

Run status (`running`, `finished`, or `failed`) describes the harness. A finished
missing-vote or coordinator-loss scenario still has blocked participants and no
transaction decision. Runner failures retain earlier observations and append a
failure event. Disconnecting a browser does not cancel a scenario.

This remains a local teaching model with std threads/channels. Faults are
simulated at message boundaries; observation logging does not provide durable
prepared promises, durable decisions, crash recovery, or independent network
nodes. The console binds only to loopback and has no authentication.

Console tests cover all six structured traces, intermediate partial-delivery
snapshots, idempotent retransmission, request validation, session history,
concurrent-start rejection, SSE cursor replay/live continuation, and process
startup on an ephemeral port. Existing CLI and protocol tests remain in place.

### Console palette

New CSS tokens use OKLCH. All are within sRGB gamut. Text pair checks measured
WCAG ratios of 7.31:1 or greater and APCA absolute Lc values of 78.9 or greater.
Status meanings also appear in text, so color is never the only signal.

| Before | After |
| --- | --- |
| No page background | `--bg: oklch(0.978 0.005 250)` |
| No surface color | `--surface: oklch(1 0 0)` |
| No primary text | `--ink: oklch(0.25 0.025 250)` |
| No secondary text | `--muted: oklch(0.42 0.025 250)` |
| No control outline | `--line: oklch(0.86 0.015 250)` |
| No active foreground | `--blue: oklch(0.42 0.1 250)` |
| No active background | `--blue-soft: oklch(0.95 0.02 250)` |
| No waiting foreground | `--amber: oklch(0.38 0.07 75)` |
| No waiting background | `--amber-soft: oklch(0.96 0.02 75)` |
| No success foreground | `--green: oklch(0.38 0.07 150)` |
| No success background | `--green-soft: oklch(0.96 0.025 150)` |
| No failure foreground | `--red: oklch(0.4 0.12 25)` |
| No failure background | `--red-soft: oklch(0.96 0.018 25)` |
| No surface shadows | `oklch(0 0 0 / 0.04)` in both shadow layers |

### State-machine diagrams

The coordinator and both participant diagrams follow the same event cursor as
the node cards and timeline. Filled states indicate the selected snapshot; bold
arrows indicate transitions observed at that event. Previous/next, scrubbing,
playback, and Follow live update all views together. The diagram never sends
protocol commands or reruns a transaction.

Transition highlights compare adjacent snapshots. Timeout observations highlight
waiting self-loops; applying a matching decision to an already terminal
participant highlights an unchanged-outcome self-loop. Sending a retry does not
imply that its recipient applied it. Coordinator late votes preserve the fixed
decision. Blocked participants remain Prepared, and coordinator unavailability
is shown separately from its transaction state.

The message topology animates PREPARE, YES/NO votes, and COMMIT/ABORT deliveries.
Receiving state changes appear on arrival; withheld messages and retries are
explicit. Pause freezes packets and the cursor while collection continues.
Seeking cancels the current flight and restores the selected snapshot immediately.
Playback suspends when the message view is offscreen or the tab is hidden. Reduced
motion skips packet travel while preserving reading pauses and replay controls.

Diagram geometry and playback timing checks require only Node, with no packages:

```sh
node crates/two-phase-commit/tests/state-machines.cjs
node crates/two-phase-commit/tests/message-demo.cjs
```
