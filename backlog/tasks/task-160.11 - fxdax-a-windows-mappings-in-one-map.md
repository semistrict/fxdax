---
id: TASK-160.11
title: 'fxdax: a window''s mappings in one map'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-03 13:08'
updated_date: '2026-11-06 02:22'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 582000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Re-checked 2026-11-05 against crates/fxdax/src/dax*, uffd.rs, extents.rs and extentd, after writable DAX's deletion (37c331a6c).

Dax<K> still keeps a second interval map and by-source index (mappings, kept) beside WindowState's, kept in step by hand under another lock (the store's dax). Its K, extents.rs's Mapped, holds the source's identity, which the window's by_source already has, and what a snapshot needs: the extent, and offsets encoded from the MAP's start (window_offset, offset), where the window keeps a page number updated as mappings split. Explains TASK-79.10.

DEFERRED went with writable DAX. What replaced it, the wire's thread-local CORKED (AsyncSender::cork), is per thread and asserts one sender per thread; the window begins a change before taking its lock, so a stage can't hold back a change itself. Found on the way: RemoteStage gives a stage operation its ID before queueing it, so two windows of one VM process (or an eviction and a MAP) may put IDs on the wire out of order, and the VM process's acknowledgement of the later lets go of the pages parked behind the earlier, still reachable (TASK-79.9's quarantine). take_change's comment knew IDs may be out of order on the wire.

