---
id: TASK-112.6
title: >-
  Guest DAX: evacuations run several seals late lose data under
  fxfs.dax_delay_us (append_writeback reads zeros, random_ops mismatches)
status: To Do
assignee: []
created_date: '2026-10-28 04:48'
updated_date: '2026-10-28 14:13'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Hypothesis (read-only analysis, 2026-10-28): fxfs_dax_seal sets region->evacuate as the seal begins, before the device seals the extent; an evacuation in between (a late one, or on main one overlapping the next seal) maps the still-open extent. Tested with a new fault injection fxfs.dax_evacuate_in_seal (queue the evacuation from fxfs_dax_seal, then fsleep 2 ms) on main 32aeffade+: checks-dax under fxfs.dax_delay_us=200 passed 3/3 (r202), so not reproduced: the evacuation may wait on the file semaphore its seal's caller holds, or the host's seal remaps the evacuation's read-only map of the open extent (seal_mapped keeps mappings of the open VMO), making it harmless. Candidate fix kept unlanded: region->sealed_at = fxfs_device_seals() at seal, gather skips a region while the seal count still equals it. Other hypotheses: a deferred seal_mapped change landing after the evacuation's newer MAP of the same own-space slot (check: memcmp own-space pages with region pages after the MAP); aborted map_write leaving moved stale; region pool full pushing writes to own-space writable maps (TASK-79.106).

Review 2026-10-28: fxfs_dax_seal sets region->evacuate and shrinks region->pages at seal begin, before the COMMIT completes; an evacuation already running can map the still-open extent read-only. Matches the sealed_at hypothesis. Suggested diagnostic: after the MAP in fxfs_dax_evacuee_place, memcmp own-space vs region page and WARN; record per page the seal count at move/gather/place.

2026-10-28 GCE r204: TASK-112.5's first cut (evacuation MAP without the file semaphore, placing waiting for the ranges' other users) failed random_ops in round 3 of 5 under fxfs.dax_delay_us=200 (seed 1, op 1799: read of 0x101000 at 0 differs at 0x14000), where main passed 5/5 the same day: the same signature as batching's. Both delay the evacuation, which then may gather a region whose seal has begun (evacuate set in fxfs_dax_seal) but whose COMMIT hasn't ended. Fix under test (with TASK-112.5): region->sealed_at, the device's seal count at the seal's begin; gather skips regions until the device's count passes it (the seal's end re-queues the file).

2026-10-28 later: the sealed_at fix does not fix it. HEAD 9b937d632 (local, unpushed: TASK-112.5 unlocked evacuation MAP + sealed_at + nr_moved) failed random_ops in round 2 of 10 (op 1028, differs at 0x6000) under fxfs.dax_delay_us=200 on r204; the evacuate_unsealed mutant (sealed_at check removed) failed an append check (zeros where 165s expected, checks.rs:2490) in round 2 on r205. So delaying evacuation relative to the seal (batching, or the unlocked MAP) exposes a race other than the seal's begin. Diagnostic added: fxfs.dax_evacuee_verify compares each placed page's own-space copy with its region page and warns; r204/r205 run it x6 each.

Verify run (fxfs.dax_evacuee_verify=1, delays): r204 6/6 green; r205 failed random_ops round 6 at op 117 (read at 0x16282 differs from 0x17000; a store at 0x17b84 after an fsync came just before) with no 'evacuated page differs' warning: each evacuated page matched its region page when placed, so the host maps the right data; the loss is in the guest's bookkeeping, or a write after placing. Parked on branch wip/task-112-5 (9b937d632 + 54aaf0633).
<!-- SECTION:NOTES:END -->
