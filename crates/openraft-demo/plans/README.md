# OpenRaft motion improvements

Base commit: `1157264`. The existing animation review supplies the findings;
the user authorized implementation on 2026-10-12.

| Order | Plan | Severity | Status | Dependency |
| --- | --- | --- | --- | --- |
| 1 | [001 — Preserve playback continuity](001-playback-continuity.md) | HIGH | DONE | None |
| 2 | [002 — Focus teaching playback](002-teaching-playback.md) | HIGH | DONE | 001 |
| 3 | [003 — Explain replication visually](003-replication-scene.md) | MEDIUM | DONE | 002 |

The implementation is confined to the OpenRaft presentation, bundled routes,
documentation and tests. The 2PC controller is the continuity reference.

All plans completed. Teaching playback retained every recorded observation and
snapshot while scheduling about 16% of full-observation playback time across
the 12 tested combinations. This measures scheduled narrative time, not frame
rate. The OpenRaft protocol runner and the 2PC implementation were preserved.
