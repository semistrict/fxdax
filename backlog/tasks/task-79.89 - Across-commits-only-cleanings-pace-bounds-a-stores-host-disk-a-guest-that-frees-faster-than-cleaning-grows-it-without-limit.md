---
id: TASK-79.89
title: >-
  Across commits only cleaning's pace bounds a store's host disk: a guest that
  frees faster than cleaning grows it without limit
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-23 13:18'
updated_date: '2026-10-24 02:46'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 348000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Left by fixing TASK-79.2's ENOSPC regression on Linux (the space fix on review-r1: freed, sealed dead space takes none of a branch's room). TASK-79.2 had bounded a guest's host disk by refusing room (ENOSPC) once dead space passed the capacity's worth; that refused an honest DAX guest's journal writes with its store a tenth used (the uploader releasing its kept frees turned 320 MiB into dead space at once), and cleaning's copies, which wait for the guest's next commit, could never land, so the store stayed full for good. Now only what a writer wasted since its last commit counts past the bound; across commits the host disk is bounded by cleaning alone: one pass of at most CLEAN_PASS (64 MiB, host_store.rs) per guest commit and per 50 ms turn, copying from victims below CLEAN_BELOW live. A guest whose segments stay just above CLEAN_BELOW live, or that frees more than a pass takes back per commit, grows dead space without limit. Letting writes wait until cleaning's pending copies cover the garbage was considered and rejected: it turns ENOSPC into a hang whenever one pass cannot cover enough (dead space in segments at or above CLEAN_BELOW live, whose live bytes include the uploader's kept frees; victims whose live data exceeds a pass or the open segment). A bound that neither refuses an honest guest nor deadlocks probably needs moves outside the open segment named by a footer, or cleaning sized to what commits free. Needs a design decision.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A guest that writes and frees at any pace, committing, keeps its store's host files within a stated bound (a multiple of its capacity), shown by a test
- [x] #2 No honest guest's write or commit is refused, and none waits without end, at any fill below its capacity, shown by the guest checks with DAX and pgbench
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
User decision (2026-10-23): a hard cap on a store's host files at a multiple of its capacity (3x), past which writes are refused (ENOSPC); honest guests never near it. Consider making cleaning's 64 MiB mark relative to capacity too (TASK-79.91, after it is measured again on main).

User decision (2026-10-24): the 4x effective ceiling is accepted: 3x capacity, plus a credit of at most one capacity's worth for the writes a commit comes with, which lets the commit land cleaning's copies, so commits never get stuck at the cap (r8/diskcap, 09463ca0).

User decision (2026-10-24): what the uploader holds (frees kept until uploaded, segments not yet uploaded) is left out of the cap; it is bounded by the upload lag bound, which makes commits wait rather than fail.

r8/diskcap cf3d4322: what the uploader holds (segments not yet uploaded, their whole files; frees kept, by their part's stored share; kept own files) is left out of the cap, counted in disk bytes. Tests what_an_uploader_keeps_counts_not_towards_the_bound and frees_an_uploader_keeps_count_not_towards_the_bound fail before ('the extent store is full') and pass after. Left: extentd's 'found the store full' log prints only used/capacity, so a refusal by the files' bound reads as a contradiction there; it should name which bound refused.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A store's own host files are bounded: a write is refused (NoSpace) once the branch's files, counted in disk bytes (its own segments' files, own extent files, open extents' raw files), would pass three times its capacity; commits, seals and cleaning's copies are never refused, and the writes a commit comes with may use a credit of at most one capacity's worth for what that commit's cleaning copies take back, so commits never wedge at the cap (the user accepted the 4x ceiling). What the uploader holds (segments not yet uploaded, frees kept for it) is left out, bounded by the upload lag bound instead (the user's decision). Tests: files_past_three_times_the_capacity_refuse_writes, the_write_a_commit_comes_with_takes_what_the_commit_takes_back, what_an_uploader_keeps_counts_not_towards_the_bound, frees_an_uploader_keeps_count_not_towards_the_bound (each failing before), cleaning_at_its_pace_keeps_a_churning_guest_from_the_bound, the Shuttle writes_racing_at_the_bound_by_pct with needs_raw_files_counted_as_writes_begin. macOS: workspace 2493/2493 and every Shuttle suite; Linux (GCE fxdax-test-r8m): workspace 2571/2571, Shuttle suites, and the guest cases fxfs-guest, fxfs-guest-dax, fxfs-guest-checks, fxfs-guest-checks-dax, extent-guest, fxfs-guest-pgbench and fxfs-guest-pgbench-dax, all OK, with a guest kernel built from the tree; scripts/upstream-changes.py --check passes. Pushed 430bb064..3ca2e731.
<!-- SECTION:FINAL_SUMMARY:END -->
