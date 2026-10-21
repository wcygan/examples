# 002 — Focus teaching playback

- Status: DONE
- Commit: 1157264
- Severity: HIGH
- Category: Purpose and frequency

## Problem and target

`src/ui/console.js:89` treats every observation and receipt as a teaching step.
The reviewed five-node catch-up trace schedules about 250 seconds, including
72 metrics observations with no displayed change. Derive meaningful steps from
typed events without modifying or deleting audit observations. Default to
Teaching playback; offer Every observation for full detail. Keep raw sequence
numbers, exact manual seeking and complete audit rows.

## Steps and boundaries

1. Classify election changes, unique votes, command replication, commitment,
   first application, faults, timeout and recovery using recorded payloads.
2. Skip routine metrics and redundant exchanges during automatic teaching
   playback, consuming their snapshots in collector order.
3. Verify all four scenarios at 3–5 nodes, including empty AppendEntries that
   carry a new commit index; never invent a COMMIT RPC or a successful delivery.

Keep the Rust runner, audit and SSE contracts intact. No dependencies.

## Verification

A significant reduction in scheduled playback time; final and intermediate
manual snapshots remain identical to raw audit snapshots. Failed runs and live
streams retain history, and reconnect deduplication remains correct.

## Completion evidence

Implemented in the bundled presentation. The motion suite passed all 12 real
scenario/count combinations, exact manual snapshots, cancellation, pause/resume,
speed changes, hidden tabs, reduced-motion timing, live arrivals and route
geometry. All 28 workspace tests, offline Clippy and formatting passed. Visible
checks covered a paused flight, resizing, reduced motion, raw playback, and
Recorded → Committed → Applied boundaries. Desktop and narrow screenshots were
saved as `/tmp/openraft-teaching-desktop.png` and `/tmp/openraft-teaching-narrow.png`.
