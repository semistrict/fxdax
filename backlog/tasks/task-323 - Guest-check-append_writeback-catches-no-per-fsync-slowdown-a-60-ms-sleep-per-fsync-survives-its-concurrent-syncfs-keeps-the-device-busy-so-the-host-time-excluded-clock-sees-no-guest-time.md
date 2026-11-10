---
id: TASK-323
title: >-
  Guest check append_writeback catches no per-fsync slowdown: a 60 ms sleep per
  fsync survives (its concurrent syncfs keeps the device busy, so the
  host-time-excluded clock sees no guest time)
status: To Do
assignee: []
created_date: '2026-11-10 11:26'
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
