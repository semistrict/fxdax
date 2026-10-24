---
id: TASK-42.12
title: >-
  An hour's torture without DAX: files deleted while open read otherwise, and a
  shaped file's page read as before its write
status: Done
assignee:
  - '@claude'
created_date: '2026-10-24 08:18'
updated_date: '2026-10-24 10:27'
labels:
  - bug
  - followers
dependencies: []
parent_task_id: TASK-42
priority: high
ordinal: 168000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
First hours-long soak (TASK-42 AC1), 2026-10-24, GCE n2-standard-8 (fxdax-test-follow4), release guest kernel, followers-batch1 rebased on main ff83e8bc1, FXDAX_FOLLOW_SECONDS=3600: fxfs-guest-follow-torture-dax passed (OK history); fxfs-guest-follow-torture (no DAX) failed: 'OK writer / STALE follower1 / STALE follower2 / WRONG history'. (1) Both followers report victims deleted while open that read otherwise, several the same victims in both (victim.3964, 329276, 395868, 536908, 634848): follower1 8, follower2 10, of ~195,950 batches each. (2) The history checker: reader 19 found shape.85 (object 2305843009213694037) as 0 as of commit 159350, which only models 4443998..=4444026 may show, and none from 4444022 on does (it holds 634849 from model 4444002): a page as before its write after the write's commit, as TASK-42.10 found before the fill barrier, now without DAX only and rarer (44M reads checked). The records (history-*.txt, ~2 GB) and consoles are in the session's scratchpad (soak1). The 20-second runs passed 10 of 10 before the rebase (TASK-42.10 AC3) and 2-3 of 3 after it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Each failure's mechanism identified with evidence (the records, a targeted repro)
- [x] #2 Fixed, with a test that fails without the fix
- [x] #3 fxfs-guest-follow-torture and -dax pass an hour each on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Evidence from the soak's records. 2. The writer's data writes written back (sync_file_range) before its next operation, so its operations reach the journal in its syscalls' order, as the history model assumes. 3. An hour each, DAX on and off, on GCE.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Mechanism (from soak1's records): both failures are the writer's buffered writes reaching the store in writeback order, not in the order of its operations, which the history checker's model (a prefix of the writer's operations in each commit) assumes. The shaped file: op 4444002 wrote page 85 (634849), op 4444022 extended the file (pages 136-148 zeros); the follower at commit 159350 found the extension and page 85 still 0: a background journal commit took the later size change while page 85's write was still dirty in the writer's page cache. The victims: a victim is written, then deleted three steps later, with a sync only every fourth step; deleted before its writeback, its data never reached the store, so a follower holding it open reads zeros; both followers report the same victims (writer-side). With DAX, writes reach the store at once: the DAX soak passed. Fix (the workload, as TASK-42.10 AC2 allowed): the writer writes back each data write (sync_file_range WAIT_BEFORE|WRITE|WAIT_AFTER, no commit waited for) before its next operation.

Verified (GCE, 2026-10-24, docs/measurements/gce-follow-soak-2026-10-24): with the writer writing back each write before its next operation, an hour each: fxfs-guest-follow-torture OK history (41,239,055 reads checked, ~170,000 batches a follower), -dax OK history (19,078,491 reads). The failing run before the fix (soak1, the same tree but for it) is the test that fails without the fix.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
An hour's torture without DAX found followers reading files deleted while open as zeros, and a page of the shaped file as before its write: the workload's, not the followers'. The writer's buffered writes reached its journal at writeback, after the operations it made since (a later size change committed first; a victim deleted before its writeback never held its data), while the history checker models each commit as a prefix of the writer's operations. The writer now writes back each data write (sync_file_range, no commit waited for) before its next operation. Verified: an hour each, with and without DAX, OK history on GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
