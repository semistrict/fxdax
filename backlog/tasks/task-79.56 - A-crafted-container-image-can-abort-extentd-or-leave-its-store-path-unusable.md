---
id: TASK-79.56
title: 'A crafted container image can abort extentd, or leave its store path unusable'
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-21 21:00'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 244000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Recursive whiteout removal over deep paths overflows the stack and aborts the daemon. An out-of-range tar mtime panics the import, and the half-made store is left behind, so later imports get EEXIST.

Where: crates/fxdax/src/import.rs:457, crates/fxdax/src/import.rs:466, crates/fxdax/src/import.rs:493, crates/fxdax/src/import.rs:66, crates/fxdax/src/extentd/stores.rs:377.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-39.5. TASK-39.5 (Done) built the import. No task covers recursive whiteout removal, out-of-range tar mtimes, or cleanup on unwind.
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
A crafted image could overflow extentd's stack through recursive whiteout removal. An out-of-range tar time panicked the import and left the half-made store behind, so later imports got EEXIST. Whiteout removal now runs from a heap stack of its own (87ff98c5), and times use checked arithmetic and are refused with EINVAL (83adc6ac, c25e41bb). make_store's guard deletes the store on an error, a panic or a dropped future (714a49f7, 8679f111). Before, the deep-image test aborted with SIGILL, the time tests panicked on an overflow, and the panic test found the store half made. All of them, and extentd's imports_that_fail_leave_no_store, pass on Linux (GCE), and all but that Linux-only test pass on macOS.
<!-- SECTION:FINAL_SUMMARY:END -->
