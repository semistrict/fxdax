---
id: TASK-149.31
title: A guest check that stores to a page of a slice a pin holds
status: To Do
assignee: []
created_date: '2026-11-05 15:58'
labels:
  - testing
dependencies: []
parent_task_id: TASK-149
ordinal: 640000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-160.32: task-79.49-store-fails-on-pins (guest/mutants), a store from user mode failing with SIGBUS on a page of a slice pinned rather than waiting for the pin to go (TASK-79.49), was killed by fxfs-guest-checks' slice_held (GCE r307), which went with writable DAX (37c331a6c). No check stores to a pinned slice's page since: fxfs-guest-checks-dax passes with the mutant in, 3 rounds (GCE fxdax-test-r625, 2026-11-05). guest/sim has no pins (no get_user_pages), so it needs a real kernel.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A check of fxfs-guest-checks-dax (or -small) pins a slice's page (vmsplice) and stores to it from user mode, the store waiting for the pin to go
- [ ] #2 task-79.49-store-fails-on-pins fails that check, and its header names it as its killer (Kernel: release)
- [ ] #3 The check passes without the mutant
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
