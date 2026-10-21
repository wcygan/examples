# 001 — Preserve playback continuity

- Status: DONE
- Commit: 1157264
- Severity: HIGH
- Category: Interruptibility and accessibility

## Problem and target

`src/ui/console.js:80` cancels flights on pause, speed changes and document hiding.
Preserve the same animation and remaining hold time, following the 2PC controller
in `../two-phase-commit/src/ui/message-demo.js:94`. Seeking and history changes
cancel deliberately. Keep 900 ms explanatory flights and 650 ms reading holds;
reduced motion removes movement while preserving reading time. A change to
reduced motion during a flight must settle safely at the recorded arrival.

## Steps and boundaries

1. Extract a testable playback clock using WAAPI pause/play/playbackRate.
2. Connect pause, speed, visibility, seek and reduced-motion controls.
3. Verify interruption with fake clocks and the visible browser.

No protocol changes, dependencies, commits or publication.

## Verification

Pause retains the visible packet and cursor; resume continues the same flight.
Speed changes and hidden tabs preserve progress. Seeking invalidates stale
callbacks. Reduced motion preserves meaningful reading holds and exact snapshots.

## Completion evidence

Implemented in the bundled presentation. The motion suite passed all 12 real
scenario/count combinations, exact manual snapshots, cancellation, pause/resume,
speed changes, hidden tabs, reduced-motion timing, live arrivals and route
geometry. All 28 workspace tests, offline Clippy and formatting passed. Visible
checks covered a paused flight, resizing, reduced motion, raw playback, and
Recorded → Committed → Applied boundaries. Desktop and narrow screenshots were
saved as `/tmp/openraft-teaching-desktop.png` and `/tmp/openraft-teaching-narrow.png`.
