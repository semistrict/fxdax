---
id: TASK-211
title: >-
  Fxfs: a full TIGHT device remounts no more: replay's object manager can't
  reserve its metadata (Insufficient free space for metadata reservation)
status: To Do
assignee: []
created_date: '2026-11-06 11:43'
labels:
  - bug
  - fxfs
dependencies: []
priority: high
type: bug
ordinal: 650000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the 1,000-case model hunt of TASK-210 (2026-11-06, macOS, on 36cd32147 with and without TASK-210's fix, so not that change). A sequence on a TIGHT (2048-block) device ends with a clean remount that fails: "at the end: opening: Journal replay failed: Failed to complete replay for object manager: Insufficient free space for metadata reservation.: Failed to reserve 1607432 - 692548 = 914884 bytes, free=827392". A filesystem that unmounted cleanly then mounts no more. Reproduce, deterministic, 0.45 s, in crates/fxfs-posix/src/model_tests.rs: Setting::awkward(0, TIGHT).with_graveyard_of_seed().holds(vec![Op::Create(2), Op::Create(0), Op::Fill { file: 5, page: 0, pages: 1329, stride: 2, byte: 0 }, Op::Fill { file: 0, page: 0, pages: 1885, stride: 2, byte: 0 }, Op::Fill { file: 0, page: 0, pages: 45, stride: 1, byte: 0 }, Op::Punch { file: 0, page: 0, pages: 1 }]). Main's commits since 1fefc017c touch only xattrs in fxfs and fxfs-posix, which the sequence doesn't use, so it is likely older and rare. Related: TASK-26.6.1, TASK-58, TASK-60 (reservation exhaustion).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The sequence pinned as a model test, failing before the fix
- [ ] #2 Its cause named: why replay needs more metadata reservation than a full device leaves after a clean unmount
- [ ] #3 A full device remounts: the pinned test passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
