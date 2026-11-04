---
id: TASK-162
title: >-
  Fxfs: the per-use checks TASK-153.12's trust boundary makes dead go, with
  their tests
status: Done
assignee:
  - '@claude'
created_date: '2026-11-04 06:22'
updated_date: '2026-11-04 10:39'
labels: []
dependencies: []
priority: medium
type: task
ordinal: 607000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The user approved (2026-11-04) removing the checks TASK-149.9 added where records are used, now dead for anything from an image since every layer item is checked as it's read (TASK-153.12): entries_in in extent_record.rs (offset or shrunk entries that don't fit), the read path's bitmap length check in store_object_handle.rs, the tolerance in ExtentValue::offset_by/shrunk; and their tests extent_entries_that_do_not_fit and the bitmap half of test_reads_of_what_an_image_says. The files come back toward upstream.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The checks are gone, the files nearer upstream, upstream-changes.py --check passing
- [x] #2 The boundary's tests still refuse each record those checks caught
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The per-use checks TASK-153.12's trust boundary made dead for what an image says are gone: offset_by and shrunk back to upstream's, entries_in and the read path's bitmap check removed with extent_entries_that_do_not_fit and the bitmap half of test_reads_of_what_an_image_says; corrupt_tests gained the two records they caught that no case did (an offset past any end, bits short of the key). Verified: macOS scripts/test.sh (workspace 2,083/2,083, Shuttle, scripts); GitHub CI on the branch (Linux workspace, guest-sim 3,000 seeds); GCE r539 at 8da75b3cc with Firecracker a2db2b3b0 (fxfs-guest-dax, checks-dax, -delay, -small, script-full-dax, xfstests-fxfs-guest-dax, checks, crash-dax, pgbench-dax all pass).
<!-- SECTION:FINAL_SUMMARY:END -->
