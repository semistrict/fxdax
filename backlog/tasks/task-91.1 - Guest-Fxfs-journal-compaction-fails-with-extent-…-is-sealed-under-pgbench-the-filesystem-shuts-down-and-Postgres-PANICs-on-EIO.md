---
id: TASK-91.1
title: >-
  Guest Fxfs: journal compaction fails with 'extent … is sealed' under pgbench,
  the filesystem shuts down and Postgres PANICs on EIO
status: Done
assignee:
  - '@main-session'
created_date: '2026-10-22 18:22'
updated_date: '2026-10-22 19:05'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: high
ordinal: 266000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
In the sandbox team's 60 s pgbench runs at 853803e (3 of 8 VMs, DAX and not), Fxfs's journal terminated: 'Compaction error: Failed to flush objects: Failed to flush oid 5: extent ExtentId { branch: …, sequence: 1099511759396 } is sealed: Journal flush error'. Without DAX the guest logged 'extent … is sealed' during pgbench -i too (at 14 s and 28 s of uptime), then 'PANIC: could not fsync file'. extentd counted 2 WRITE errors: a WRITE to a sealed extent is refused with EPERM, which the guest turns into SealedExtent. So a write, likely the compaction's flush of object 5, went to an extent that a seal had already sealed. Possibly of the family of TASK-79.10 (a seal remapping the wrong extent offset).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A guest test reproduces it (pgbench runs after the load, or a narrower one) and passes
- [x] #2 No write is sent to an extent the guest's own seal has sealed; the race that let it is identified in the commit
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Reproduce: pgbench runs after the load in fxfs-guest-pgbench[-dax]; a fast check (writes_across_seals: 4 MiB writes written back while another thread fsyncs).
2. Find which writes the device refuses (instrumented device.rs: each extent's writes and the seals begun).
3. Make the guest's seal wait for writes in flight and hold new writes until it ends, as doc-4's ordering asks.
4. Verify: the check red on the old kernel, green on the new over rounds; pgbench runs in both suites.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Cause: doc-4 applies a request in flight while a barrier is on either side of it. The guest only kept a barrier from being *submitted* between a write's requests (the submitting RwLock), and let writes be submitted while a barrier was in flight. A write longer than max_io is several requests: its first opened its fresh extent before the barrier, the barrier sealed it, and the rest were refused with EPERM, which Fxfs takes for SealedExtent: 'extent … is sealed', EIO, and in a compaction's layer-file write the journal shut down. Instrumented, every refused write was the only write to a fresh extent, 1 MiB at offset 0, with no seal begun after its submission: so the barrier in flight was the one before it.

Not the cause, ruled out on the way: multi_write's allocations are fresh extents (Allocator::allocate, unshared); only txn_map_write stages shared blocks, and dax_map_write already retries SealedExtent.

Fix (guest/rust/fxfs-linux/src/device.rs): a second Gate, the writes': plain writes hold its passage from submission to their end; Seal::begin closes it and waits for those in flight, and Seal's drop opens it, after the barrier completes, as fxfs_device_seal_end lets DAX stores go then. Gate's branch moved out to its own Arc. Plain writes now wait out a seal as DAX stores did; TASK-91.2 (no object-storage wait inside a seal) is what makes that cheap.

Evidence: writes_across_seals failed in its first round, 1.5 s in, on the old kernel (twice); 6 of 6 rounds pass on the fixed one, DAX and not. fxfs-guest-pgbench (no DAX) hit 'is sealed' at 66 s and Postgres's backends died in run 3 before the fix.

Possibly related, not yet checked: 128 MAP errors in the sandbox team's run; a MAP_WRITE in flight across a barrier would fall either side too (TASK-91.6).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
86e45a2: the guest's seal waits for writes in flight and holds new ones until its barrier completes (a writes Gate in device.rs), as doc-4's ordering needs: a multi-request write submitted while a barrier was in flight fell on both sides of it and was refused ('extent … is sealed', EIO, journal shut down). Verified: new check writes_across_seals red on the old kernel (round 1, 1.5 s, twice), 6/6 rounds green on the new, DAX and not; GCE default guest suite passes; pgbench load + three 60 s runs without DAX pass (hit the bug before). Workspace crates unchanged; upstream-changes.py --check passes. The DAX MAP EPERM seen in pgbench-dax's runs is TASK-91.9.
<!-- SECTION:FINAL_SUMMARY:END -->
