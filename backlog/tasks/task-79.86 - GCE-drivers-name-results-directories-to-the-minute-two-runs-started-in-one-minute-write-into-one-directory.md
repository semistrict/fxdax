---
id: TASK-79.86
title: >-
  GCE drivers name results directories to the minute: two runs started in one
  minute write into one directory
status: Done
assignee: []
created_date: '2026-10-23 09:16'
updated_date: '2026-10-24 16:13'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 345000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found reviewing TASK-79.62 (round 2c), pre-existing. bench-gce.sh, example-gce.sh, gets-gce.sh and mutants-gce.sh named their default results directory to the minute (gce-*-%Y-%m-%dT%H%MZ) while default instance names go to the second, so two drivers started in the same minute ran on separate hosts but wrote into one results directory (source.txt, instance.json, logs). Fixed on r2c/scripts (default results and host names end in a per-run mark, also on test-gce's and gets' bucket prefixes).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Two drivers started in the same second write into separate results directories and bucket prefixes
- [x] #2 scripts/tests pins it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Finalization check (after the push of 47b7a348): not done. AC#1 and AC#2: the bucket prefixes are not proven. Missing is a scripts/tests test that runs test-gce.sh `run` (with FXDAX_EXTENTD_REMOTE) and gets-gce.sh `run` twice within one second against fake_gcloud, and asserts that the two runs' bucket prefixes are distinct and end in their marks. The results-directory half is proven and pinned by test_runs_at_once_have_names_of_their_own. Audit: Agree that it is not done; the proposal claims no AC. cbe33d60 appends -$gce_maker to the bucket prefixes in test-gce.sh (FXDAX_EXTENTD_REMOTE/$run-$gce_maker) and gets-gce.sh ($bucket/$run-$gce_maker). No test in scripts/tests checks either: rg finds FXDAX_EXTENTD_REMOTE only in the host_command tests (test_gce.py:121-194), which pass prefixes in by hand, and no test runs test-gce.sh's or gets-gce.sh's `run`. Names.test_runs_at_once_have_names_of_their_own covers only host names and results directories, for bench, example, gets and mutants. So AC#1's bucket-prefix half and AC#2 lack a scripts/tests test that runs test-gce.sh `run` (with FXDAX_EXTENTD_REMOTE set) and gets-gce.sh `run` twice against fake_gcloud and asserts distinct prefixes ending in each run's mark. The DoD items hold: workspace logs, --check exit 0, and the change touches only scripts, no ported code.

Handed over (2026-10-24): not finished; nothing pending on a branch.

test_gce.RunNames.test_two_drivers_started_at_once_name_their_runs_apart passes (macOS, 2026-10-24). Every driver used run-<second>-<mark> on GCE today: r65, r66, r67 and the bench and example runs, with separate results directories. Linux: GCE r65 (2026-10-24) workspace 2621/2621; macOS: scripts/test.sh green, scripts/tests 84 OK; scripts/upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
GCE drivers name a run for its second and their mark (gce_run_name, scripts/lib/gce.sh), on the host and in the bucket, so two drivers started together write apart; pinned by scripts/tests RunNames (ae54e01de).
<!-- SECTION:FINAL_SUMMARY:END -->
