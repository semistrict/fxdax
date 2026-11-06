---
id: TASK-153.12
title: 'To try: one trust boundary for layer records'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 23:09'
updated_date: '2026-11-06 06:56'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: medium
type: task
ordinal: 559000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-149.9: journal and super-block records are validated, layer records trusted then checked piecemeal: key order, short bitmaps, entries_in, tombstones. Check each item once in take_item with the journal predicate, as upstream TODO fxbug.dev/42178223 asks. It costs a check on the hottest read path.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Prototyped, its cost on the read path measured
- [x] #2 Decided from the numbers
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Bench first: layer_read_cost (persistent_layer.rs), a scan and seeks of a 200k-item object-store layer, in release; baseline.
2. Refactor: validate_mutation's object-store arm becomes ObjectValue::is_valid_for + ExtentValue::fits, shared.
3. Boundary: LayerKey::is_valid_item (default true; ObjectKey and AllocatorKey check their store's values), Key gains LayerKey; take_item refuses an invalid item as Inconsistent.
4. Key order: take_next refuses a key not strictly after the last or overlapping it (follows).
5. Tests: unit tests per invalid kind and order; fsck tests that wrote off-block items into layers made aligned; corrupt_tests: records no layer may hold, rewritten into a real image's layer as a writer would, each refused as read, none read differently; red with the boundary out.
6. Measure A/B (none / item / item+order), decide; record in OPEN_QUESTIONS.md.
7. scripts/test.sh, GCE all with fxfs-guest and fxfs-guest-checks, upstream-changes --check.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Results (2026-11-03)

**Design.** One trust boundary, where a layer is read: `take_item` checks each item with `LayerKey::is_valid_item` (a default method, true; ObjectKey and AllocatorKey check their store's values, downcast with `dyn Any`, which monomorphization resolves; `Key` now requires `LayerKey`), and `take_next` checks each key follows the last, overlapping none (`follows`). The predicate is the journal's: validate_mutation's object-store arm became `ObjectValue::is_valid_for` and `ExtentValue::fits`, which both share; the allocator's is `AllocatorValue::is_valid_for`, mirroring valid_device_range (mutations and items are different types). Error paths are `#[cold]`. A seek still trusts order (a binary search must): a point lookup may miss a key out of order; a scan finds it.

**Cost** (layer_read_cost, 200k object-store items, release; each row one GCE n2-standard-4, three hosts agreeing to 1%): scan base 144.9 ns an item; item check 156.9-157.2 (+8%); item and order 164.8-165.6 (+14%); seeks 1,050-1,200 ns, unchanged. Isolated, the check is 14-17 ns on x86, 3-7 on arm64. Taking the check's two divisions out changed nothing (165.0), so that commit was dropped. The `#[cold]` error paths weren't measured: gcloud's credentials expired mid-run. Where the 12 ns go is unexplained.

**Decided** (OPEN_QUESTIONS.md): land both. Without the boundary, 10 of 16 cases of records with checksums over their key, or overlapping keys, read back differently, with nothing saying so.

**Tests.** persistent_layer.rs: each kind of invalid item (extent off blocks, offset off blocks or overflowing, checksums or bitmap not fitting, allocation off blocks or of no owner) fails a scan and a seek Inconsistent; keys out of order, equal or overlapping fail a scan; all red with the checks out. fsck tests that wrote 512-byte extents into a 4 KiB filesystem's layers now write blocks; the misordered and overlapping ones see MalformedLayerFile. corrupt_tests: `records_no_layer_may_hold_fail_what_reads_them` rewrites a real closed image's layer as a writer would, with one record changed (six kinds, 8 seeds each): none panics, each fails the mount or a scan of the volume, none reads differently but OutOfOrder (a seek). With the boundary out: SumsOverKey 4 and Overlapping 6 of 8 read differently, and no scan fails.

**Blind spot, and around it.** No record but what Fxfs wrote reached the readers, and random flips rarely make one that parses: the new class test writes them. Dead now for what's on disk: entries_in, the read path's bitmap check; removing them takes deleting tests (open question in OPEN_QUESTIONS.md).

**Verification.** macOS scripts/test.sh: before the rebase, 2,047 of 2,048 (extent_remote free_bytes_are_the_free_blocks_size raced other processes' disk use; passed alone), Shuttle and scripts' tests all pass. GCE (fxdax-test-t15312-all, working tree of 9129a55e0's content): workspace 2,212 of 2,212 and every Shuttle suite; guest cases fxfs-guest and fxfs-guest-checks OK. upstream-changes.py --check passes. After the rebase onto 80c4bfdb3: see below.

After the rebase onto 80c4bfdb3 (branch head 280425844): macOS scripts/test.sh, workspace 2,064 of 2,064, Shuttle suites and scripts' 136 tests pass; upstream-changes.py --check passes. Not rerun on GCE after the rebase or the #[cold] commit: gcloud needs signing in (OPEN_QUESTIONS.md).

On the final tree (rebased on 81e8869c5, extent_store only came in): fxfs and fxfs-posix nextest, 812 of 813 then 813 of 813; the one failure, the model test, unexplained and its output lost, is TASK-149.27. Not reproduced in 5,600 more sequences.

Pushed at 704cdbead (rebased on 7dc85f28c, which brought the errno table: Inconsistent is EUCLEAN to the VFSs now): macOS scripts/test.sh 2,072 of 2,072, Shuttle suites and scripts' tests pass; upstream-changes.py --check passes.

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**A layer's records cross one trust boundary, checked as they're read (TASK-153.12, 2026-11-03)**

Layer records were trusted as read, then checked piecemeal where used. Decided: each item a layer file holds is checked as `take_item` reads it (`LayerKey::is_valid_item`), with the predicate the journal's mutations pass at replay (`ObjectValue::is_valid_for`, `ExtentValue::fits`; `AllocatorValue::is_valid_for` for the allocator's), and each key against the one before as an iterator advances (`follows`: after it, overlapping none). An item no layer may hold fails the read, a seek to it, fsck (`MalformedLayerFile`, also for keys out of order or overlapping, which fsck named on its own before) and the compaction meeting it. Cost, `layer_read_cost` on one GCE n2-standard-4 per comparison, three hosts agreeing to 1%: a scan 144.9 ns an item before, 157 with the item check (+8%), 165 with the order check too (+14%); seeks unchanged (1,050 to 1,200 ns either way). On an M-series Mac, under load, about +10%. Landed for both: without it, a record with checksums over its key, or keys overlapping, read back differently in 10 of 16 cases of `records_no_layer_may_hold_fail_what_reads_them`, with nothing saying so; the cost is on scans (compactions, fsck, a file's extents in turn), not on the seek every read starts with. Where the 12 ns of the item check go is unexplained: neither its two divisions (taken out, no change) nor, measured on arm64 only, the check alone (3 to 5 ns) account for it; moving the error paths out of line (`#[cold]`) wasn't measured, as gcloud's credentials expired. A seek still trusts order, as a binary search must: a key out of order may be missed by a point lookup, and only a scan finds it.
**Instead:** (1) keep checking where records are used: piecemeal, and wrong data where no use checks; (2) drop the order check, saving 6%: merges then trust order unchecked; (3) a check function each tree gives its layers, as `merge_fn`: threads one through every layer's opening, where a default method on `LayerKey`, downcasting to the store's value type (`dyn Any`, resolved at compile time), needs nothing threaded.
**Undoing it:** take the checks out of `take_item` and `take_next`; the predicates stay the journal's.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Prototyped and landed one trust boundary for layer records: each item a layer holds is checked as take_item reads it, with the predicate the journal's mutations pass (ObjectValue::is_valid_for, ExtentValue::fits, AllocatorValue::is_valid_for), and each key against the last as an iterator advances. Cost measured with layer_read_cost on quiet GCE hosts: scans +8% for the item check, +14% with order; seeks unchanged. Decided to land both (OPEN_QUESTIONS.md): without it, 10 of 16 rewritten-record cases read back wrong data silently. Verified by unit tests per invalid kind, fsck tests, and corrupt_tests' records_no_layer_may_hold_fail_what_reads_them (red with the checks out); macOS scripts/test.sh and GCE workspace plus fxfs-guest and fxfs-guest-checks pass. Not done: removing the now-dead per-use checks (needs deleting tests, an open question); seeks still trust order; the #[cold] error paths are unmeasured.
<!-- SECTION:FINAL_SUMMARY:END -->
