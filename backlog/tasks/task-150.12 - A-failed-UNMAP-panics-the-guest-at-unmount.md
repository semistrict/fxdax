---
id: TASK-150.12
title: A failed UNMAP panics the guest at unmount
status: To Do
assignee: []
created_date: '2026-11-01 16:52'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: medium
type: bug
ordinal: 527000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed, and traced against linux-7.1.2, not the 6.18 tree the guest builds on. dax.c:1727 leaves the bits allocated on purpose; :4661-4667 calls gen_pool_destroy, whose BUG_ON(bit < end_bit) fires once window_lost_bytes is above 0.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test fails on the code as it stands, or shows the finding false or unreachable, which closes this as not a bug
- [ ] #2 Fixed, the test red again with the fix taken out
- [ ] #3 The notes name the blind spot, the test of its class, and what a search around it found
- [ ] #4 Passes on GCE in the guest suites, DAX and not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
