---
id: TASK-25.4
title: 'DAX: batch dirty transitions of written pages'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-10 19:08'
updated_date: '2026-10-10 20:56'
labels: []
dependencies: []
references:
  - docs/measurements/gce-fs-bench-2026-10-10T1823Z.md
parent_task_id: TASK-25
type: feature
ordinal: 29000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sequential writes through a DAX window run at 35 MiB/s, against 58 without DAX and 107 on ext4, and mmap writes at a quarter of ext4's rate (docs/measurements/gce-fs-bench-2026-10-10T1823Z.md): each first store to a clean page faults, and the kernel asks the pager to dirty it, a page at a time. Dirty runs of pages per fault, as the pager allows, and make stores to newly written pages not fault again.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Sequential DAX writes at least as fast as without DAX, and mmap writes within 2x of ext4, measured on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Faults on a window that store to the page after the last one a store faulted on dirty and map a run of pages (up to 16); random stores still dirty one page. 2. A write fault on a page not mapped maps the pages after it read-only once the faulting thread is woken, so that the next stores fault as stores (write-protect faults), which runs apply to. 3. Fix the second stage's race this exposed: new memory inaccessible or read-only until registered; libkrun retries KVM_RUN failing with EFAULT for a moment. 4. Profile what is left: TLB flushes from changes of protection made a page at a time; make them in runs, and skip those that change nothing. 5. Measure 3 iterations against no DAX.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Step 1 alone (T2025Z) changed nothing: fio fallocates the file and its stores fault on pages not mapped, which userfaultfd reports as writes whether the guest's or KVM faulting a load in for writing, so they cannot start runs. Step 2 crashed the guest (T2029Z to T2037Z): KVM_RUN failed with EFAULT, the second stage's VMA replacement racing a vCPU's store (bisected). Step 3 fixed it (T2043Z, 12 retried episodes in 3 iterations). Sequential DAX writes 35 -> 47 MiB/s (no DAX ~55). Committed 2f24a88, libkrun 8d4cfe9.

Profile (T2047Z): TLB shootdowns, KVM zapping its tables and flushing vCPUs' TLBs dominate: each change of protection to the window's host mapping, a page at a time. Batched into runs, unchanged ones skipped (4f7a63c). T2050Z, 3 iterations: sequential DAX writes 60.2 MiB/s, no DAX 58.2; mmap random writes 4,562/s (ext4 6,348 in T1823Z, 1.39x). Validation: scripts/test.sh 1858 passed on macOS, scripts/test-linux.sh 1862 passed in Lima, scripts/upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Sequential stores through a DAX window dirty runs of 16 pages per fault (a write fault on a page not mapped maps the pages after it read-only, so the next stores fault as stores); changes of protection to the window's host mappings are made in runs, sparing TLB flushes. Fixed a race this exposed in the userfaultfd second stage (a vCPU's access during a VMA replacement: EFAULT, an untracked store or a page of zeros): new memory stays inaccessible or read-only until registered, and libkrun retries KVM_RUN failing with EFAULT for a moment. Verified on GCE, 3 iterations (docs/measurements/gce-fs-bench-2026-10-10T2050Z): sequential DAX writes 35 -> 60 MiB/s against 58 without DAX; mmap random writes 4,562/s, 1.39x ext4's. Tests: dax_sequential_stores_dirty_runs_of_pages, window_mappings_are_made_to_the_second_stage; full suite on macOS and Linux (Lima).
<!-- SECTION:FINAL_SUMMARY:END -->
