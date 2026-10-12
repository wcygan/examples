# Two-phase commit, in miniature

This crate demonstrates two-phase commit with one coordinator. The UI supports
2–5 numbered participants. All six scenarios are available in the UI. The example
makes protocol state transitions and failure tradeoffs visible.

## Run the UI

1. From the repository root:

   ```sh
   cd crates/two-phase-commit
   cargo run
   ```

2. Open [http://127.0.0.1:3000](http://127.0.0.1:3000) in your browser.
3. Pick a scenario, set **Participants** to 2–5, and click **Run scenario**. Use **Pause**, the arrows, or the
   slider to inspect what happened.

Keep the terminal running while using the UI. Press **Ctrl+C** to stop it.
Run history lasts until the server stops. No frontend setup is required.

## Structure

- `src/lib.rs`: pure coordinator and participant state machines, explicit states, immutable decisions, idempotent duplicate messages, and unit tests.
- `src/main.rs`: starts the UI server.
- `src/demo.rs`: shared scenario runner using std threads and channels.
- `tests/console_e2e.rs`: starts the compiled executable and checks all scenario/count combinations through HTTP and SSE.

## Phases and safety

1. **Prepare / vote:** coordinator asks participants to prepare. A participant votes YES only if it can prepare its local update; otherwise NO. Any NO fixes the coordinator decision to ABORT.
2. **Decision:** coordinator chooses COMMIT only after YES from every distinct configured participant. A repeated YES from one identity counts once. Missing votes leave the coordinator undecided.
3. **Apply:** participants apply the immutable decision idempotently. Duplicate COMMIT delivery remains COMMIT; contradictory decisions are rejected.

A prepared participant cannot infer ABORT from timeout. It remains prepared and blocked because the coordinator may already have committed. The demo's `coordinator-loss` scenario injects coordinator loss after a YES and shows this blocking condition. `partial-delivery` withholds one decision delivery; `recovery` resends the same decision. These are deterministic in-memory message-boundary injections, not process crashes. Participants can temporarily display different local states while delivery is incomplete, but a decision never changes to the opposite outcome. The coordinator decision remains COMMIT even when a participant has not received it; until delivery is complete the outcome is uncertain at that participant. This demo does not implement acknowledgment messages or a production acknowledgment protocol.

## Scenarios

Choose **Commit**, **Abort**, **Missing vote**, **Coordinator loss**,
**Partial delivery**, or **Recovery** from the UI's scenario selector.
No scenario arguments are needed.

## Test layers

- Unit tests cover all-YES commit, NO abort, distinct-identity vote counting, missing-vote timeout, prepared blocking, idempotent retries, immutable outcome, and rejection of unknown participants.
- End-to-end tests launch the UI server on ephemeral loopback ports and exercise
  all six scenarios with 2–5 participants through HTTP and SSE. They verify exact
  states and balances, withheld messages, retries, and replay. Children are
  killed and reaped after each test.

Repeatable offline verification from workspace root:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo test --workspace --offline
```

## Limitations

All state is in memory. A real crash/restart implementation would require fsync'd participant prepared promises before YES, and a fsync'd coordinator decision before broadcasting it; it would also need recovery and retransmission logic, message identity/epoch handling, and acknowledgment tracking. None is implemented, and no durability is implied. Two-phase commit can block when the coordinator is unavailable after participants prepare. There is one coordinator, no consensus or failover, no real network, no authentication, and no production HA claim. A local std channel is adequate here; Tokio becomes useful for a later networked example with many concurrent connections, but would not solve those protocol limitations.

## Realtime console

See [Run the UI](#run-the-ui) for the quick start. If port 3000 is busy, choose
an available port from the crate directory:

```sh
cargo run -- --port 0
```

Open the URL printed in the terminal. Assets are embedded in the
binary; there is no frontend build step, remote asset dependency, or database.
From the workspace root, `cargo run -p two-phase-commit` also starts the UI.

Select any of the six scenarios, choose 2–5 participants with the slider, and
choose **Run scenario**. Nodes are labeled Participant 1, Participant 2, and so
on; the waterfall uses P1–P5. The slider configures the next run. Historical
runs keep their recorded participant count, including at the initial replay
position.

Participant 1 starts with balance 100; each other participant starts with 20.
On COMMIT, Participant 1 transfers 10 to each other participant. ABORT leaves
balances unchanged, and retries never apply a transfer twice. NO-vote and
partial-delivery target the last participant. Missing-vote withholds one vote;
simulated coordinator loss occurs after one YES and prevents delivery of all
remaining votes. Prepared participants remain blocked in both cases.

The audit log collects
at protocol speed while the visual demo plays its buffered events independently.
At 1×, each receiving event animates a message for 900 ms, followed by a 650 ms
reading pause. The coordinator's decision and vote count appear inside its node;
participant nodes show state, vote, balance, and blocking. Recording completion
is separate from the transaction outcome and the playback position.

The audit waterfall sits beside the scene on desktop. Its coordinator and
participant lifelines represent observation order, not elapsed time. Every
observation gets a row; arrows connect actual sends and receipts by message ID
and delivery attempt. Faults, simulated timeouts, and recording completion are
local markers, not protocol sends. A sent vote intercepted by the fault harness
has a dashed, stopped connection and no receipt. A decision withheld before
sending has only a fault marker; its later transmission is labeled as a retry.
Rows are revealed as
the replay progresses, with a count of collected observations waiting to play.
Each participant sends its recorded vote once. Contradictory late votes are
tested separately against the coordinator; they are not injected into scenario
traces.

**Pause** freezes packets and the cursor while recording continues. Use
previous/next, the slider, or **Play** with 0.5×/1×/2× speed. **Follow live** drains
buffered observations without jumping ahead. Clicking or keyboard-activating an
audit row seeks to its exact snapshot; scrolling or keyboard-browsing history
pauses playback. **Selected observation details** reveals timing, IDs, payload,
state changes, and the full recorded event. Selecting **History** starts at the
initial state. Only one scenario can record at a time, even across browser tabs.

The workspace fits desktop viewports of 1024×768 and 1280×800, with history
scrolling inside its panel. Narrow screens stack the scene above the waterfall
and keep replay controls sticky. Node buttons, tabs, audit rows, and native
controls support keyboard operation. Arrow keys switch visual tabs. When many
lanes do not fit, scroll sideways inside the waterfall; its headers stay aligned.

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
| `POST /api/runs` | Start with JSON `{"scenario":"commit","participant_count":5}` (count defaults to 2); 201 with run ID, 400 for invalid input or counts outside 2–5, 409 while a run is active |
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
startup on an ephemeral port. Protocol tests remain in place.

The process-level console suite launches the compiled binary on isolated
ephemeral loopback ports. Six separate tests exercise the public HTTP endpoints
and SSE stream at each supported count: 24 scenario/count combinations. They check complete protocol boundary counts, one vote per
participant, real send/receipt correlation, withheld messages, blocked states,
exact balances, duplicate COMMIT idempotency, and snapshot-preserving SSE
reconnection. Each child has startup and request deadlines and is killed and
reaped after the test, including assertion failures. No additional dependencies
or running preview are needed. Run this suite on its own with:

```sh
cargo test -p two-phase-commit --test console_e2e --offline
```

These tests exercise the server process and wire contract; the Node suites below
cover waterfall presentation and playback behavior.

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
| Card shadows | Flat bordered panels; a subtle shadow only on sticky mobile controls |

### State-machine diagrams

Click a scene node or choose **State machine**, then select a node. Inspection
pauses playback and shows one diagram at a time at the shared cursor. Filled
states indicate the selected snapshot; bold
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
Playback suspends when the tab is hidden, preserving its cursor. Switching visual
views pauses it; Play returns to the transaction scene. Reduced
motion skips packet travel while preserving reading pauses and replay controls.

Diagram geometry, playback timing, and waterfall checks require only Node, with no packages.
The waterfall test starts an ephemeral console and checks all 24 actual traces
and desktop/mobile node and route geometry:

```sh
cargo build -p two-phase-commit --offline
node crates/two-phase-commit/tests/state-machines.cjs
node crates/two-phase-commit/tests/message-demo.cjs
node crates/two-phase-commit/tests/waterfall.cjs
```

### Presentation restructuring

The UI has one authoritative playback cursor and shared message presentation
rules. Separate node-card rendering and automatic page jumps were removed;
state-machine inspection is a secondary view. History polling no longer rebuilds
unchanged visuals, and the waterfall updates only when its visible prefix changes.

Physical line counts across the five formatted bundled UI assets increased from
1,792 to 2,379 (+587), while the main controller decreased from 333 to 326 lines.
The added code implements delivery-attempt correlation, the selectable waterfall,
recorded-event details, and responsive/focus styles. This is structural cleanup
with new visual capabilities, not a net production-code reduction. Tests and
README content are excluded from these counts; no frontend dependencies were added.
