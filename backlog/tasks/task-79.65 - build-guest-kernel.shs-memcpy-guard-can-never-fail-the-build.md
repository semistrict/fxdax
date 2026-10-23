---
id: TASK-79.65
title: build-guest-kernel.sh's memcpy guard can never fail the build
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
ordinal: 253000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A bare `! pipeline` never triggers errexit, so a Rust memcpy linked into the kernel goes unnoticed. The guard also omits bcmp and strlen.

Where: scripts/build-guest-kernel.sh:143, scripts/build-guest-kernel.sh:21, scripts/build-guest-kernel.sh:131.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-64, TASK-39.6. No task covers the bare '! pipeline' memcpy guard in build-guest-kernel.sh (added in 48ed7f7). TASK-64 is about packaging that script's outputs.
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
build-guest-kernel.sh checks that fxfs_rust.o defines none of the kernel's string functions. The check was a bare '! nm | grep -q', which errexit never stops on, and it left out bcmp and strlen. The script now names memcpy, memmove, memset, memcmp, bcmp and strlen once, for both the archive strip and the check. It reads nm's whole listing first, so a failing nm stops the build, and it fails the build naming what the object defines. scripts/tests/test_build_guest_kernel.py runs the script on fakes of the build tools: 8 of its 10 cases failed before, and all pass on macOS and on Linux (GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
