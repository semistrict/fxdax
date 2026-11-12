---
id: TASK-386
title: >-
  fxfs-guest-bucket-soak fails 'VM 0 ended before the run did' when its phases
  run late: each guest's 1,800 s count from its own boot, the host's phases from
  each phase's start
status: Done
assignee:
  - '@claude'
created_date: '2026-11-12 15:15'
updated_date: '2026-11-12 16:57'
labels:
  - bug
  - test
  - harness
dependencies: []
priority: medium
ordinal: 764000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-382's soak on ubu1 (res-fix1, 2026-11-12): VM 0 wrote its 3,840 MiB and exited at its 1,800 s (guest log 'OK wrote 3840 MiB ... reboot' at 1801.7 s), while the host's last phase, begun at 1263.6 s instead of 1260 s, ran to 1803.6 s and failed the run on its running() check. Each phase's end was taken from the phase's own start (Instant::now() + fraction), so a phase that ran over (the out-of-service one took 182.8 s, sleeping 1 s ticks and reading stats) pushed every later one, while the guests started their clocks at their own boot, before the host's 'began'. Blind spot: the case never had a phase overrun; its check assumed the host outlasts every guest.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Phases end at fixed offsets from the run's start
- [x] #2 A VM that ends after its own run's seconds from its boot passes; one ending before fails
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fixed: phases end at began + the sum of their fractions; a VM's running() check passes once its own run's seconds from its boot are up. Verified by res-fix3 on ubu1 (1,800 s soak, every check passed; the out-of-service phase's bucket.set took ~6 s there and no later phase moved). The blind spot was the host's phases never running over before; the case's own end-to-end run is its test (guest cases build on Linux only).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxfs-guest-bucket-soak's phases end at fixed offsets from the run's start, and a VM may end once its own run's seconds from its boot are up; verified by a passing 1,800 s soak on ubu1 (TASK-382's res-fix3).
<!-- SECTION:FINAL_SUMMARY:END -->
