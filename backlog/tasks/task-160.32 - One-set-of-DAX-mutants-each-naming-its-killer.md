---
id: TASK-160.32
title: 'One set of DAX mutants, each naming its killer'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-03 13:09'
updated_date: '2026-11-06 02:22'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 603000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
scripts/guest-mutants-gce.py (Python string edits) and guest/sim/mutants (patches) are two mutant sets; the gate has spent hours on GCE mutants the sim finds in seconds; neither records which check should kill a mutant. Proposal: one directory of patches with a header naming the expected killer and kernel; the gate runs the sim first and sends to GCE only what needs a real kernel; killed only when the named killer fails. After the CI and results tasks.

At 9f81f6018, writable DAX's deletion (37c331a6c) took eleven of guest-mutants-gce.py's mutants (the seals rechecks, seal_keeps_writable, held_not_written, the moved_* and moves_before_bytes, map_failure_unretried) and most of guest/sim/mutants' writable-DAX ones. Left: six GCE mutants (map_to_unlocked, resize_moves_the_last_block, regions_find_unlocked, inherited_acls_written_back, fault_ignores_fatal_signals, store_fails_on_pins; scripts/guest-mutants-gce.py:41-) and 28 sim patches. One bug is in both: map_to_unlocked and guest/sim/mutants/task-44.19-map-to-unlocked.patch, which, copying on write, neither the sim's oracles nor any guest suite kills (guest/sim/README.md:187, guest-mutants-gce.py:42-43). The headers' killers would say so.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 One mutant set
- [x] #2 Each mutant's fate is its named killer
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Move guest/sim/mutants to guest/mutants; turn guest-mutants-gce.py's six string edits into patches there, map_to_unlocked merged with task-44.19-map-to-unlocked.
2. Header per patch: '# Kernel: sim|release|debug|kasan' and '# Killer: <sim test [args]> | <guest case [check]> | none', prose saying why.
3. Find each mutant's killer: every sim one-run test and scenario plus 300 seeds of interleave and failures, per mutant; what the sim can't kill goes to a real kernel.
4. guest/sim/mutants.sh: runs each sim mutant's killer alone; killed only if it fails. guest-mutants-gce.py: runs each real-kernel mutant's killer case alone (FXDAX_CHECKS = its check), fate from summary.json's failed_checks; --check validates every header and applies the real ones.
5. dax-gate.sh: sim mutants first (guest/sim/linux.sh, a container on macOS), then GCE's. Drop the mutants and dax-gate-mutants tags.
6. Verify: mutants.sh --check and a full sim run; FXDAX_TEST_PARTS=scripts scripts/test.sh; one GCE run of the real-kernel mutants (r625).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Design: guest/mutants/*.patch is the one set (36), README.md there. Header '# Kernel: sim|release|debug|kasan' and '# Killer: <sim test [args]> | <guest case [check]> | none'. guest/sim/mutants.sh runs the Kernel: sim ones, each its killer alone (a one-run test, 'scenarios NAME', or interleave/failures seed by seed), killed only if that run fails; it checks the killer is a run there (a 'scenarios nope' would otherwise exit 1 and count as a kill). scripts/guest-mutants-gce.py runs the rest on GCE, each its killer's case alone (FXDAX_E2E_TEST) with FXDAX_CHECKS its check, fate from summary.json: killed only if the case failed/timed out with the check in failed_checks (case alone: any failure, as a console complaint); MISSED if it passed or failed otherwise; BROKEN if the run failed without the case failing. --check validates every header against lib.rs's cases, mod.rs's checks, guest/sim/tests and scenarios.c, and git-apply-checks the real-kernel patches; mutants.sh --check applies the sim ones. dax-gate.sh runs the sim's first (guest/sim/linux.sh: Linux as is, elsewhere a Docker container of guest/sim/Dockerfile, seccomp off for personality(2)) and stops before the hosts if one survives. Tags mutants and dax-gate-mutants removed.

Killers found by running each mutant against every sim one-run test and scenario plus 300 seeds of interleave and failures. Sim: 27 killed (2:11 for all, 0-15 s each); regions_find_unlocked moved from GCE's debug kernel to the sim (read_mappings_stay, lockdep assertion, in a second); task-79.28-before-147 now killed by scenarios store_past_end_then_written, task-79.53-gap-left by failures seed 2. task-164.4-release-stale-entries had decayed into a self-deadlock (filemap_fdatawrite_range -> writepages under the release's file->sem): every run died at once and its scenario was never reached; rewritten as the cleaning taken out, killed by cow_reclaim_after_writeback_met_a_fault. none: patch-0012, 153.1, 79.28-range-too-small, 164.4-truncate-unwritten (equivalent), task-44.19-map-to-unlocked (merged with GCE's map_to_unlocked; killed by neither, says so).

GCE fxdax-test-r625 (us-west1-b), through guest-mutants-gce.py as the gate calls it, FXDAX_E2E_ROUNDS=3, release: inherited_acls_written_back killed (fxfs-guest-crash, ENODATA at the host's getxattr). resize_moves_the_last_block MISSED (append_stopped_short_in_a_page alone 3 rounds; then fxfs-guest-checks-dax whole, 1 round): killed by nothing since copying on write -> none, TASK-149.30. fault_ignores_fatal_signals MISSED (window_held_killed alone 3 rounds; checks-dax-small whole 1 round): equivalent, the no-room loop is bounded (FXFS_DAX_NO_ROOM_WAITS) and its wait killable -> none. store_fails_on_pins MISSED (checks-dax whole, 3 rounds): its killer slice_held went with writable DAX -> none, TASK-149.31. Each run's guest kernel recompiled fs/fxfs with the patch (guest-kernel.log), commit -dirty. Host deleted. So the GCE half now runs one mutant (inherited ACLs): the rest the sim kills in seconds or nothing can.

Verified: guest/sim/mutants.sh --check (33 sim patches) and a full run (27 killed, 5 none, exit 0); scripts/guest-mutants-gce.py --check (36); FXDAX_TEST_PARTS=scripts scripts/test.sh (160 tests OK). Not run: dax-gate.sh end to end (its two halves were); lib.rs's test-module edit (a_selection_is_of_cases_named_and_tagged) is uncompiled: the guest test crate builds on Linux only, and the GCE host built the runner (lib, not tests). Structure: the bug class here was mutants decaying silently (a patch still applying but testing something else, a killer gone with its code); named killers make both visible as MISSED/BROKEN rather than a kill by whatever failed first.

Verified on main, 2026-11-05 (code of dde8d58d3; 752515020 adds backlog only): GCE r691, the Linux workspace, 2302 of 2303, the one failure TASK-187's a_store_after_a_workers_write_waits_for_one_lift (TASK-190; its fix 788e3b6b4 is on land-p, not main); GCE r690, the e2e suite (--tag e2e, 15 cases) jailed with the production seccomp filter, 15 of 15; CI 42137709782 on dde8d58d3 green (workspace 2303 of 2303, sim, shuttle, clippy scripts, guest, guest-sim); macOS FXDAX_TEST_PARTS=workspace scripts/test.sh at 752515020, 2118 of 2118; scripts/upstream-changes.py --check at 752515020 passes. AC #1: guest/mutants holds the one set (38 patches and its README), guest/sim/mutants is gone; every patch has a '# Killer:' header (rg --files-without-match finds only README.md). AC #2: guest/sim/mutants.sh's full run (27 killed, 5 none) and r625's GCE run as recorded above; CI 42137709782: guest-sim's 'The mutants apply' step, and mutants --check 17, 4 and 38 checked, 0 broken, in the scripts part; a_selection_is_of_cases_named_and_tagged, left uncompiled before, passes in its guest job.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
One mutant set, guest/mutants, each patch's header naming its kernel and killer; the sim's mutants run first, GCE's only what needs a real kernel, killed only by their named killer. On main: 4b6580817, b7653f781, b0caea65d. Verified: the full sim run and GCE r625 as noted; CI 42137709782 (mutants --check, guest-sim, the guest job).
<!-- SECTION:FINAL_SUMMARY:END -->
