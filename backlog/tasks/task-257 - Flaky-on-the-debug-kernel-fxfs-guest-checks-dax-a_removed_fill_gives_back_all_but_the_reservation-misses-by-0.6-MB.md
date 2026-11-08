---
id: TASK-257
title: >-
  Flaky on the debug kernel: fxfs-guest-checks-dax
  a_removed_fill_gives_back_all_but_the_reservation misses by 0.6 MB
status: To Do
assignee: []
created_date: '2026-11-08 10:38'
updated_date: '2026-11-08 14:53'
labels:
  - bug
  - flaky
dependencies: []
priority: medium
ordinal: 690000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE validation run B (2026-11-08, main 08d444ef7, debug kernel: KCSAN, lockdep; fxdax-keep-n2-standard-4-us-west1-b-1), seed 1794336019930021975: 'a_removed_fill_gives_back_all_but_the_reservation: 268435456 bytes, 242262016 free before, 254799872 after, -12537856 short; reserved 13128828, then 3907564' then attributes.rs:201 '-12537856 short, the reservation grew -9221264': short - grew = -3.3 MB against a tolerance of size/100 (2.68 MB). The check came with TASK-149.25 (ffc85acac), green on the release kernel (r945-r947). Likely other metadata (compaction's layer files) freed during the fill on the slower debug kernel, which the check counts against the reservation; unconfirmed. Results: the run's e2e/fxfs-guest-checks-dax log.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause found: what besides the metadata reservation changes free space across the fill and removal
- [ ] #2 The check holds on the debug kernel for 20 rounds, measuring what it means to, its tolerance not loosened to hide a cause
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-08: failed on the release kernel too (GCE fxdax-test-r1001-t1492, n2-standard-16, fxfs-guest-checks-dax seed 1794222176066265452, at the tracepoints commit ad266aff6's parent tree plus it): 268435456 bytes, 240873472 free before, 253026304 after, -12152832 short; reserved 14139932, then 5202908 (the reservation grew -8937024). It passed in the same commit's first release run and its debug run.
<!-- SECTION:NOTES:END -->
