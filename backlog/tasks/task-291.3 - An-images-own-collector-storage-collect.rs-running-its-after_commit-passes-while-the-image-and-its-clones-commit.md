---
id: TASK-291.3
title: >-
  An image's own collector (storage/collect.rs) running its after_commit passes
  while the image and its clones commit
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 08:37'
updated_date: '2026-11-10 10:43'
labels: []
dependencies: []
parent_task_id: TASK-291
priority: medium
ordinal: 694000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
storage/collect.rs starts a pass on a thread after an image's commit, one per process per bucket, shared by an image and its clones, with OPTIONS' 60 s grace, and replaces a panicked pass's collector. Tests only call pass_now, which joins any running pass and then collects alone with grace 0, so the background pass never meets concurrent commits, clones or drops.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test commits through an image and a clone from several threads while passes start on their own (EVERY made small for the test), and checks every file against a model after each round
- [x] #2 Dropping an image or clone while its pass runs leaves the bucket readable and the next pass completes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. collect.rs: a Pace (every, options) per Collecting, PACE by default, quickened per thread in tests; passes return their Report, summed in State; a collect::pass failpoint.
2. kept.rs: an image and its clone, mounted, churn files of 1-6 MiB noise (make, write over and past their ends, unlink, fsync each) from a thread each while passes start after every commit (every 0, grace 0, compact_below 0.5); each round's files checked against a model; after, passes > 4 and compacted and deleted > 0; reopened after a pass alone, both hold their models.
3. Dropping: a pass held up by the failpoint as the clone's commit starts it, both closed and dropped meanwhile; reopened, both hold their models; the next pass completes and deletes, the clone still whole.
Verify: cargo nextest run -p fxdax --lib storage::; a hand mutant (doomed deletes named segments) fails step 2.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Verified: macOS fxdax storage:: 20/20, clippy clean; ubu1 20/20 at 1e003de7, the two new tests 5/5 more. Hand mutant (collector doomed() deleting named segments) fails passes_run_among_an_image_and_its_clones_commits (journal replay failed on reopening); the drop test, about drops, passes under it.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
storage/collect.rs gets a Pace (how often passes run, how they collect) that tests quicken per thread, and passes report what they did. Two tests in storage/kept.rs: an image and its clone churn files from a thread each while passes start after every commit, compacting and deleting at once, files checked every round and after reopening (catches a hand mutant deleting named segments); and both dropped while a held-up pass runs, then reopened whole and collected again. macOS and ubu1.
<!-- SECTION:FINAL_SUMMARY:END -->
