---
id: TASK-149.27
title: 'Flaky: the model test failed once on macOS under load, cause unknown'
status: To Do
assignee: []
created_date: '2026-11-03 20:06'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 605000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxfs-posix model_tests::whatever_a_vfs_asks_reads_back_as_a_model_says failed once (80.8 s, not its 120 s watchdog, so an assertion) in a cargo nextest run of fxfs and fxfs-posix at 76fdc073a's tree (TASK-153.12's branch, rebased on 81e8869c5), on a Mac other sessions loaded. Its output, which says the minimal sequence, was lost: the run's output was filtered. Not reproduced since: two full runs, 8 stress runs of 96 cases, 6 of 800 (about 5,600 sequences). Proptest seeds it from entropy (TestRunner::new), so no seed replays it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The failure is reproduced, with its sequence, or the test seeds deterministically per run and prints what replays a failure
- [ ] #2 A cause found is fixed with a pinned regression, or ruled out with the evidence
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
