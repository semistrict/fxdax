---
id: TASK-323
title: >-
  Guest check append_writeback catches no per-fsync slowdown: a 60 ms sleep per
  fsync survives (its concurrent syncfs keeps the device busy, so the
  host-time-excluded clock sees no guest time)
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-10 11:26'
updated_date: '2026-11-10 13:28'
labels:
  - testing
dependencies: []
priority: medium
type: bug
ordinal: 726000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-263 (done, main CI run 42804968972). A check's clock now excludes the time the virtio_extent device had any request in flight (busy_ns), counting the host's time at only 1/18. In append_writeback, a concurrent syncfs loop keeps the device busy for the whole check, so nearly all of the check's time is excluded. A mutant adding a 60 ms sleep per fsync passes. No exact request count holds either, because the counts vary with the syncfs race.

What would catch it: something that bounds the guest's fsync cost directly, for example per-fsync guest-side latency excluding that fsync's own device time, or an fsync count against a fixed amount of work. It must stay independent of host disk speed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A mutant adding a per-fsync delay in the guest's fsync path (60 ms or less) fails append_writeback
- [ ] #2 The check still passes 20 rounds on ubu1 on ZFS /home and on the ext4 scratch disk
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. append_writeback gets a quiet phase after its syncfs race: rounds of the same appends and fdatasync with nothing else asking the device, each fdatasync's guest time measured as its time but for the device's busy_ns meanwhile (its own requests, any other overlapping once).
2. Bound the guest's per-fsync cost from that (statistic and bound chosen from measured rounds on ubu1, ZFS and ext4, -dax, -dax-small, -dax-delay), independent of host disk speed as busy_ns leaves the host's time out.
3. Mutant guest/mutants/task-323-fsync-sleeps.patch: msleep(60) in fxfs_vfs_fsync; Kernel: release, Killer: fxfs-guest-checks-dax append_writeback. Build its kernel on ubu1, show it fails the check, and a smaller delay too.
4. 20 rounds of the check on ubu1, rounds on ZFS /home and on /mnt/fxdax-scratch (ext4).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-10:
- append_writeback keeps its 64 rounds racing syncfs, then runs 32 more alone, nothing else asking the device; each fdatasync's guest time is its time but for the growth of the device's busy_ns meanwhile (its own requests'). The median must be at most 5 ms (FDATASYNC, scaled as the budgets are, FXDAX_CHECKS_SCALE). The median, not the sum: in most rounds one or two took about 2 ms, the rest about 0.1 ms.
- Measured on ubu1 (release kernel built from main's guest sources, ~/src/fxdax-t263-kernel): 15 rounds (-dax, -dax-small, -dax-delay, 5 each, ZFS) median 71-103 us, longest 2.4 ms, all 32 at most 5.8 ms; host time 1.5-8.5 s.
- Mutant guest/mutants/task-323-fsync-sleeps.patch, msleep(60) at the start of fxfs_vfs_fsync, its own kernel: FAIL, 'an fdatasync's guest time, the median, 62.92ms of 5.00ms'; budget's guest time 2.06 s of 3 s, which alone would have passed. A hand variant of 6 ms (not committed): FAIL, median 11.05 ms (msleep's jiffies), 0.41 s of 3 s.
- 20 rounds of fxfs-guest-checks-dax, FXDAX_CHECKS=append_writeback, fxjob --bench, final artifacts: rounds on ZFS /home 20/20 (median 60-86 us, longest 1.78 ms, host up to 14.6 s); on ext4 /mnt/fxdax-scratch 20/20 (median 59-96 us, longest 2.34 ms, host up to 1.29 s).
- The new phase adds 32 durable fdatasyncs' host time (up to ~7 s on ZFS), well inside HOST_SCALE's 18x a 3 s budget.
<!-- SECTION:NOTES:END -->