Proposal: the window's mappings are the one map. A source says what its pages are (a sealed extent's ID, or zeros), so Dax needs no K. The window makes a change under its lock; RemoteStage holds a change's operations back and sends them at once as it ends, each given its ID as it is queued.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 One map per window
- [x] #2 dax shuttle tests pass
- [x] #3 Deferral is the RemoteStage's: no thread-local, and a window's change is made under its lock
- [x] #4 Stage operations' IDs reach the VM process in the order given
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Source::origin (a sealed extent's ID, or zeros) replaces is_zeros; Dax loses K, Keep, its mappings and kept: map, map_zeros, unmap, map_zeros_over (returns the ranges it zeroed) and mappings (WindowMapping, from the window's page numbers) work on the window's one map. extents.rs's Mapped and Maps go.
2. Window begins its stage's change with its lock held (Window::change), map_zeros_over one change under one lock (map_locked, unmap_locked).
3. AsyncSender: cork, uncork, corked and CORKED go; send_all queues a batch, its headers made under the queue's lock. RemoteStage holds a change's ops (change: Mutex<Option<Vec<StageOp>>>), sends them with send_all as it ends, IDs given inside (Acks::sent); Parking::wait's corked assertion goes, a wait inside a change being impossible.
4. Shuttle: Remapping models the per-window hold-back, failing a change begun in another or an op from another thread mid-change; red with the window's change begun outside its lock.
5. Tests: dax unit tests on the one map; wire test of batch order under 8 threads; macOS workspace+shuttle, Linux in Docker, GCE DAX suites jailed.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Design (90a62d342, bcd1c9b4d):
- One map: Dax<K>'s mappings and kept are gone, with Keep and extents.rs's Mapped/Maps. K held the source's identity (the window's by_source has it) and the extent a snapshot records, which the cached extent knows: Source::origin (Extent(id) or Zeros) replaces is_zeros, and Dax::mappings builds WindowMappings from the window's Mapping (offset = page * page size). Dax is no longer generic; its map/unmap no longer return a Result that never failed. A free's zeros go over every range of the source under one window lock and one change (Window::map_zeros_over), returning the ranges for the store's change log. Decided against the proposal's Mapping-carries-K (OPEN_QUESTIONS.md, Decisions).
- Deferral in the RemoteStage: DEFERRED had gone with writable DAX; its successor was the wire's thread-local CORKED. The window now begins and ends its change with its lock held (Window::change), so a window's stage calls are serialized, changes can't nest, and no wait happens inside one: Parking::wait's corked assertion went, the case being impossible. RemoteStage keeps a change's ops (Mutex<Option<Vec<StageOp>>>) and sends them with AsyncSender::send_all as it ends; cork/uncork/corked/CORKED are gone.
- Bug found by reading: RemoteStage gave an op its ID (Acks::next) before queueing it, so two windows of one VM process, or an eviction and a MAP, could put IDs on the wire out of order; the VM process's ack of the later released the pages parked behind the earlier while the guest could still reach them. Now send_all makes the headers under the queue's lock, IDs given inside (Acks::sent). Blind spot: nothing checked wire order of IDs across threads; take_change's comment even assumed it. Test: wire's sends_of_a_batch_arrive_in_the_order_made (8 threads x 100 batches), red 10/10 with the headers made before taking the lock, green 200/200. Looked around: the only other ID-giving sender is the client's Made::Ack, one thread. Not to be confused with dead-ends/deferred-stage-unmaps.md: that deferral left the MMU lock; this hold-back ends inside the window's lock.
- Shuttle widened: uffd's Remapping now holds back per window, as RemoteStage does, and fails a change begun in another or an op from another thread mid-change. Red with copy's change begun before the lock: remapped_stores_race_frees_randomly and _by_pct fail (a change begun in another).
- Structure: the class (two maps kept in step; per-thread state shared by stages) is gone, not guarded.
- Overlap with the stores_under_rebinds / PRIVATE ENOMEM regression: this touches Window::copy only in where its change begins (now under the lock); private-page counting and make_private are unchanged.
Runs: macOS FXDAX_TEST_PARTS='workspace shuttle' scripts/test.sh: all pass (workspace 2068/2068; fxdax Shuttle 32/32, the userfaultfd models being Linux-only; the other crates' Shuttle suites pass). Linux (Docker, arm64): cargo nextest -p fxdax 456/463, the 7 failures the known environment ones (eu-stack x3, grants ENOENT x3, build stamp); seccomp test passes; Shuttle 55/55. The intermediate commit 90a62d342 alone: dax/extents/extentd unit tests 194/194, Shuttle 55/55.

GCE r600 (us-west1-b, n2-standard-4, jailed, FXDAX_WORKSPACE_TESTS=0, the tree of bcd1c9b4d before formatting): fxfs-guest-dax, fxfs-guest-checks-dax-small (69 s of 368), fxfs-guest-crash-dax, fxfs-guest-bigmap-dax (143 s of 3600), fxfs-guest-migrate-tenant-dax: 5/5 OK, no unexplained refusals, no cores. Not run: fxfs-guest-checks-dax and pgbench-dax-forked (the regression being bisected elsewhere).

Verified on main, 2026-11-05 (code of dde8d58d3; 752515020 adds backlog only): GCE r691, the Linux workspace, 2302 of 2303, the one failure TASK-187's a_store_after_a_workers_write_waits_for_one_lift (TASK-190; its fix 788e3b6b4 is on land-p, not main); GCE r690, the e2e suite (--tag e2e, 15 cases) jailed with the production seccomp filter, 15 of 15; CI 42137709782 on dde8d58d3 green (workspace 2303 of 2303, sim, shuttle, clippy scripts, guest, guest-sim); macOS FXDAX_TEST_PARTS=workspace scripts/test.sh at 752515020, 2118 of 2118; scripts/upstream-changes.py --check at 752515020 passes. AC #1: Dax (dax.rs:1160) holds the window and its zeros, no map of its own; extents.rs's Mapped and Maps are gone. AC #2: fxdax's Shuttle suite 59 of 59 in CI 42137709782's shuttle job, dax::uffd's remapped_stores_race_frees_randomly and _by_pct among them. AC #3: rg finds no CORKED, cork or uncork in crates/fxdax. AC #4: extentd::wire's sends_of_a_batch_arrive_in_the_order_made passes in r691. r690's DAX suites, jailed, pass.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
One map per window (Dax lost K and its second map); a window's change is held back by its RemoteStage under the window's lock, the thread-local cork gone; stage IDs given under the queue's lock, in wire order. On main: 065f54f3e, 92ff6fc16. Verified: GCE r691 (workspace 2302 of 2303, the failure TASK-190's), CI 42137709782, macOS workspace 2118 of 2118 at 752515020; fxdax Shuttle 59 of 59 in CI; GCE r690's jailed DAX suites.
<!-- SECTION:FINAL_SUMMARY:END -->
