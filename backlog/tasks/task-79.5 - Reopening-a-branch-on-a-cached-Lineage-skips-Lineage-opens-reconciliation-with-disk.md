---
id: TASK-79.5
title: >-
  Reopening a branch on a cached Lineage skips Lineage::open's reconciliation
  with disk
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 193000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
host.rs reuses any live Lineage, and caches keep closed Stores alive. hold re-takes the flock without re-reading, so another process's fork is invisible and the reopen deletes an index file the fork needs, after which no branch of the store opens. reset keeps a failed flush's non-durable releases (doomed files, dropped marks), so the recommitted epoch deletes segments that the branch or a sibling fork still holds. Raw open-extent files are swept only by Lineage::open, so after an abandoned write, a failed store or a reopen, a re-minted large extent fails create_new with EEXIST and the guest gets EIO.

Where: crates/extent_store/src/host.rs:41, crates/extent_store/src/lineage.rs:197, crates/extent_store/src/lineage.rs:307, crates/extent_store/src/lineage.rs:399, crates/extent_store/src/lineage.rs:593, crates/extent_store/src/lineage.rs:755, crates/extent_store/src/store.rs:1068, crates/extent_store/src/store.rs:1398, crates/extent_store/src/store.rs:1999.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-28.2, TASK-29.2, TASK-31, TASK-43.8. No task covers cached-Lineage reuse skipping Lineage::open's reconciliation (hold without a reload, reset keeping non-durable releases, open-extents not swept, EEXIST). TASK-28.2 added the per-process Lineage registry. TASK-29.2 put the open-extents sweep in Lineage::open only. TASK-43.8 is about the flock wait.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test reproduces the failure on the code before the fix
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Reopening a branch on a cached Lineage now does the reconciliation that Lineage::open does. Holding the directory again reloads branch records another process changed, recovery takes back what non-durable commits let go of, and opening a branch removes raw open-extent files left by its last store. A large extent re-minted before the next flush now replaces a leftover raw file rather than failing with EEXIST. The MemDir test model's create over an existing path was fixed along the way. Verified by tests that failed before each fix and now pass on macOS and Linux (GCE), together with the workspace runs on both.
<!-- SECTION:FINAL_SUMMARY:END -->
