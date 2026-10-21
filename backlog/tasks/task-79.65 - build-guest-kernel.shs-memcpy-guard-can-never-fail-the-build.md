---
id: TASK-79.65
title: build-guest-kernel.sh's memcpy guard can never fail the build
status: To Do
assignee: []
created_date: '2026-10-21 21:00'
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
- [ ] #1 A test reproduces the failure on the code before the fix
- [ ] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
