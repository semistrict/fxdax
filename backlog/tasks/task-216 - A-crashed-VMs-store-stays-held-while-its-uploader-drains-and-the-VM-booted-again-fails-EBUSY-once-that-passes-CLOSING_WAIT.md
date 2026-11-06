---
id: TASK-216
title: >-
  A crashed VM's store stays held while its uploader drains, and the VM booted
  again fails EBUSY once that passes CLOSING_WAIT
status: To Do
assignee: []
created_date: '2026-11-06 16:24'
labels:
  - bug
dependencies: []
priority: high
ordinal: 650000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by fxfs-guest-soak (TASK-67) on GCS, GCE r758 round 14 (run FXDAX_SEED=6539521978587016551, round seed 17212213123500368056; plan: 1000 ms added to every bucket request, writer::upload=27*off->1*return, slot 2's pgbench VM killed at 69 s): the VM booted again on slot 2 at once failed to start, Firecracker exiting 1: `opening the extent store at .../stores/slot-2: EBUSY`. Slot 2's branch (8e15cc7e16504c71) was not the one whose upload failed (eba9e0fa6cee884a). A connection whose VM process went lets its store go only once the store is closed, and closing it drops its uploader, which, stopping, closes the open segment, uploads everything gathered, waits for every upload in flight and checkpoints (crates/fxdax/src/remote.rs:854-878, the uploader's run loop; its Drop at :740-747 waits for that). A new writer's hold waits for the going connection at most CLOSING_WAIT, 10 s (crates/fxdax/src/extentd/stores.rs:53-56, :130), then fails EBUSY. Under a slow bucket the drain takes longer, so a VM manager restarting a crashed VM is refused for as long as the store's backlog takes to upload: the store's data is local and durable, and nothing about reopening it needs the bucket first. 13 rounds before it on GCS with 250 and 1000 ms added, host losses and VM crashes, passed: the crash and the reboot came while a backlog was uploading. For after TASK-165.9 (the uploader moves into extent_store): a store opened again while its uploader drains should take the uploader over, or the drain go on in the background, not hold the store.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A VM booted again on a store whose last VM crashed opens it at once, whatever its uploader has left to upload, shown by a test with a slow bucket
- [ ] #2 fxfs-guest-soak adds latency to real buckets again (its Plan::latency_ms no longer zeroed for them)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
