---
id: TASK-153.16
title: 'To try: a panic under a store lock leaves the store usable'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-02 01:59'
updated_date: '2026-11-06 06:56'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: medium
type: task
ordinal: 564000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-104, TASK-105 and TASK-150.20 are one pattern: a panic with a store lock held poisons it, and every later request of that guest dies.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Decided: what a poisoned store does, and a test of it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Decide what a poisoned store does: fail-stop per store (fence), usable again reopened as of its last commit. 2. crate::fence: Fence, StoreMutex, StoreRwLock, a guard fencing as a panic drops it; every ExtentStore lock through it. 3. extentd: a fenced store's requests answer EIO unserved; a panic serving a request, flushing or cleaning fences it (past its locks); teardown doesn't look at a fenced store's window. 4. OpenPages' drop tolerates a poisoned table. 5. Tests: fence unit tests; extents a_panic_holding_a_lock_fences_the_store; extentd a_panic_holding_a_stores_lock_fences_it_alone and a_panic_past_a_stores_locks_fences_it; mutants red. 6. Decision in OPEN_QUESTIONS.md. 7. scripts/test.sh on macOS, GCE workspace.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
DECISION: fail-stop per store. A store a panic went through is fenced: every request after it fails EIO, unserved and without panicking. The guest's filesystem stops, and its VM with it. When the connection ends, the store is closed as a writer that went with writes uncommitted (is_quiescent's EIO retires it, its followers are cut), and the next open has it as of its last commit. Rejected: recovering the lock's state (nothing proves it consistent across the store's locks and the layers beneath; the three instances happened to panic before changing anything); fencing and reopening in place under the running guest (its filesystem holds open extents a reopen loses, and no protocol makes a writer's guest remount); poison to EIO lock by lock (a store poisoned in open could still map, through dax, what the panic left half done). Recorded in OPEN_QUESTIONS.md # Decisions with how to undo it.
HOW: crate::fence (Fence, StoreMutex, StoreRwLock, Held). Every ExtentStore lock (open, commit_buffer, dax, opening, changes, recently_sealed, a follower's view, an attached base's) can only be taken through the store's fence: EIO once it is fenced, and a guard dropped during a panic fences it. So no store lock is ever locked poisoned. extentd: serve_in_turn answers a fenced store's requests EIO without serving them. Answer::drop during a panic fences the store, which covers panics beneath the store's locks, as in extent_store (TASK-79.1, TASK-79.80). flush_behind fences a store whose flush or clean panicked. reaches_pages doesn't look at a fenced store's window. OpenPages' drop no longer panics on a poisoned table: dropped during a panic's unwinding, it would have panicked again and aborted the whole daemon.
BLIND SPOT: no test ever panicked under a store lock. The three bugs were each fixed at their expect, and the poisoning that turned one panic into a dead store stayed. pool.rs's jobs_after_a_panicking_one_run checked that the pool survives a panic, not the store.
TESTS: fence::tests (3); extents::tests::a_panic_holding_a_lock_fences_the_store (panics under open, then shows read, write, free, commit, map, unmap, window_mappings, is_quiescent and take_over all EIO, then reopens: last commit there, the uncommitted extent ENOENT, a new write and commit served); extentd::tests::a_panic_holding_a_stores_lock_fences_it_alone (through the daemon: one panic only, another VM's store serves on, the reopened store serves) and a_panic_past_a_stores_locks_fences_it (the extentd::commit failpoint set to panic). Red with the fix removed: guards that don't fence turned all three fence/extents tests red (macOS); without Answer::drop's fence and serve_in_turn's refusal, both extentd tests went red (GCE r t15316-mut2: info Ok(16777216), read Ok(data), each where EIO was expected).
LOOKED AROUND: what's left is filed as TASK-149.24: the process-wide extent cache lock (a panic there poisons every tenant, so fencing one store can't help), Window::state shared with the connection's DaxWindow, the SecondStage's end_change/end_deferring run from Drop guards, followers sharing the writer's extent_store::Store (one panic each before they're fenced), and extent_store's own locks, left alone to stay clear of TASK-158.
STRUCTURE: this is the structural fix TASK-150.20's notes asked for: the class (a std Mutex's poison turning one panic into a dead store) can no longer happen for a store's own locks, because the lock type has no way to lock without the fence.
RESULTS: macOS scripts/test.sh exit 0 (2,014 of 2,014 workspace, Shuttle and scripts suites). GCE workspace (fxdax-test-t15316-ws2, us-west1-a): 2,175 of 2,175, the Shuttle suites too. upstream-changes.py --check passes. All hosts deleted.

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**A panic through a store fences it: fail-stop, usable again once reopened (TASK-153.16, 2026-11-03)**

A panic holding a store's lock poisoned it, and every later request of its guest panicked too (TASK-104, 105, 150.20; in `extent_store`, 79.1 and 79.80). Decided: the store is fenced. Every request after the panic fails with `EIO` without panicking, the guest's filesystem stops, and so does its VM. When the connection ends, the store is closed like a writer that left writes uncommitted: it is retired and its followers are cut off. The next open sees it as of its last commit, as after a crash. `crate::fence`: each `ExtentStore` lock can only be taken through the store's fence, and a guard dropped during a panic sets it. The daemon also fences a store when a request, flush or clean of it panics below the locks.
**Instead:** (1) recover the lock's state: nothing proves it consistent across the store's 7 locks and the layers beneath them, even though the three instances happened to panic before changing anything. (2) Fence and reopen in place under the running guest: its filesystem holds the open extents a reopen would lose, and no protocol tells a writer's guest to remount, the way `ESTALE` tells a follower. (3) Turn poison into `EIO` lock by lock: a store whose `open` lock is poisoned would still map from its unpoisoned `dax` lock what the panic left half done.
**Undoing it:** for (1), `StoreMutex::lock` takes `PoisonError::into_inner` and stops checking the fence. The daemon's fencing is in `serve_in_turn`, `Answer::drop` and `flush_behind`.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Decided: a store a panic went through is fenced, fail-stop. Its requests fail EIO without panicking at the poison, nothing reads what the panic left half done, and once reopened the store serves as of its last commit. crate::fence makes every ExtentStore lock go through the store's fence, and the daemon fences a store whose request, flush or clean panicked beneath its locks. Proven by fence/extents unit tests (macOS and Linux) and two extentd tests (Linux), each red with its part of the fix removed. macOS suite and GCE workspace (2,175 of 2,175) pass. Remaining non-store locks: TASK-149.24.
<!-- SECTION:FINAL_SUMMARY:END -->
