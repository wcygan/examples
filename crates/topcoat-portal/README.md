# Topcoat two-phase commit lab

This local portal combines two OpenRaft shards with a single coordinator. It uses Topcoat 0.10.0's `topcoat-router` routing/SSE component; the direct `topcoat` facade dependency is pinned with default features off so the build stays on the cached 0.10.0 feature set. The portal binds only to loopback and defaults to port `0` (an OS-selected ephemeral port), not the unrelated service on port 3000.

## Run

From the workspace root:

```sh
cargo +1.98.0 build --workspace --offline
cargo +1.98.0 run -p topcoat-portal -- --port 0 \
  --raft-bin "$PWD/target/debug/raft-sharded-tx" \
  --journal /tmp/two-phase-decisions.jsonl
```

Open the `PORTAL_READY` URL printed by the app. The coordinator writes a JSONL `Begin` before prepare and syncs each immutable `Commit` or `Abort` decision to the journal file before broadcasting phase two. The UI shows the persisted decision map, current balances, prepared transaction IDs, and configured node IDs. A recovered transaction resends the exact journaled decision; it cannot switch to the opposite decision. `CommitAcknowledged` means both shard leaders acknowledged the state-machine write through their Raft groups after the coordinator decision was synced. A timeout after a decision is recorded returns an uncertain-delivery result and keeps that decision available for resending.

### Controls and failure boundaries

- **Commit / Inject a NO vote**: run a normal transfer or a real source state-machine rejection. The no-vote path durably records ABORT and sends it to both groups.
- **Drop a vote**: omit the destination vote after the source has returned YES. This is a deliberate message-boundary drop injection, not a process crash. The coordinator records no decision and the source stays prepared.
- **Pause after YES votes / Resume**: pause the coordinator after both participants are prepared but before writing a decision. It demonstrates the blocking window. This is labeled a pause, not a killed coordinator.
- **Withhold destination decision / Recover**: sync COMMIT, send it to source only, then intentionally withhold destination delivery. Recover/resend applies the same decision to destination.
- **Add voter**: start a new child under a fresh node ID, add it as an OpenRaft learner and wait for catch-up, then promote it using a committed membership change. Each shard is capped at five voters.
- **Remove node**: change Raft membership first, then stop and reap that child. It refuses to reduce a shard below three voters and refuses changes while an undecided transaction exists.
- **Kill a follower process**: terminate one actual portal-owned follower process without changing membership. A three-voter group should retain quorum with its two remaining voters. The dead node is not restarted because its log/state were volatile.

The browser's event stream uses monotonically increasing IDs, a bounded 512-event in-memory replay ring, `Last-Event-ID` replay on reconnect, and an authoritative status snapshot when the requested event is older than the ring. A dropped SSE connection never owns transaction execution; the coordinator continues independently.

## Tests and cleanup

```sh
cargo +1.98.0 fmt --all -- --check
cargo +1.98.0 clippy --workspace --all-targets --offline -- -D warnings
cargo +1.98.0 test -p topcoat-portal --offline
```

`tests/protocol.rs` starts six child nodes and checks committed values, NO-vote abort, missing vote with no coordinator decision, blocked prepared state, partial decision delivery and recovery, duplicate retry idempotence, immutable decision-file reopen, real learner promotion/removal, and quorum survival after a child process kill. `tests/http_e2e.rs` starts the portal binary itself, makes HTTP requests to the Topcoat router, exercises the visible page, status, transfer, recovery, learner lifecycle, and SSE route, then requests graceful shutdown. It uses a unique temporary journal and cleans it after the child portal and node processes stop. The portal's own `/api/shutdown` is intended for local demo/test cleanup.

## Honest boundaries

The coordinator journal is synced to disk, but the participant OpenRaft logs, prepared promises, membership, snapshots, and balances are all volatile. A process crash/restart loses participant state. Do not interpret the durable coordinator decision file as durable participant recovery or atomic recovery across machine crashes. If a durable participant store is added later, each YES must follow a synced prepared promise, and each replacement/recovery path must reconcile that promise with the same immutable coordinator decision before applying. This example does not implement consensus for the coordinator, coordinator failover, a production acknowledgement protocol, authentication, or production HA. Prepared participants can block while the coordinator is unavailable. All six children belong to this local demo; no unrelated process is touched.

For the standard-library-only counterpart, see [`two-phase-commit`](../two-phase-commit/README.md). Tokio is used here for concurrent TCP peers, process controls, bounded async RPC, and long-lived SSE; Tokio itself contributes no transaction safety.
