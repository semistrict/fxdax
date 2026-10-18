---
id: TASK-44.2
title: >-
  TLA+ spec of the guest's DAX seal and relocation protocol, rediscovering the
  6.18 SIGBUS
status: Done
assignee:
  - '@claude'
created_date: '2026-10-18 04:06'
updated_date: '2026-10-18 04:29'
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
- [x] #1 A TLA+ spec of the seal gate, writable and relocated pages, faults (entry locks, invalidate_lock), write(2) and zeroing, with each protocol rule switchable, checked by make check-tla
- [x] #2 Invariants: a fault never maps a page where its DAX entry says it was, nor makes writable a mapping of another page (what insert_page_into_pte_locked and validate_page_before_insert refuse on 6.18); no store lands in a sealed extent; a page's mapping shows its latest data once its write(2) or fault is done
- [x] #3 A mutation with dce4f14's bug (any iomap consuming relocation marks) gives a TLC counterexample of the fault-mapping invariant, as does each other rule removed
- [x] #4 Any design flaw it finds in the current code is filed and fixed with a guest test that reproduces it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. specs/dax/DaxSeal.tla: one page; write(2), faults (with user stores) and the sealer as processes at the code's steps (sem, gate, move, seals check, cow, invalidation, insert); 6.18's insert refusals as SIGBUS. 2. Rules: faultsOnly, shared, wp, forgetWritable, sealsCheck, gateWait, tryGate; a mutation each. 3. Generalize scripts/check-tla.sh to specs/*/, with deadlock expectations.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
specs/dax/DaxSeal.tla models one page with a write(2) task, a faulting task storing through its mapping, and the sealer, at dax.c's and device.c's steps. DaxSeal (2 writes, 3 faults, 2 stores, 2 seals) holds: 186K states, 2s; DaxSealDeep (3, 4, 2, 3): 2.8M states, 8-11s; no deadlock. Mutations, each failing as expected: NoFaultsOnly (dce4f14: a write(2)'s iomap forgetting the mark) -> NoSigbus, its counterexample the seal_race interleaving (a read fault inserting where the page was after the write(2) moved it, a write fault then keeping that entry); NoShared -> Coherent; NoWp, NoForgetWritable, NoSealsCheck, NoGateWait -> NoSealedStores; NoTryGate (faults waiting for the gate holding their entry) -> deadlock. No flaw in the current code found. scripts/check-tla.sh now runs every spec under specs/ (models by NAME or SPEC/NAME), with 'expect: deadlock'; README Testing documents make check-tla.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
specs/dax/DaxSeal.tla model-checks the guest Fxfs's DAX seal and relocation protocol at its code's steps; it holds (186K and 2.8M states), and each of its seven rules removed fails as expected, dce4f14's bug rediscovered as the seal_race interleaving and the -EBUSY rule as a deadlock. make check-tla runs it with the follower spec.
<!-- SECTION:FINAL_SUMMARY:END -->
