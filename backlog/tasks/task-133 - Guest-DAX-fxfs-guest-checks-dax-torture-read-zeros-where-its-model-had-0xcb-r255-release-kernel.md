---
id: TASK-133
title: >-
  Guest DAX: fxfs-guest-checks-dax torture read zeros where its model had 0xcb
  (r255, release kernel)
status: To Do
assignee: []
created_date: '2026-10-30 01:36'
updated_date: '2026-10-30 01:41'
labels:
  - dax
  - bug
dependencies: []
priority: high
ordinal: 480000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r255 (n2 nested, release guest kernel), fxfs-guest-checks-dax round 1 at faeec5186 (main 83c92d55f + vendor/firecracker 43e203825, which pre-faults MAP_AHEAD's read-only maps after the MAP's reply, TASK-131.3): 'seed 1794450854429239374, phase 6, after the threads: /mnt/fxdax/torture-1 at 0x1e06b (thread 1's) reads 0x0, its model 0xcb'. 101 refused stores in firecracker.log, none unexplained. Not reproduced since: with that change, torture alone 10 of 10 rounds and the full suite 6 of 6 more; without it (vendor/firecracker 38cb48619), the full suite 8 of 8. Whether the change makes it likelier, or it was there before, is open (TASK-131.3 is measuring more rounds). Logs: /tmp/fxdax-r255-fork-suites/e2e/fxfs-guest-checks-dax/round-1.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause is found, or the failure shown independent of the deferred pre-fault by rounds with and without it
- [ ] #2 FXDAX_SEED=1794450854429239374 FXDAX_CHECKS=torture and the full suite pass 20 rounds on the fix
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Second failure with the deferred pre-fault (vendor/firecracker 43e203825), r255 at faeec5186, full suite round 1 of 12: append_while_mapped 'byte 0x74000 of 0x90000 mapped' read 0, its append wrote 2. So with the change: 2 zero reads in 9 full rounds (plus 10 of 10 torture-only); without it, 8 of 8 full rounds clean. The change is implicated: a KVM_PRE_FAULT_MEMORY of a MAP_AHEAD range run after the MAP's reply races the guest's next window changes there (UNMAP, a writable MAP of the same space); the page tables it reads may be mid-change, or a page not present faults through the window's userfaultfd from the replies thread. The same pre-fault before the reply can race other requests' changes too, only in a narrower window: whether the race exists before the change at a lower rate is the open question. 43e203825 is not pushed.
<!-- SECTION:NOTES:END -->
