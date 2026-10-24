---
id: TASK-91.17
title: >-
  extentd: segment closes land inside the no-DAX load's fsync commits (~41 ms
  each)
status: In Progress
assignee: []
created_date: '2026-10-23 18:01'
updated_date: '2026-10-24 12:29'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: high
ordinal: 355000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Round 6: without DAX, pgbench -i's 206 fsync commits average ~41 ms, 6.34 s of their 8.37 s store-commit phase closing segments; with fsync=off the load takes 12.4 s (their runtime 12.3 s). TASK-41 framed closed segments off the ordered commit, yet after the load's large writes the closes still land in the commit.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 No-DAX load within 10% of its fsync=off time (12.4 s on their host)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Landed (7112b50d): the commits' slow seal copies read raw files with O_DIRECT, which waited for the guest's just-written dirty pages to reach the disk; raw files are now read through the page cache. The no-DAX load: 21.78 s -> 17.9-18.6 s with fsync on, 13.6-13.9 s with fsync off (new guest test fxfs-guest-pgbench-nofsync); store commits 3.79 s -> 0.36-0.49 s of the load, seal copies 1,444 / 9.3 s -> 139 / 0.31 s. AC #1 (within 10%) is not met: the rest, +30%, is the guest's: PostgreSQL's WAL fsyncs (2.06 s, ~650 MB written synchronously) and ~1.5 s of guest writeback not attributed. Also found: three ordered commits spent 89-143 ms in append_record (index checkpoints inline under the state lock); with fsync off, a commit sealing hundreds of 1 MiB extents into their own files uploads them OWN_AT_ONCE=4 at a time and passes max_lag_bytes. Linux (GCE fxdax-test-r7-closes): workspace 2559 passed; Shuttle suites; guest fxfs-guest-pgbench, -pgbench-nofsync, -dax, -checks, extent-guest, -follow, -live-fork, -snapshot-dax OK (pgbench-dax hit TASK-79.104's guest crash in DAX evacuation, not this change). macOS workspace 2481. Pushed ff83e8bc..7112b50d.

Handed over (2026-10-24): landed on main; what is left is in the notes above.
<!-- SECTION:NOTES:END -->
