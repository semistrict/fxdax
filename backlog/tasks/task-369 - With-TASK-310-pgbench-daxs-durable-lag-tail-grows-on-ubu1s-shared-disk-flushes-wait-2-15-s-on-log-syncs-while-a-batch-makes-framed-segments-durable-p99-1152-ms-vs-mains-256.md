---
id: TASK-369
title: >-
  With TASK-310, pgbench-dax's durable-lag tail grows on ubu1's shared disk:
  flushes wait 2-15 s on log syncs while a batch makes framed segments durable
  (p99 1152 ms vs main's 256)
status: Done
assignee:
  - '@claude'
created_date: '2026-11-11 22:52'
updated_date: '2026-11-13 01:53'
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
- [x] #1 Which syncs the slow flushes wait behind, shown by a trace
- [x] #2 pgbench-dax's durable lag p99 back to main's, slowest framing kept under 1 s, by an A/B on ubu1
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
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

2026-11-12, the version landed (branch t3/369-lean, rebuilt from main):
- **Kept:**
  - de-duplicated entry syncs;
  - the next segment's log prepared off the state lock (TASK-333), by a flush that made commits durable, not at open;
  - recovery taking a log as its segment's only if its header names it;
  - a store's close waiting for a preparation in progress.
- **Dropped:** raw-file recycling (spare raw files) and log room (zero-filled room past a log's records). Both keep files on a tenant's disk that write admission doesn't count:
  - Linux's tenant disk tests failed (hostile a_tenant_filling_its_disk_leaves_others_committing and a_compromised_vm_process_harms_no_tenant_beside_it, disk.rs tenants_past_their_disk_limits_are_refused_while_others_write);
  - serves_hundreds_of_forks_of_a_base held a file per fork more.
  - A frugality switch (no preallocation near a tenant's limit) fixed the hostile tests but not disk.rs: freed data must leave the disk, and preallocated files stay.
- **Their worth, measured:**
  - durable_commits_through_the_daemon on a quiet ubu1 (bucket on /dev/shm, 25 ms, 5 interleaved runs): main 39.2-40.6 ms median rounds, with room and recycling 39.6-42.9, without them 39.8-42.2. Nothing.
  - Recycling's one gain: doc-12's median under load, within 50 ms 68% vs 56% (pgbench) and 63% vs 52% (pgbench-dax). Its cost: about 10% more host writes and up to a segment of disk per store. Filed as TASK-387.
- The daemon test's gain is TASK-384's: 32.0 ms against a 30.1-30.6 ms round trip.

Validation on t3/369-lean, rebased on main f926e1488:
- macOS scripts/test.sh (clippy, workspace 2282/2282, sim 53, Shuttle 37, the rest) green.
- Linux on ubu1 (fxjob) green but for: TASK-315's page-cache test (ZFS target dir, fails on main too); fork_faults' host-death copy, which raced the stores' renames and was fixed (10/10 runs after); Go, not installed on ubu1.
- pgbench A/B, lean vs main, 3 quiet interleaved rounds (/mnt/fxdax-scratch/t369/ab9-pg), all 12 runs within doc-12's median:
  - pgbench: within 50 ms 81.9% vs 79.2%, p50 32 vs 64 ms, p99 128 vs 128 ms, max 466 vs 300 ms, tps 3005 vs 3015, io per guest byte 1.32 vs 1.32.
  - pgbench-dax: within 50 ms 81.7% vs 76.1%, p50 32 vs 64 ms, p99 128 vs 256 ms, max 378 vs 1158 ms, slowest framing 105 vs 124 ms, tps 3102 vs 3075.
- Daemon test, quiet-ish ubu1: main 39.7 ms median rounds, lean (with TASK-384) 35.5.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
pgbench-dax's durable-lag tail is back under main's (p99 128 vs 256 ms, max 378 vs 1158 ms, 3 interleaved rounds on ubu1).
- **Changed:**
  - A flush marks the entries it synced, so a making of framed segments durable syncs only its own.
  - The next segment's log is prepared off the state lock, by a flush that made commits durable (TASK-333).
  - Recovery takes a log as its segment's only if its header names it.
- **Dropped:** raw-file recycling and log room. They broke tenant disk limits and bought nothing in the daemon test on a quiet host; TASK-392 follows them up.
- **Verified:** macOS and Linux suites, Shuttle, sim, crash loop, the pgbench A/B, the daemon test.
<!-- SECTION:FINAL_SUMMARY:END -->
