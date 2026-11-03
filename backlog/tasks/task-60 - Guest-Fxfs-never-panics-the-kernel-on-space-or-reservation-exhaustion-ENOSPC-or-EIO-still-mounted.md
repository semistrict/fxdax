---
id: TASK-60
title: >-
  Guest Fxfs never panics the kernel on space or reservation exhaustion: ENOSPC
  or EIO, still mounted
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-21 19:54'
updated_date: '2026-11-03 07:32'
labels: []
dependencies: []
priority: high
ordinal: 169000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxfs_rt_panic turns Rust panics in Fxfs (allocator.rs reservation asserts and others) into kernel panics. Space and reservation exhaustion must fail the operation (ENOSPC, or EIO) and keep the filesystem mounted; audit the panics reachable from guest operations.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Static audit of fxfs's non-test panic sites (assert/panic/unwrap/expect, ~400) for those reservation or space exhaustion reaches; the guest glue (fxfs-linux, linux-rt).
2. Journal's own transaction (make_room): its asserts become a terminated journal and an error (TASK-139's path), test with a chunk too small for a zeroing slice.
3. Dynamic: xfstests' enospc group, with and without DAX, on GCE; a model-test hunt (FXDAX_MODEL_CASES=3000) on macOS; fix what they find, each with a test.
4. scripts/test.sh on macOS, GCE xfstests generic/161 and the guest suites touched.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
05d1dd5: did_commit_transaction's MetadataReservation::Reservation branch borrows a shortfall in release builds rather than panic (debug builds still panic).

Hold branch ('Transaction was larger than metadata reservation'): now commits only what the hold has (min(txn_space, hold)), disowns and adds that, and borrows the rest in release builds; debug builds still panic. (Before, commit(txn_space) took more off the reservation's reserved count than the hold had.)

Audit of the rest: allocator.rs's remaining release asserts (move_to, forget, release_reservation, remove_reservation) and fxfs-linux's unwraps (lock poisoning, invariants) are internal-consistency checks, not exhaustion paths. Dynamic check: the guest check 'enospc' (fxfs-guest-checks[-dax], FXDAX_CHECKS=enospc) fills the filesystem with one large file, many small files, a sparse file then truncated, fallocate and mkdir, each to ENOSPC, then frees and writes again.

6dbc848: Hold-branch borrow; a shrinking truncate borrows metadata space (guest setattr), so a full filesystem's files can be cut. The enospc check (DAX only: buffered writes reserve nothing, TASK-80; fallocate reserves nothing by design) passes in the guest, but the image's fsck then warns once: TASK-82. The check stays uncommitted until TASK-82 is fixed.

The enospc guest check is kept on branch enospc-check (66f6d20b) until TASK-82's fix.

enospc check now on main (with TASK-80, TASK-82), in fxfs-guest-checks and -dax.

Handed back 2026-11-02 (triage): its own work is on main (the enospc check in fxfs-guest-checks and -dax, with TASK-80 and TASK-82). Left: TASK-60.1, the guest's infallible Rust allocations.
<!-- SECTION:NOTES:END -->
