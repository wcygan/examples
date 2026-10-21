# OpenRaft consensus lab

Run the UI:

```sh
cd crates/openraft-demo
cargo run
```

Open **http://127.0.0.1:3001**. Choose a scenario, set **Raft nodes** to 3, 4, or 5, and click **Run scenario**.
Port 3001 lets this run alongside the two-phase commit console on port 3000.
Use `cargo run -- --port 3002` to select another port.

## Lessons

- **Replicate a write:** Node 1 wins an election and replicates `lesson = first`.
- **Lose the leader:** stop Node 1, elect Node 2, and acknowledge another write with the surviving nodes.
- **Lose the quorum:** isolate Node 1. Its next write times out and no node applies it.
- **Catch up a follower:** isolate the last node, write with the remaining quorum, then restore the link and observe catch-up.

These are 3–5 real OpenRaft 0.9.25 instances. The harness triggers elections
rather than waiting for randomized automatic election timers. Transport isolation
and node loss are simulated within one process. No independent network servers
or process restart recovery are modeled.

The node slider configures the **next** run. Selecting history restores that
recording’s original node count and geometry.

## Playback

Every teaching step includes a short explanation of what changed and why it
matters. During a flight, a two-line rectangle identifies the payload: election
term and last-log index, a key/value entry and its index, or a commit announcement.
The caption explains the message's purpose; arrival explanations distinguish RPC
receipt from recording, commitment, and application. Long card fields are
shortened with an ellipsis; audit details retain the full payload. Manual seeking
restores the same teaching explanation.

**Teaching** playback is the default. It emphasizes elections, votes, new
application entries, commitment, application, faults and follower recovery.
Repeated metrics and routine traffic advance quietly in observation order.
**Every observation** plays each raw observation and each correlated receipt.
Both modes retain the same complete audit log and final snapshots.

The log strips distinguish **Recorded**, **Committed**, and **Applied** entries.
Recorded means the entry exists in that node’s observed local Raft log; it does
not imply commitment.
The majority indicator counts identical entries present in recorded node logs;
it marks commitment only after a recorded commit update. A blue route and the
caption identify the active message. Routes avoid cards; labels stay readable.

Recording and playback are independent. **Pause** freezes the visible packet
and its progress; **Play** resumes it. The continuous **Speed** slider adjusts
from 0.25× (slower) to 2× (faster) while playing, using the pointer or arrow keys.
Speed changes, hidden tabs, and resizing preserve flight and reading progress.
At 1×, Teaching uses 1.8-second flights and 3.2–7 second reading pauses based
on explanation length. Every observation keeps 900 ms flights and 650 ms pauses.
Reduced motion removes packet movement and value highlights while keeping
reading time and exact state.

The arrow buttons, scrubber and audit rows inspect **individual observations**
immediately, even in Teaching mode. Seeking cancels the pending flight and
restores the selected recorded snapshot. Select **History** to start a previous
run from its initial state. **Follow** waits for new observations without
jumping over teaching steps. Expand **Selected observation details** for every
recorded field. Controls support the keyboard, and browsing the audit pauses
playback. Every recorded audit row remains inspectable.

RequestVote, AppendEntries and their responses are recorded at actual transport
boundaries. Local markers show storage mutations, committed application, metrics,
client results and injected faults. Collector sequence means observation order;
it is not a distributed clock. Metrics watch updates may coalesce.

## Audit versus Raft log

The **audit stream is append-only in memory**. It preserves past snapshots even
when a Raft node truncates an uncommitted suffix or purges a snapshotted prefix.
Playback never reruns the protocol. History lasts only for the server session;
there are no journal files, durable recovery, or tamper-proof guarantees.

The Raft storage is also volatile. An acknowledged write survives the simulated
single-node loss through the remaining quorum, but not a server restart. A client
**timeout has an unknown outcome**, not an abort decision. Recording completion
means the lesson ended, not that every attempted write committed.

[Library comparison and design reasoning](RESEARCH.md).

## Checks

From the workspace root:

```sh
cargo test -p openraft-demo --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo fmt --all -- --check

# Built-in Node tests; no npm dependencies or frontend build required
cargo build -p openraft-demo --offline
node crates/openraft-demo/tests/motion.cjs
```

The JSON API provides `GET /api/runs`, `POST /api/runs` with
`{"scenario":"replication","node_count":5}` (count defaults to 3), and `GET /api/runs/{id}`. The SSE endpoint
`GET /api/runs/{id}/events` replays observations after `Last-Event-ID`, then streams
new ones. Only one recording runs at a time. Port zero is supported for tests.

## Property-style end-to-end coverage

The process-level test starts the real console on an ephemeral port and runs
all four scenarios at 3, 4 and 5 nodes **twice** (24 recordings). Each complete
HTTP/SSE trace is checked observation by observation for:

- majority replication before commit or client acknowledgement;
- a leader retaining earlier committed entries;
- matching log IDs having identical prefixes and no overwritten committed prefix;
- monotonic saved-vote terms and commit/applied indexes, contiguous application and identical entries applied at each index;
- acknowledgement after local application and values changing only during application;
- one observed elected leader per term;
- actual correlated RPC sends/receipts with unchanged payloads;
- scenario-specific outcomes, including no application without quorum and eventual follower catch-up;
- exact replay at multiple SSE cursor positions and retained history across node counts.

Direct cluster tests cover the same 12 combinations. Mutation tests deliberately
corrupt traces to check that the safety checker rejects missing sends, collector
gaps, premature application, missing majorities, conflicting leaders and overwritten committed entries.
Run this focused suite with:

```sh
cargo test -p openraft-demo --offline
```

This is exhaustive coverage of the supported scenario/count matrix under varying
Tokio schedules, not a proof over every network schedule or a randomized fault
model. Metrics watches can coalesce; the election check covers recorded metrics.
The safety checker currently targets these non-compacting scenarios; snapshot
storage contracts are checked separately by OpenRaft's storage suite.

The motion suite checks all 12 real scenario/count traces, exact snapshots at
every sequence, complete event retention, reduced scheduled teaching time,
actual message correlation, commit-bearing empty RPCs, live arrival and
reconnect deduplication. Fake clocks verify pause/resume, speed, seek cancellation,
hidden tabs, resize continuity and reduced-motion reading time. Geometry checks
cover routes around cards at 3–5 nodes in desktop and narrow layouts.
