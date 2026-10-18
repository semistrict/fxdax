---
id: TASK-44.2
title: >-
  TLA+ spec of the guest's DAX seal and relocation protocol, rediscovering the
  6.18 SIGBUS
status: To Do
assignee: []
created_date: '2026-10-18 04:06'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: high
ordinal: 131000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The guest's DAX code (guest/linux/fs/fxfs/dax.c, the seal gate in device.c: about 2,600 lines of C) is the system's most intricate concurrency: seals waiting for stores and keeping new ones out, the sealer making user mappings read-only and forgetting writable pages, writes moving pages to new blocks (relocation marks), faults holding DAX entry locks and the invalidate_lock, write(2) and zeroing replacing entries only after copying. Its bug dce4f14 (6.18 SIGBUS: a write(2) consumed a page's relocation mark before replacing its mapping) hid behind a silent WARN_ON_ONCE on 6.12 and was found only by the seal_race stress test. A spec as TASK-42.8's (specs/follower), checked by make check-tla, models it; the follower's invalidation (TASK-42.6) will extend the same code.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A TLA+ spec of the seal gate, writable and relocated pages, faults (entry locks, invalidate_lock), write(2) and zeroing, with each protocol rule switchable, checked by make check-tla
- [ ] #2 Invariants: a fault never maps a page where its DAX entry says it was, nor makes writable a mapping of another page (what insert_page_into_pte_locked and validate_page_before_insert refuse on 6.18); no store lands in a sealed extent; a page's mapping shows its latest data once its write(2) or fault is done
- [ ] #3 A mutation with dce4f14's bug (any iomap consuming relocation marks) gives a TLC counterexample of the fault-mapping invariant, as does each other rule removed
- [ ] #4 Any design flaw it finds in the current code is filed and fixed with a guest test that reproduces it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
