---
id: TASK-149.2
title: 'Seeded, replayable, shrinking operation scripts for DAX in a guest'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-01 06:13'
updated_date: '2026-11-01 06:16'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: high
ordinal: 503000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Most bugs are in DAX, where random_ops runs but failures take hours to pin down: no operation script to replay and no shrinking, and few failures combined. This brings the model test's method to a real guest, on top of TASK-44.15's oracle: scripts of mmap, DAX writes, reads through mappings of the same file, fork, truncate, a nearly full disk and VM kills, generated on the host from a seed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A script is generated from a seed and a failure prints the seed
- [ ] #2 A failing script shrinks to a smaller one that still fails, in a VM
- [ ] #3 Scripts combine mappings, fork, a nearly full disk and a VM killed at a random point
- [ ] #4 Each file is checked against a model, after a kill as after a clean stop
- [ ] #5 Reintroducing a past DAX bug (TASK-140's livelock, or a moved-unmapped mutant) fails, shrunk
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Existing: test_random_ops has seeded choices, a byte model, a mapping kept across operations, offsets at page, chunk and file ends, and a history of its operations; test_random_ops_threads adds threads. Missing: operations decided outside the guest, shrinking, several files, reflinks, fork, a nearly full disk, a VM killed. 1. Split test_random_ops into a generator, a pure function of the seed giving a script of text lines, and an interpreter that runs a script in the guest, checks against the model and reports the failing line. Same behaviour, proven by replaying a run. 2. The host writes the script, passes it to the guest, and on a failure shrinks it by deleting chunks of lines and rerunning, a VM each try, keeping the smallest that fails the same way, with a bound on tries. 3. New operations: several files, FICLONE, fork with the child writing through the mapping, a small volume where ENOSPC is allowed and leaves the model unchanged. 4. A VM killed at a line, then remounted, each file checked against states since its last fsync, as the Rust model does. 5. Prove it: put back a moved-unmapped mutant, and TASK-140's livelock, and each must fail and shrink.
<!-- SECTION:PLAN:END -->
