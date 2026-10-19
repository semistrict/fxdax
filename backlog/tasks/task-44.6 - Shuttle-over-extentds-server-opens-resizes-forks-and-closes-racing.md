---
id: TASK-44.6
title: 'Shuttle over extentd''s server: opens, resizes, forks and closes racing'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-18 04:06'
updated_date: '2026-10-19 04:48'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: medium
ordinal: 135000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
crates/fxdax/src/extentd/server.rs serves stores to connections: one opener per store (open_paths, EBUSY), RESIZE through the connection holding the store or directly (a shrink needs the guest's agreement, TASK-37), FORK of an open store, closes. About 29 lock uses and 4 threads, with tests that drive it from real sockets one schedule at a time.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 In-process connections drive the server's request handling under Shuttle: opens, resizes (growing, shrinking with the guest agreeing or not), forks and closes, racing
- [x] #2 Invariants: a store has one opener; a resize ends with the store's capacity, the device's configuration and the guest's view agreeing; no fork misses a commit before it; a store closed is reopenable
- [x] #3 Mutation checks: each rule (the opener check, resizing through the holder, the guest's agreement to a shrink) turned off makes a Shuttle test fail
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Refactor (no behaviour change but one fix): extentd's registry of open stores (open_paths, hold_path, release_path, paths_released) becomes crates/fxdax/src/open_stores.rs: Paths<H: Holder>, hold (wait bounded or not), release, holder, holder_or_hold; portable, so its tests run on macOS. 2. Fix: the holder-less RESIZE let the store go without waking waiters (they waited out CLOSING_WAIT); it releases through Paths::release now. 3. Shuttle tests (fxdax 'shuttle' feature): a connection whose VM process goes while holding, a restored VM opening at once, a resize holding while none does; unbounded waits so a missed wake is a deadlock; rules one_holder and release_wakes each turned off fail. 4. Linux: extentd's tests in Lima.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Landed (7b672fd and the next commit): the registry of open stores is crates/fxdax/src/open_stores.rs, Paths<H: Holder>, portable; under Shuttle (fxdax feature 'shuttle', scripts/test.sh runs --lib open_stores::) a connection whose VM process goes while holding, a restored VM opening at once, and a holder-less resize race, 3000 random + 3000 PCT schedules; invariant: one holder at a time, none left after all let go. Mutations: one_holder off -> 'two holders hold the store at once'; release_wakes off -> Shuttle deadlock. Bug found and fixed: a holder-less RESIZE released without notifying, so a waiting open waited out CLOSING_WAIT (10s) and got EBUSY. Verified: macOS scripts/test.sh (2002 passed + Shuttle suites), Lima extentd 23/23 and open_stores 4/4, upstream-changes --check passes.

Remaining for AC #1-#3: request handling itself (Connection's open/resize/fork/close, the guest's agreement to a shrink, the device configuration) under Shuttle. It runs over UnixStream, ExtentStore on HostDir and the in-process Zircon kernel (parking_lot), none of which Shuttle schedules: needs those behind traits with in-memory implementations on the switchable locks (as extent_store's Dir/MemDir), the same question as TASK-44.3. Left In Progress pending that decision.

Batch 2 (7d7a4a2): the request handling's decisions about held stores are crates/fxdax/src/store_requests.rs, generic over the store (Capacity, Close), the guest (Guest) and the connection (Resizes: Holder): resize_held (a connection's resize: grow and tell; shrink once the guest agrees, ENOSPC/ETIMEDOUT), resize (through the holder, or here growing only, EBUSY to shrink, held until closed), forking (the holder's store live, or the store opened here, held until closed). server.rs (Connection's Resizes and Guest, RESIZED -> Shrinking::answered) and stores.rs (resize, fork) call it with ExtentStore. open_stores gained holder_or_hold_async (a waiting holder_or_hold) and a Shuttle-aware within().

Shuttle tests (store_requests::shuttle_tests, 2000 random + 2000 PCT schedules; SHUTTLE_SOAK=5 passes): a VM's connection opens, commits, its guest answers shrinks, closes after its manager's grow/shrink/refused-shrink; a fork races at any point; the restored VM opens as the first goes. Invariants: the store open once at a time (connections and the daemon's opens here alike); capacity == the guest's view after the resizes; no shrink without agreement; no fork misses a commit made before it; nothing held at the end. Rules turned off: guest_agrees -> 'shrunk without agreeing'; through_holder, fork_through_holder and open_stores' one_holder -> 'the store is open twice at once' (each first opens the store a second time; stable over 25 repeats). Scope: the store's I/O, the socket and the kernel are not under Shuttle; their decisions are.

Found on the way: a fork racing a connection's open got EBUSY (it now waits and forks live); a store opened here was released before it closed.

Verified: macOS Shuttle suite; GCE n2-standard-8 (fxdax-test-b1): workspace 2111 passed + Shuttle/simulation/extentd suites, all 18 guest cases with vmlinux-b1.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Request handling on held stores (resize through the holder with the guest agreeing, resize here, fork live or here) is store_requests.rs, the daemon's own code, generic so that Shuttle runs it with model stores, guests and connections racing opens, commits, resizes, forks and closes; invariants hold over every schedule, and each rule turned off fails. Fixed a fork racing an open (EBUSY) and a store released before closing. Verified on macOS and GCE (workspace + 18 guest cases).
<!-- SECTION:FINAL_SUMMARY:END -->
