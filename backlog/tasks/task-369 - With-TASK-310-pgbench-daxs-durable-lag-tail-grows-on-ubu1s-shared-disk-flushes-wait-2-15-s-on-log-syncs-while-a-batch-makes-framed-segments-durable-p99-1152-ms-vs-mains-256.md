---
id: TASK-369
title: >-
  With TASK-310, pgbench-dax's durable-lag tail grows on ubu1's shared disk:
  flushes wait 2-15 s on log syncs while a batch makes framed segments durable
  (p99 1152 ms vs main's 256)
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-11 22:52'
updated_date: '2026-11-12 07:46'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-12 (claude): two fixes, local only, not pushed.
(1) A making of segments framed durable synced the directories of raw files and logs whenever any segment's entries were unsynced. The segment just opened always was, and a flush had usually synced them already. Now it syncs only its own segments' entries, and a flush that syncs them marks them synced (tests/store.rs segments_made_durable_sync_no_entries_a_flush_synced: 3 directory syncs a closing, not 5; red with either half out).
(2) TASK-333: the next segment's log is prepared off the state lock by a 'store opener' thread.
Threads are named (store framer, durable, opener), and store-syncs.bt now filters by cgroup and attributes each sync's journal wait.
Evidence, traced pgbench-dax under a disk loader (one run each, pre-TASK-310 23dfe74bf vs main 95bbb627d):
- A flush's raw-file fdatasyncs take 116-120 s on both trees, 85-90% of it waiting on ext4 journal commits (data=ordered, a growing file). This predates TASK-310 and is TASK-322's.
- Main's extra: the durable thread's directory syncs of open-segments/logs, 50+49 s of journal wait (pre's framer: 26+14 s), which is fix 1; and log-header syncs on the write path, 58 s vs 24 s, which is fix 2.
The interleaved A/B (main vs both fixes; pgbench and pgbench-dax 6 rounds under the loader, shared-disk-dax 3) was inconclusive: the loader saturated both sides (flushes 4-33 s, most runs failing doc-12's median on both), and shared-disk-dax's framing held at about 500 ms on both. The loader also ran as plain fxjob work beside TASK-330.4's bench A/B, the coordinator stopped it; a loader now runs inside its rounds' fxjob --bench job.
Next: rebase on TASK-371 (one durable commit per sync, same path) when it lands, then re-measure with the bench slot.
<!-- SECTION:NOTES:END -->
