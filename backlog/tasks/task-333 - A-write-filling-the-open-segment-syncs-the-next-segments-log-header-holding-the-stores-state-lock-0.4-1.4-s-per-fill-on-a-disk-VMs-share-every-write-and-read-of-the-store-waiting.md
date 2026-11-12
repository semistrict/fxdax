---
id: TASK-333
title: >-
  A write filling the open segment syncs the next segment's log header holding
  the store's state lock: 0.4-1.4 s per fill on a disk VMs share, every write
  and read of the store waiting
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-10 21:40'
updated_date: '2026-11-12 07:46'
labels:
  - perf
  - extentd
dependencies: []
priority: high
type: bug
ordinal: 736000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-310's sync trace (scripts/bpftrace/store-syncs.bt) on ubu1's shared-disk case (fxfs-guest-shared-disk, main 7e1740cb7, 16 VMs 8 at once, ext4 on a zvol): the slowest syncs of the whole run were fdatasyncs of 4 KiB logs (logs/<branch>/<n>, a log's header alone) on the 'extentd io' threads, 1.1-1.4 s each, several stores at once (main-trace-2: logs/...04 1.38-1.40 s, logs/...05 1.15 s). That is Segments::fill -> create_open, called from Store::make_room on a guest's write that finds the open segment full, holding the state lock: the new log's header is written and synced (log.sync_cached) before the spare is renamed its log (TASK-301, TASK-329). Every write, read and commit of that store waits behind the sync, which waits behind the disk's queue (each sync a journal commit and a device flush). A guest writing a large file fills a segment every 32 MiB, so this stalls its writes as often. The framings' syncs (TASK-310) are off the framer's path; this one is on the guest's.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 No sync of the file system runs holding the store's state lock as a write fills the open segment; the next segment's log header is still durable before any record of its is, and before a spare is renamed its log (TASK-329's crash round passes)
- [ ] #2 The shared-disk case's trace shows no log-header sync on the write path; guest write latency measured before and after on ubu1
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-12 (claude): with TASK-369, local only: Segments::to_prepare/prepared and Preparation::prepare. A 'store opener' thread writes and syncs the next segment's log header off the state lock, under a spare's name (one made anew gets a number with the top bit set, so no segment's spare can take its name). The opening renames it to its log, syncing nothing; it falls back to the old inline path while a preparation runs. Test: segments::tests::segments_open_with_their_logs_prepared_syncing_nothing, red without it. macOS suite green, crash tests 10/10 on ubu1. Waits on TASK-371 and its re-measure, like TASK-369.
<!-- SECTION:NOTES:END -->
