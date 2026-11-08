---
id: TASK-149.2
title: 'Seeded, replayable, shrinking operation scripts for DAX in a guest'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 06:13'
updated_date: '2026-11-08 11:22'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: high
ordinal: 503000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Most bugs are in DAX, where random_ops runs but failures take hours to pin down: no operation script to replay and no shrinking, and few failures combined. This brings the model test's method to a real guest, on top of TASK-44.15's oracle: scripts of mmap, DAX writes, reads through mappings of the same file, fork, truncate, a nearly full disk and VM kills, generated on the host from a seed.

At 9f81f6018 (writable DAX deleted, 37c331a6c): the past bugs AC5 named went with writable DAX: TASK-140's livelock (its mutant task-140-read-only-slice.patch, deleted) and the moved-unmapped mutants (no page moves now). Copy-on-write DAX's own are in guest/mutants (one set since b7653f781, each patch's header naming its killer and kernel): unmapped-page-read-as-hole (a load reading zeros where read(2) reads a write's bytes) and the task-164.4-* writeback mutants, such as task-164.4-protect-by-entries (a fault's writable PTE left across a writeback, then a store the host refuses).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A script is generated from a seed and a failure prints the seed
- [x] #2 A failing script shrinks to a smaller one that still fails, in a VM
- [x] #3 Scripts combine mappings, fork, a nearly full disk and a VM killed at a random point
- [x] #4 Each file is checked against a model, after a kill as after a clean stop
- [x] #5 Reintroducing a past copy-on-write DAX bug fails in a VM, shrunk: task-149.16-short-write-cut-to-its-start (a DAX write past the end, stopped short, cut back), killed by fxfs-guest-fsx-dax and shrunk in the guest to 12 of 4,082 operations (GCE r931). Not the two first named: task-164.4-protect-by-entries passes the release kernel's DAX suites (GCE r1000), and unmapped-page-read-as-hole needs a truncate's MAP of zeros to fail, which no fault injection does (decided 2026-11-08)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Existing: test_random_ops has seeded choices, a byte model, a mapping kept across operations, offsets at page, chunk and file ends, and a history of its operations; test_random_ops_threads adds threads. Missing: operations decided outside the guest, shrinking, several files, reflinks, fork, a nearly full disk, a VM killed. 1. Split test_random_ops into a generator, a pure function of the seed giving a script of text lines, and an interpreter that runs a script in the guest, checks against the model and reports the failing line. Same behaviour, proven by replaying a run. 2. The host writes the script, passes it to the guest, and on a failure shrinks it by deleting chunks of lines and rerunning, a VM each try, keeping the smallest that fails the same way, with a bound on tries. 3. New operations: several files, FICLONE, fork with the child writing through the mapping, a small volume where ENOSPC is allowed and leaves the model unchanged. 4. A VM killed at a line, then remounted, each file checked against states since its last fsync, as the Rust model does. 5. Prove it: put back a moved-unmapped mutant, and TASK-140's livelock, and each must fail and shrink.

Changed as built (steps 1-3, branch test/dax-scripts): a script is a list of typed operations, not text lines, in guest/tests/test_cases/src/script.rs, which both builds compile; the guest makes it again from the seed, and a replay is told the seed and the indices to drop (FXDAX_RANDOM_OPS_DROP=12-40,100-110), as the kernel's command line is the only channel, so no script file goes to the guest. Steps 4 (VM kills) and 5 (mutants, GCE) are still to do.

Step 4 as built (2026-11-08): fxfs-guest-script-killed(-dax): the guest does the operations before script::kill_index (seeded), says 'kill me' and waits; the host kills the VM process (Vm::kill_on); the host's Fxfs replays and fscks the store; each file must hold what script::Survivals allows: a size it had since its last fsync, and each page as one of its states since then had it (page by page, as writeback takes pages at different times). The model is replayed on the host (script::survivals), pure for this mix on a volume with room. Shrinks through script_vm's VM replays, the drops written beside the seed for the verdict.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-01, steps 1-3, commits 7bbc99422, 73d396b24, 6d787c3aa, e2a22e24b. NOT RUN IN A VM: no KVM on the machine that built them. Verified in the Linux container: both builds compile; cargo nextest of test_cases, guest feature 50 passed, host feature 52 passed (sqlite's 3 filtered out: the container has no sqlite3). script_run's tests run the interpreter on the host's filesystems: a real 3000-operation script, FICLONE among them on overlay over btrfs, and full ext4 and XFS on loop devices (16 seeds: 24818 and 374 refusals, the model held throughout). A planted bug shrinks to its pair, real runs each try. script_vm's tests shrink through a fake runner process, a planted pair in 48 tries, of 64 allowed, and a hang.

Deviations: ENOSPC does not always leave the model unchanged: fxfs_vfs_remap_file_range says what a refused clone got to stands, and fxfs_dax_punch_begin zeroes a punch's partial blocks before the rest, so the model takes those two from what the files read, within what they could have come to (clone_stopped_short, punch_stopped_short, after fxfs-posix's stopped_short); those paths are unit tested, but no filesystem here was seen refusing one. On a full volume every store is a child's, as a store with no room is a SIGBUS, which would kill the guest's init. FICLONE onto a longer file with a partial last source block is EINVAL (generic_remap_check_len), which the model expects. Whole-file checks are operations of the script, so a shrink can drop them; the interpreter always checks at the end. random_ops keeps one file and its mix; several files, clones and forks are the new cases fxfs-guest-script, -dax, -full, -full-dax, not yet in test-e2e.sh's list.

Unknown until GCE: whether 180 s holds a script, whether a script that refuses on Fxfs comes from LEFT = 1 MiB on the 64 MiB volume (a run refused nothing fails, kind 'nothing refused'), and whether replays reproduce: a timing bug may not, and then the report says it isn't shrunk.

Handed back 2026-11-02 (triage): steps 1-3 on main (7bbc99422, 73d396b24, 6d787c3aa, e2a22e24b), fxfs-guest-script* pass on GCE r429. Left: step 4, kill the VM at a random line, remount, check each file against the model (AC3's kill, AC4); step 5, a past DAX bug or mutant failing on GCE and shrinking (AC5). TASK-149.16 depends on it.

2026-11-05 triage: the mutants it names moved from guest/sim/mutants to guest/mutants (b7653f781).

2026-11-08 audit against main, with evidence:
- AC1 holds: script::generate(seed, mix) is a pure function of the seed (script.rs). A failure says the seed, and how to replay it with FXDAX_SEED and FXDAX_RANDOM_OPS_DROP (script_vm.rs Replays::report). fxfs-guest-script* passed on GCE r429.
- AC2 holds: script_vm boots a VM for each try of a shrink. TASK-149.16's GCE r931 shrank a fxfs-guest-fsx-dax failure in the guest to 12 of 4,082 operations, and a replay from a boot failed the same way. Here, r1001e shrank killed-run failures through VM replays: 2 of 3,314 operations in 34 tries, and 2 of 3,258 in 22.
- AC3, AC4: the kill was missing. Built now: fxfs-guest-script-killed and -killed-dax (Tag::Nightly), script::{kill_index, Survivals, survivals}, KILLED_DIFFERS. Not combined with the nearly full volume: what a refused clone or punch leaves, the guest reads back, which the host can't after the kill. The full cases still run without a kill.
- Found: the DAX script cases never used DAX. The mix's files are made on the DAX mount and never read in again, and copy-on-write DAX makes them page-cache inodes until then; the fsx mix reopens them, this one didn't. Now the DAX cases reopen each file first and fail NEVER_DAX unless all are DAX.
Tests: script::tests a_killed_file_holds_pages_of_its_states_since_its_last_fsync and kill_indices_fall_within_the_script; test_fxfs_guest_script::host a_killed_runs_files_are_checked_as_the_store_holds_them (on a real store: right contents pass, a flipped page fails KILLED_DIFFERS, a run never killed fails DIED).
GCE, one host, fxdax-test-r1001-t1492, us-east1-b, release guest kernel; stopped after, not deleted, and its max run duration cleared:
- r1001a: killed and killed-dax pass, 3 rounds each.
- Planted, fsync skipping DAX writeback in fxfs_vfs_fsync (r1001b, r1001c): passes. writepages writes those pages back too, so the plant changed nothing.
- Planted, fxfs_dax_write_back doing nothing (r1001d): the guest died before the kill; not a test of the check.
- Planted, fsync doing nothing for regular files (r1001e): killed-dax fails 'killed differs: script-2: 3447697 bytes, where it was {3496514, 3506653}', shrunk in VMs to 2 of 3,314 operations (a write and an fsync of file 0); killed fails 'script-0: 0 bytes', shrunk to 2 of 3,258 (a write and an fsync of file 1).
- r1001f, the clean tree: script, -dax, -full, -full-dax, -killed and -killed-dax all pass, 3 rounds each.
- AC5: task-164.4-protect-by-entries in a release kernel (GCE r1000, fresh host before the user's reuse rule, us-east1-b): fxfs-guest-fsx-dax, -fsx-delay-dax, -script-dax (then page-cache files) and -checks-dax all pass. The mutant isn't killed in a VM; its header says so. unmapped-page-read-as-hole needs a truncate's MAP of zeros to fail, which no fault injection does (dax_fail_maps fails ranges' maps of more than one piece). A past copy-on-write DAX bug does fail in a VM, shrunk: task-149.16-short-write-cut-to-its-start (GCE r931, 12 of 4,082 operations). AC5 stays open as worded.

2026-11-08, decided by the lead: AC5 accepts task-149.16-short-write-cut-to-its-start, a past copy-on-write DAX bug that fails in a VM and shrinks (GCE r931), and is reworded to say why the two first named don't serve. Filed: protect-by-entries surviving the release kernel's suites, and fault injection of a truncate's MAP of zeros for unmapped-page-read-as-hole. AC3: try modelling a refused clone or punch on the full volume as allowed states, so that a kill can follow a refusal; if that takes over about a day, reword AC3.

2026-11-08, AC3 done on a follow-up branch, the kill on the nearly full volume too. Rather than branching the model into sets of states (later operations resolve their places against a size the host wouldn't know), the guest says what each refused operation left of the file it changes: script::Partial, a new length and the byte ranges that read as the operation done whole has them, as before elsewhere. That's what a cut-short write or child's store, a stopped clone and a part-way truncate or punch leave (Model::stopped_short checks it). An fsync refused is 'unsynced', and resets no window. Run::step derives it from the model before and after when Options::partials and full. The host replays by those lines (script::survivals). New cases fxfs-guest-script-killed-full and -killed-full-dax.
Tests: partials_are_said_and_replayed. GCE r1001g: killed-full and killed-full-dax pass, 3 rounds each, 79 to 1,117 operations refused before the kill; killed and script-full pass too. r1001h, planted fsync doing nothing: killed-full fails 'killed differs: script-0: 3163035 bytes, where it was {3403873, 4010308}', shrunk in 31 tries to 2 of 3,299 operations (a truncate and an fsync of file 2). killed-full-dax passed both planted rounds; the same plant failed killed-dax (r1001e), so the check works with DAX, and on the full volume the data reached the store some other way (presumably the worker's writeback under its pressure; not confirmed). Host stopped after, not deleted.

Verified 2026-11-08: on main, GitHub CI 42469441083 green at 08d444ef7 (macOS and Linux workspace, shuttle, sim, guest-sim, firecracker), scripts/upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Seeded, replayable, shrinking operation scripts for DAX in a guest: generated from a seed, shrunk in a VM, combining mappings, forks, a nearly full disk and a kill, checked against a model, and shown to catch a reintroduced copy-on-write bug (on main via land-r4/r5 in land-all14; GCE r1001g/h).
<!-- SECTION:FINAL_SUMMARY:END -->
