---
id: TASK-142
title: >-
  Object storage: a guest's sparse extents become thousands of little objects,
  and pgbench's checkpoint stalled commits for two minutes
status: To Do
assignee: []
created_date: '2026-10-31 21:45'
labels: []
dependencies: []
priority: high
ordinal: 492000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r353, non-DAX pgbench, jailed, in GCS: at pg_ctl stop 1.2 GiB waited to upload, past max_lag_bytes, commits waited, 'Write I/O request blocked' for over 100 s, and pg_ctl's 60 s ran out. An extent is stored on its own by its reach (Options::large_extent, 4 MiB), so a sparse one holding a few KiB is an object of its own: 6128 files averaging 3 KiB in that store. They uploaded 4 at once: 60 PUTs/s, 1 KiB each. OWN_AT_ONCE 32 relieves it; each is still a PUT.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 pgbench passes in GCS, jailed, its commits stalling under 10 s
- [ ] #2 Whether an extent is stored on its own goes by the bytes it holds, not its reach, or that is shown not to matter
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
