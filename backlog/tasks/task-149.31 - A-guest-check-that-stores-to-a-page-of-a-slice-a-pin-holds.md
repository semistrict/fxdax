---
id: TASK-149.31
title: A guest check that stores to a page of a slice a pin holds
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 15:58'
updated_date: '2026-11-05 22:32'
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
- [x] #4 guest/sim pins a page as get_user_pages does (sim_pin_user_page), and a scenario stores to a pinned slice's page from user mode, the store waiting for the pin to go (cow_store_to_pinned_slice)
- [x] #5 task-79.49-store-fails-on-pins fails that scenario, and its header names it as its killer (Kernel: sim)
- [x] #6 The scenario passes without the mutant
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Check the mutant's path still runs copying on write: a store to a slice's chunk (-EXDEV) drops the slice (fxfs_dax_fault_drop_slice), which waits for pins only for a user fault.
2. guest/sim instead of a guest check (GCE unavailable): model get_user_pages' pin as a reference to the page (kernel/mm.c, sim_pin_user_page / sim_unpin_page), which dax_layout_busy_page_range already sees.
3. Scenario cow_store_to_pinned_slice, slice_held's steps: a slice's page pinned, a store waits for the pin, lands once it goes.
4. Red with task-79.49-store-fails-on-pins, green without; header Kernel: sim, Killer: scenarios cow_store_to_pinned_slice.
5. mutants.sh --check, zloop.sh -s 3000, the full sim mutant run, guest-mutants-gce.py --check, scripts tests.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Verdict: a real gap, not an equivalent. Copying on write, a store to a slice's chunk still finds the slice can't take it (-EXDEV). fxfs_dax_fault_drop_slice drops it, waiting for pinned pages only when the fault is from user mode (fxfs_dax_drop_pinned's 'pinned'). The mutant has every fault fail there instead (-EBUSY, then SIGBUS).

The killer is in guest/sim, not a guest check, so criteria 1 to 3 are left unchecked. GCE is unavailable (gcloud sign-in), and a guest check takes minutes a round. guest/sim now pins a page as get_user_pages does: kernel/mm.c's sim_pin_user_page takes the page's reference, faulting it in as the kernel would (no FAULT_FLAG_USER; handle_fault takes 'user' now), and sim_unpin_page drops it. fs/dax.c's dax_layout_busy_page_range and dax_break_layout already see such a reference, and folio_put wakes the waiter. The sim still can't show GUP's own races (its fast walk, or a pin taken as a mapping goes); README says so.

Scenario cow_store_to_pinned_slice runs slice_held's steps (commit 79c20b0bf). An 8-page file is written back and sealed, and its ranges dropped. A load maps the chunk as a slice, page 0 is pinned, and a storer's store to page 0 must not land within 50 ms (which also proves page 0 was a slice's). After the unpin the store lands, and read(2) and Fxfs hold it.
- Red, with the mutant applied: 'fxfs: a write fault of order 0 at page 0 of ino 100 failed: -18', then 'sim: PANIC in storer: a store to page 0' (exit 2). In the full mutant run: 'killed task-79.49-store-fails-on-pins scenarios cow_store_to_pinned_slice 0 s'.
- Green, without it: 'cow_store_to_pinned_slice: passed'.
The mutant's header says Kernel: sim, Killer: scenarios cow_store_to_pinned_slice. Decision recorded in OPEN_QUESTIONS.md # Decisions.

Verified: guest/sim mutants.sh --check (34 checked, 0 broken); zloop.sh -s 3000 passed, every one-run test and scenario, this one included, and 3,000 seeds each of interleave and failures; the full sim mutant run killed every Kernel: sim mutant that has a killer; scripts/guest-mutants-gce.py --check (38, 0 broken); FXDAX_TEST_PARTS=scripts scripts/test.sh passed; scripts/upstream-changes.py --check passed. DoD 1 not run: no Rust changed, and GCE is unavailable.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
task-79.49-store-fails-on-pins was a real gap: a store to a slice's chunk still drops the slice, and a user fault must wait for pins there. guest/sim now pins a page as get_user_pages does (sim_pin_user_page). The new scenario cow_store_to_pinned_slice, slice_held's steps, kills the mutant ('a store to page 0', SIGBUS) and passes without it. The mutant is now Kernel: sim, Killer: scenarios cow_store_to_pinned_slice. Verified with zloop -s 3000, the full sim mutant run, mutants.sh --check, guest-mutants-gce.py --check and the scripts tests.
<!-- SECTION:FINAL_SUMMARY:END -->
