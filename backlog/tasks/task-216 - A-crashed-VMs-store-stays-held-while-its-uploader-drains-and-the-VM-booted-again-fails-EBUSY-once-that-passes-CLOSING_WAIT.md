---
id: TASK-216
title: >-
  A crashed VM's store stays held while its uploader drains, and the VM booted
  again fails EBUSY once that passes CLOSING_WAIT
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-06 16:24'
updated_date: '2026-11-07 08:45'
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. A test that fails first: a store whose connection went with an upload held in flight; a VM booted again opens it within CLOSING_WAIT and reads what the first committed.
2. Fix: a writer waiting for a going connection's store has its uploader stop at once (the WAL keeps the rest, the new opening uploads it); daemon holds still wait for the drain.
3. Linux Docker for the test, macOS suites, Shuttle, CI; then the soak with latency on real buckets back.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fixed in 583b1bdba (land-k3). Still present after TASK-165.9, which made the drain explicit: ExtentStore::drop stops its uploader first, which closes, uploads and checkpoints (TASK-192); a connection's hold goes only after its store closed (Connection::drop), and hold_to_write waits CLOSING_WAIT (stores.rs:53-56, :130). Now: extent_store::upload::Remote::cut_short gives a handle that, once the uploader is stopping, has it return at once (no gather, uploads in flight dropped, no checkpoint), as a crashed host's; the store's write-ahead log keeps what wasn't acknowledged and the next opening uploads it. extentd sets it on a writer's table entry as the connection opens its store (Hold::when_wanted, store_table's Entry::Writer.wanted, called by a waiting writer's claim without the table's lock). A store taken over as it was by a writer (followers') clears it (Remote::drain_on_stop), so its later clean close drains.

Test: extentd::tests::stores_whose_vm_went_open_again_while_their_uploads_wait (GatedReads holds the going store's next segment upload alone, hold_next_write; the VM booted again opens, reads extent 2, commits durably): red before, EBUSY after 10.006 s; green after, 0.06 s, Linux Docker. macOS extent_store and fxdax 466/466; Shuttle fxdax 33, extent_store 6.

Blind spot: no test opened a store again while its predecessor's close waited on the bucket; the tests' buckets answer at once. Looking around: the daemon's holds (control API: fork, delete, drop, sync; stores.rs:145-169) wait the same CLOSING_WAIT for a going connection, and don't cut the drain, as DropStore and SyncStore count on it uploading a crashed VM's commits; a control call on a crashed VM's store under a slow bucket may still say EBUSY, which callers retry. Structure: the close's drain is right for a store not opened again here; the wait is the waiter's to cut, so the table carries the waiter's word to the holder, the one owner of who waits.
<!-- SECTION:NOTES:END -->
