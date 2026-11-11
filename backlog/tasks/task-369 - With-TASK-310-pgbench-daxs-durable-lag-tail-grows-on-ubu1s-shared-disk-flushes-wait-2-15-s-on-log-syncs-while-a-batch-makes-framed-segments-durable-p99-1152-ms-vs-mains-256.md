---
id: TASK-369
title: >-
  With TASK-310, pgbench-dax's durable-lag tail grows on ubu1's shared disk:
  flushes wait 2-15 s on log syncs while a batch makes framed segments durable
  (p99 1152 ms vs main's 256)
status: To Do
assignee: []
created_date: '2026-11-11 22:52'
labels:
  - extent_store
  - durability
  - perf
dependencies: []
priority: high
type: bug
ordinal: 751000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-310's A/B on ubu1 (2026-11-11, main 04280b595's base cf757b22e vs TASK-310, 6 interleaved rounds each, one guest kernel, fxjob --bench): fxfs-guest-pgbench-dax, branch vs main: slowest flush 4609 ms median (270-14941) vs 1369 (139-3053); durable lag p99 1152 ms (128-4096) vs 256 (128-512); max 6125 (352-16635) vs 1723; pgbench tps 2709 vs 2948. Every run passed the case's own durability check (doc-12's window). Not in plain pgbench (branch flush 961 ms median, 156-2085; main 890, 206-9658) nor in the shared-disk cases. The slow branch runs (4 of 6: 14:15, 15:10, 15:35, 17:43; one before any OOM on the host) have batches of 1-3 segments taking 6-8 s to make durable (wait 3-5 s behind the batch before, entries 1.4-2.2 s, dirs 1.4-4.3 s) while flushes spend 6-10 s in their logs phase: the host's journal stalls (each directory sync a jbd2 commit, which in data=ordered writes back the DAX guests' dirty raw-file pages first), and with the framer no longer taking those syncs on itself the flushes now wait on them. Main's framings took the same stalls (its slowest framing 1068 ms median there). Next: bpftrace (scripts/bpftrace/store-syncs.bt) on pgbench-dax to see which syncs the flushes' log fdatasyncs wait behind, then fewer directory syncs per batch (the entries and segments directories in one, or the entries synced by the flush that needs them, as main did).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Which syncs the slow flushes wait behind, shown by a trace
- [ ] #2 pgbench-dax's durable lag p99 back to main's, slowest framing kept under 1 s, by an A/B on ubu1
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
