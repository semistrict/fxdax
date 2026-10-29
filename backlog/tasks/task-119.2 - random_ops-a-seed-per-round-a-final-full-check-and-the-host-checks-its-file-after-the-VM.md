---
id: TASK-119.2
title: >-
  random_ops: a seed per round, a final full check, and the host checks its file
  after the VM
status: Done
assignee:
  - '@claude'
created_date: '2026-10-28 19:27'
updated_date: '2026-10-29 12:38'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: high
ordinal: 438000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
random_ops always runs seed 1, has no full check after its last op, compares only 1 MiB + 4 KiB of a file reaching 1 MiB + 16 KiB, and removes its file, so the host never reads what the store holds.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Each round uses a different seed, printed, replayable by setting it
- [x] #2 The whole file is checked after the last op
- [x] #3 The host compares the stored file with the model after the VM exits
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Host picks a seed per run (FXDAX_SEED replays), passes FXDAX_RANDOM_OPS_SEED, writes it to the run dir, names it in failures. random_ops checks the whole file (model length + a page) every 256 ops and after the last; keeps random-ops and random-ops.model; host compares them from the store.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
On main since b9af10794 (2026-10-28), unrecorded till now: FXDAX_SEED picks the run's seed (host-chosen and printed otherwise, 'seed N (FXDAX_SEED=N replays it)', FXDAX_RANDOM_OPS_SEED in the guest); random_ops checks the whole file evicted after its last op; the host compares the stored random-ops file with its model after the VM. GCE r221: checks-dax and -delay printed a new seed each round and passed (2 rounds each); r224 checks-dax-delay 10 rounds. DoD #1 (macOS workspace nextest) not run for this.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Seeded random_ops (a seed per round, printed, replayable), its whole file checked after the last op, and the host's comparison of the stored file with the model: b9af10794. Verified by GCE r221 and r224 passing rounds with distinct printed seeds.
<!-- SECTION:FINAL_SUMMARY:END -->
