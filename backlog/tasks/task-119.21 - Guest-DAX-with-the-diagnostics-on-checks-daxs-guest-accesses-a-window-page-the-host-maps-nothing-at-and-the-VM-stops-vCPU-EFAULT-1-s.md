---
id: TASK-119.21
title: >-
  Guest DAX: with the diagnostics on, checks-dax's guest accesses a window page
  the host maps nothing at, and the VM stops (vCPU EFAULT 1 s)
status: To Do
assignee: []
created_date: '2026-10-29 13:35'
labels:
  - dax
  - bug
dependencies: []
parent_task_id: TASK-119
priority: high
ordinal: 467000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r228c (main 7b146e0b5, kernel built), fxfs-guest-checks-dax with FXDAX_GUEST_CMDLINE='fxfs.dax_check_moves=1 fxfs.dax_shadow=1 fxfs.dax_check_seals=1 fxfs.dax_verify_seals=1': after direct_io passed, the VMM logged 'the DAX window refused an access to a page the window maps nothing at, at offset 0x1002a000 (made 34)', the page withheld, and a second later 'Failure during vcpu run: Bad address': the VM stopped. The guest took a page as mapped that the host had unmapped: guest and window disagree. Before it, the store to window offset 0x10341000 was refused 8 times in 3 s (made 21 to 61), and 12 times at the same offset in the mutant run r228m with check_moves and shadow on: a page refused again and again (TASK-44.12's signal). check_moves logs every kept range from Rust (fxfs_set_check_kept, ~3 ms a line on the serial console, 4850 lines), which may only widen a race present without them. Artifacts: /tmp/fxdax-r228c/e2e/fxfs-guest-checks-dax.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Which diagnostic's run reproduces it is known, and the access's window offset traced (fxfs.dax_trace) to the guest's mapping of it
- [ ] #2 Fixed, or shown a diagnostic's own fault; checks-dax with every diagnostic on passes on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
