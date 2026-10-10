---
id: TASK-25.6
title: 'libkrun: size fxdax''s in-process kernel'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-10 19:08'
updated_date: '2026-10-10 21:22'
labels: []
dependencies: []
references:
  - docs/measurements/gce-fs-bench-2026-10-10T1823Z.md
parent_task_id: TASK-25
type: feature
ordinal: 31000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxdax's in-process kernel keeps files' pages in 1 GiB of memory unless fxdax::kernel::configure says otherwise before it starts, and libkrun has no way to say it, so a VMM's fxdax cache is capped at 1 GiB whatever the guest or host (the benchmark keeps its data at 256 MiB for this; docs/measurements/gce-fs-bench-2026-10-10T1823Z.md). Add a libkrun API, before the first fxdax device, that sizes it, with its pressure thresholds scaled.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A libkrun C API configures fxdax's kernel memory, documented in libkrun.h, and failing once the kernel has started
- [x] #2 A guest test sets it and reads a file larger than the default
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. fxdax: kernel::Config::with_memory scales the default thresholds; configure fails with EBUSY or EINVAL. 2. libkrun: krun_set_fxdax_memory, documented in libkrun.h. 3. Guest test virtiofs-fxdax-memory: sets 1.5 GiB, reads back a 1.25 GiB file.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
fxdax 1f3f1ab, libkrun 33871fe. Validation: crates/fxdax/tests/kernel.rs (scaling, EBUSY once configured or started, EINVAL); virtiofs-fxdax-memory passes on macOS (HVF) and on GCE under KVM (scripts/test-gce.sh: 1864 workspace tests and the 4 fxdax guest tests passed); workspace on macOS and in Lima before (1858, 1862 passed).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
krun_set_fxdax_memory(memory_bytes) sizes fxdax's in-process kernel memory, once, before a device starts it, with its memory pressure thresholds scaled (fxdax::kernel::Config::with_memory); -EBUSY once set or started, -EINVAL for too little. Documented in libkrun.h and the README. Verified by fxdax's kernel tests and the guest test virtiofs-fxdax-memory (1.5 GiB, a 1.25 GiB file read back), on macOS and on GCE under KVM.
<!-- SECTION:FINAL_SUMMARY:END -->
