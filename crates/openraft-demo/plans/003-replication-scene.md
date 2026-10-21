# 003 — Explain replication visually

- Status: DONE
- Commit: 1157264
- Severity: MEDIUM
- Category: Spatial continuity and performance

## Problem and target

`src/ui/console.js:7` draws ten equal links at five nodes and flies packets
under card centers. `src/ui/console.js:22` does not display replicated entries;
`src/ui/console.js:39` rebuilds every waterfall row on every advance.
Show short log strips with Recorded / Committed / Applied text, highlight the
actual active route, and fly between card edges. Preserve node and row elements;
append newly revealed rows and update selection instead of rebuilding the list.

## Steps and boundaries

1. Add persistent node fields, log strips and observed replication progress.
2. Route a labeled packet between measured card boundaries; dim background wires.
3. Incrementally append raw audit rows and actual correlated connections.
4. Inspect 3-, 4-, 5-node desktop/narrow layouts and keyboard navigation.

Use the existing blue/amber/green/red palette. Short, restrained state highlights
may use opacity; no bounce or decorative heartbeat loops. Reduced motion keeps
text and states readable. No protocol changes or dependencies.

## Verification

Append, commit and application remain distinct and occur at their recorded
boundaries. Packet and label stay visible during pause. Rows preserve focus,
manual seeking is immediate, and narrow layouts do not overflow horizontally.

## Completion evidence

Implemented in the bundled presentation. The motion suite passed all 12 real
scenario/count combinations, exact manual snapshots, cancellation, pause/resume,
speed changes, hidden tabs, reduced-motion timing, live arrivals and route
geometry. All 28 workspace tests, offline Clippy and formatting passed. Visible
checks covered a paused flight, resizing, reduced motion, raw playback, and
Recorded → Committed → Applied boundaries. Desktop and narrow screenshots were
saved as `/tmp/openraft-teaching-desktop.png` and `/tmp/openraft-teaching-narrow.png`.
