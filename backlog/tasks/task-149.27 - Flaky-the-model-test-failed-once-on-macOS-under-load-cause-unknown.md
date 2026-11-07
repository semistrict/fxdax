---
id: TASK-149.27
title: 'Flaky: the model test failed once on macOS under load, cause unknown'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-03 20:06'
updated_date: '2026-11-07 05:39'
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
- [x] #1 The failure is reproduced, with its sequence, or the test seeds deterministically per run and prints what replays a failure
- [x] #2 A cause found is fixed with a pinned regression, or ruled out with the evidence
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Hunt under CPU starvation (2026-11-07, main 5613fbb50)
- macOS: 16 runs of whatever_a_vfs_asks_reads_back_as_a_model_says at once on 15 cores, FXDAX_MODEL_CASES=400 each: 6,400 sequences, 19,831 power cuts, 27,574 cuts of replays, each run 255-330 s; all passed.
- Linux (Docker arm64): 6 runs at once pinned to one CPU (taskset -c 0), FXDAX_MODEL_CASES=300 each: 1,800 sequences, each run 739-788 s (about six times slower than unloaded); all passed.

## Why it is closed rather than reproduced
- Nothing in a sequence depends on the clock but the watchdog, which aborts and doesn't assert, and two timeouts no load reaches in an 80 s run: debug_assert_not_too_long's 60 s, and the graveyard's batches 3,600 s apart. The executor is single-threaded, and a sequence is seeded (Setting::awkward(seed, ..)): a failure replays from what its panic prints.
- AC 1: proptest's panic prints the shrunk input, (seed, blocks, ops), which Setting::awkward(seed, blocks).with_graveyard_of_seed().holds(ops) replays. TASK-182, 210, 211 and 218 pinned their failures that way; that is how a failure keeps what replays it. The output of 2026-11-03 was lost because the run's output was filtered.
- AC 2: since 2026-11-03 these hunts have found and fixed real bugs that fail exactly this test: TASK-182 (journal end refused after a cut, fsck hunt), TASK-210 (a cut during replay), TASK-211 (a full device that won't remount) and TASK-218 (borrowed metadata across mounts), all rare and seed-dependent. The 2026-11-03 failure was most likely one of them, at 76fdc073a's tree; under load it would only have taken longer. Starvation doesn't reproduce anything on main.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Not reproduced under CPU starvation: 6,400 sequences in 16 concurrent runs on macOS and 1,800 in 6 runs pinned to one CPU on Linux, all passing. Nothing in a sequence depends on the clock but timeouts no load reaches, and a failure's panic prints the seed and operations that replay it, as TASK-182, 210, 211 and 218 used. The lost failure of 2026-11-03 was most likely one of those bugs, since fixed.
<!-- SECTION:FINAL_SUMMARY:END -->
