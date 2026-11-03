---
id: TASK-44.19
title: >-
  Hand mutants of the guest's DAX locks and rechecks: which removed lock no test
  notices
status: Done
assignee: []
created_date: '2026-10-24 21:31'
updated_date: '2026-11-03 07:10'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: medium
ordinal: 398000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Before TASK-99's fix, taking region->lock out of fxfs_dax_region_open would have failed no test; cargo-mutants (TASK-44.1) covers Rust only. Removing each lock, gate check and seal recheck in dax.c and device.c, one at a time, and running the DAX suites, shows the code no test guards.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Each lock acquisition, gate check and seals recheck in dax.c and device.c is removed in turn and the DAX suites run on it
- [x] #2 Every surviving mutant gets a test that kills it, or a reason it is equivalent
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
map_to_unlocked (2026-11-03): put it in guest/sim's mutants (task-44.19-map-to-unlocked.patch), whose seeded interleavings reach two map_to's of one region at once, and run mutants.sh's 3,000 seeds on GCE against a baseline of the same seeds. Found: the patch stays as a past bug the sim finds. Not found: a scenario in guest/sim/tests that plays two slices of one staging extent reaching at once, and a seal after.

map_to_unlocked, done: 3,000 seeds missed it, so tests/scenarios' reaches_at_once plays it out, through a MAP the sim's device holds at a test's word (sim_device_hold_map) and a seal begun (fxfs_sim_sealing) before the held MAP ends. zloop.sh (CI) runs tests/scenarios' scenarios too, which only check.sh and mutants.sh ran.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
GCE r83 (kernel built per mutant from HEAD): killed region_open_unlocked, map_chunk_no_seals_recheck, new_slice_no_seals_recheck, write_no_seals_recheck, seal_keeps_writable, relocate_unmarked, cow_not_shared, held_not_copied_back; MISSED prune_in_use (fxfs_dax_prune without its users == 1 check: a range lets go of regions while another iomap uses it), which needs a test that kills it (AC2).

GCE r88: prune_while_read green x3 on HEAD (pushed, da13b9094), but MISSED the prune_in_use mutant: AC2 still open.

GCE r92: prune_while_read with fxfs.dax_delay_us=300 (and the new delay point between a read's iomap and its read) still MISSED prune_in_use, 6 rounds; green x6 on HEAD. The mutant may be equivalent in practice: a prune that ignores the range's users needs a reader's iomap to point into a region the prune drops, i.e. a page moved to a staging region, evacuated back, and the region's last reference dropped while the read is between iomap_begin and its copy. Next: a check that forces that sequence (write a page, fsync to seal, wait for the evacuation, read with the delay) or record the mutant as unkilled with that reasoning.

dax-gate on main at 10e0d6749 (2026-10-31): suites, pgbench and probes passed; 22 of 24 mutants killed, 2 MISSED within the gate's rounds: inherited_acls_written_back (TASK-79.29: inherited ACLs written with write_inode, an fsync of another inode durable without them) and fault_ignores_fatal_signals (TASK-79.49: a fault with no room ignores its signals). The tests for those behaviours exist but didn't fail in the rounds run: find out whether they need more rounds or never reach the path. Results: /tmp/fxdax-gate-10e0d67490c3 on the machine that ran it.

2026-11-02, main at d7ab673c5, GCE us-east4-a, release kernel: two mutants stale since refactors (their edits found 0 times, which stops a run of every mutant) retargeted. region_open_unlocked had no target: 7caa43578 took region_open's look at mapped and the region's lock around it out (TASK-99); it is now map_to_unlocked, fxfs_dax_map_to without region->lock, the lock's one use. MISSED by fxfs-guest-dax, -checks-dax, -checks-dax-delay and -pgbench-dax, 1 round (AC2). Two MAPs of one region concurrently are idempotent, but mapped may go back to the lesser 'to', and a sealed region shrinks to what it mapped (-EXDEV): a slice past it may lose pages. A test needs two map_to's of one region at once, two slices of a staging extent written at once, with fxfs.dax_delay_us. map_failure_unretried retargeted to fxfs_dax_fall_behind (07ab65415, TASK-150.13): killed, an injected MAP failure's EIO in fxfs-guest-checks-dax and -delay. New, moved_ptes_kept (TASK-149.14's unmap of a moved write's PTEs taken out): killed, 'a load through a PTE during the move' in -checks-dax and -delay, 2 rounds. Results: /tmp/fxdax-t18-mut-retarget, /tmp/fxdax-t18-mut-main.

2026-11-03, map_to_unlocked killed. Found how: GCE fxdax-test-t18-sim, mutants.sh -s 3000 on the mutant and a baseline: neither failed in 3,000 seeds (interleave and failures; last traces mutant e44707289a46357b, baseline 84df4097954ee74 at seed 3000). Blind spot: the sim's device answered a MAP at once, and no workload had two slices reach one region past its doubling step at once with a seal before the grow worker maps on; the guest suites, the same, by timing. Test: guest/sim's device holds a MAP when a test asks (sim_device_hold_map), and tests/scenarios' reaches_at_once holds the nearer write's MAP (16 pages, the first step), lets a further write (40 pages) reach, begins a seal (fxfs_sim_sealing), releases. With the mutant the further file's page 15 reads the nearer file's bytes: the region shrank to 16 pages, the next staging extent's region took window page 0x2010 on, and the nearer write, sealed before its copy, moved its page there. Red with the mutant, green without (GCE fxdax-test-t4419-reach, mutants.sh -s 0 task-44.19-map-to-unlocked baseline). Looked around: zloop.sh, CI's sim job, never ran tests/scenarios' scenarios (only check.sh and mutants.sh did, and mutants.sh's loop over them was nested in the loop over the one-run tests): lib.sh's sim_one_runs lists both for both now. zloop.sh -s 3000: every one-run test and scenario, 3,000 seeds, 0 failed (286 s). mutants.sh -s 0 over every mutant: each scenario-found mutant still found where the README says. Seeds reaching this class need MAPs that take seeded time: TASK-149.22. Structure: the region's lock is the one owner of mapped and the MAPs it covers; nothing to restructure. Mutant patch guest/sim/mutants/task-44.19-map-to-unlocked.patch. The other survivors in these notes: prune_in_use went with fxfs_dax_prune (fafefbac7); inherited_acls_written_back and fault_ignores_fatal_signals, missed by the DAX gate's suites, are killed by fxfs-guest-crash (r307, ENODATA, TASK-79.29) and by window_held_killed (r307, 'process still runs', TASK-79.49).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Every lock, gate check and seals recheck of dax.c and device.c was taken out in turn and run (scripts/guest-mutants-gce.py). Every survivor is now killed. The last, map_to_unlocked, which no guest suite and no 3,000 seeds of guest/sim found, is killed by guest/sim's reaches_at_once, through a MAP the sim's device holds at a test's word: red with the mutant, green without. zloop.sh (CI) now runs tests/scenarios' scenarios, and 3,000 seeds pass. DoD 1: no Rust changed; the workspace at main passed (ab17d2d87). DoD 2: upstream-changes.py --check passes. DoD 3: nothing of guest/linux changed.
<!-- SECTION:FINAL_SUMMARY:END -->
