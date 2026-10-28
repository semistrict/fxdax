---
id: TASK-112.6
title: >-
  Guest DAX: evacuations run several seals late lose data under
  fxfs.dax_delay_us (append_writeback reads zeros, random_ops mismatches)
status: To Do
assignee: []
created_date: '2026-10-28 04:48'
labels:
  - bug
dependencies: []
parent_task_id: TASK-112
priority: high
ordinal: 429000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
With evacuation delayed to every 16 seals or 20 ms (a TASK-112.4 experiment, not landed), fxfs-guest-checks-dax under fxfs.dax_delay_us=200 failed 2 of 8 rounds: append_writeback read 120 bytes of zeros after set_len(0), 132 one-byte appends, set_len(120) and sync_data, with a syncfs loop alongside (round 31); random_ops seed 1 op 781 read 0x384a bytes at 0x11f4c differing from the model at 0x13000 after a punch at 0xfc874 and a store at 0xddaeb. Main at 0d4a1a8b1 passed 5 of 5 rounds the same way. Evacuation after each seal may only be making a latent race rare: something between a seal and its evacuation (more seals, truncates, punches, regions sealed again) is not handled. Evidence: GCE r202 run-20261028T040329Z-b6ec46f4 round 3, r201 run-20261028T041213Z-8a31b666 round 4 (results copied to /tmp/fxdax-r202-112bd and /tmp/fxdax-r201-112bd5 on the laptop).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The race is identified and recorded
- [ ] #2 A check reproduces it on main with a delay injection (fxfs.dax_*), and passes with the fix
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
