---
id: TASK-291.4
title: 'fsx and fxfs-posix model sequences: collector passes between operations'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 08:37'
updated_date: '2026-11-10 11:01'
labels: []
dependencies: []
parent_task_id: TASK-291
priority: low
ordinal: 695000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The model-checked sequences (fxfs-posix model_tests, guest fsx scripts) remount and compact Fxfs between operations but never collect the bucket under them, so regressions where collection loses data a filesystem still reads are caught only by the soak.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Model sequences can run a collector pass (grace 0, compact_below > 0) as one of their operations, and do by default in some seeds
- [x] #2 The sequences pass with it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Per the user: a new model in fxdax, not the fxfs-posix model (FakeDevice, no bucket) nor guest fsx.
1. storage/model_tests.rs (cfg(test)): proptest sequences of ops on an image with its own bucket, its collector quickened to grace 0, compact_below 0.5: Write (file 0..4, MiB-aligned offset, 1..6 MiB of noise, made if absent), Truncate, Unlink, Fsync, Clone (the image to a clone, mounted, later ops on either), Remount (close and mount again), Collect (pass_now, grace 0).
2. Each step reads back what it changed against the model; Remount and the end compare every file; the end closes, reopens, collects, and compares again.
3. FXDAX_COLLECT_MODEL_CASES for more cases; a hand mutant (doomed deletes named segments) must fail it.
Verify: cargo nextest run -p fxdax --lib storage::model_tests
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Verified: macOS storage:: 21/21 (the model 40 s at 4 cases), 16 cases 176 s; ubu1 21/21 at d6dfd518, 16 cases 94 s; clippy clean. Hand mutant (collector deleting named segments) fails it in 6.5 s (journal replay failed). Timing: remounts ~1.5 s and clones 1.5-8 s (Fxfs in a debug build, the file bucket's syncs), hence 4 cases by default.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
As the user chose: a new model in fxdax (storage/model_tests.rs), not fxfs-posix's (no bucket under its FakeDevice) nor guest fsx. Proptest sequences on an image with its own bucket and up to two clones: writes, truncations, unlinks, fsyncs, clones, remounts and Collect (a pass, grace 0) as an operation, with passes after every commit too; each step's change read back, everything compared at clones, remounts, passes and the end, then opened anew after a pass alone. 4 cases by default, FXDAX_COLLECT_MODEL_CASES for more; a hand mutant fails it. macOS and ubu1.
<!-- SECTION:FINAL_SUMMARY:END -->
