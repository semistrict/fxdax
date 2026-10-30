---
id: TASK-134
title: >-
  Flaky?: fxdax
  extentd::tests::frozen_vm_processes_are_killed_without_peer_pidfds failed once
  (FAIL + LEAK at 33 s) in a full Docker run
status: To Do
assignee: []
created_date: '2026-10-30 01:37'
updated_date: '2026-10-30 09:29'
labels:
  - bug
  - flaky
dependencies: []
priority: low
ordinal: 481000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
OrbStack Docker, the TASK-111.7 worktree (e04c0daa9's parent tree with the build stamp), one full fxdax run: the test failed and leaked at 33 s. It passed in two more full runs and 10 of 10 alone. Unknown whether it predates the change.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Reproduced, or shown to pass 50 runs under the full suite's load on Linux; the cause fixed if reproduced
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
r273 (Linux workspace, batches 6-7 on 76ec99cf9): frozen_vm_processes_are_killed_without_peer_pidfds failed at 0.088 s: 'no chunk of ArenaId(1) holds 0x0' at page_is_free, the page a frozen VM process maps not even in a chunk, so freed while mapped, or the arena's chunk let go. r270 (76ec99cf9 alone): frozen_vm_processes_hold_nothing_up timed out, 'the frozen VM process lives on'. Both frozen_vm tests 40 of 40 under Docker Linux, alone (nextest --stress-count 40): only under the full suite's load, so far. The first could be memory a VM process still maps being freed: not a test's flake until shown otherwise.
<!-- SECTION:NOTES:END -->
