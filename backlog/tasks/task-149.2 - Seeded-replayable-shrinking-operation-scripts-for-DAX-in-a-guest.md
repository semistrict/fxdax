---
id: TASK-149.2
title: 'Seeded, replayable, shrinking operation scripts for DAX in a guest'
status: To Do
assignee:
  - '@claude'
created_date: '2026-11-01 06:13'
updated_date: '2026-11-03 02:10'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: high
ordinal: 503000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Most bugs are in DAX, where random_ops runs but failures take hours to pin down: no operation script to replay and no shrinking, and few failures combined. This brings the model test's method to a real guest, on top of TASK-44.15's oracle: scripts of mmap, DAX writes, reads through mappings of the same file, fork, truncate, a nearly full disk and VM kills, generated on the host from a seed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A script is generated from a seed and a failure prints the seed
- [ ] #2 A failing script shrinks to a smaller one that still fails, in a VM
- [ ] #3 Scripts combine mappings, fork, a nearly full disk and a VM killed at a random point
- [ ] #4 Each file is checked against a model, after a kill as after a clean stop
- [ ] #5 Reintroducing a past DAX bug (TASK-140's livelock, or a moved-unmapped mutant) fails, shrunk
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Existing: test_random_ops has seeded choices, a byte model, a mapping kept across operations, offsets at page, chunk and file ends, and a history of its operations; test_random_ops_threads adds threads. Missing: operations decided outside the guest, shrinking, several files, reflinks, fork, a nearly full disk, a VM killed. 1. Split test_random_ops into a generator, a pure function of the seed giving a script of text lines, and an interpreter that runs a script in the guest, checks against the model and reports the failing line. Same behaviour, proven by replaying a run. 2. The host writes the script, passes it to the guest, and on a failure shrinks it by deleting chunks of lines and rerunning, a VM each try, keeping the smallest that fails the same way, with a bound on tries. 3. New operations: several files, FICLONE, fork with the child writing through the mapping, a small volume where ENOSPC is allowed and leaves the model unchanged. 4. A VM killed at a line, then remounted, each file checked against states since its last fsync, as the Rust model does. 5. Prove it: put back a moved-unmapped mutant, and TASK-140's livelock, and each must fail and shrink.

Changed as built (steps 1-3, branch test/dax-scripts): a script is a list of typed operations, not text lines, in guest/tests/test_cases/src/script.rs, which both builds compile; the guest makes it again from the seed, and a replay is told the seed and the indices to drop (FXDAX_RANDOM_OPS_DROP=12-40,100-110), as the kernel's command line is the only channel, so no script file goes to the guest. Steps 4 (VM kills) and 5 (mutants, GCE) are still to do.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-01, steps 1-3, commits 7bbc99422, 73d396b24, 6d787c3aa, e2a22e24b. NOT RUN IN A VM: no KVM on the machine that built them. Verified in the Linux container: both builds compile; cargo nextest of test_cases, guest feature 50 passed, host feature 52 passed (sqlite's 3 filtered out: the container has no sqlite3). script_run's tests run the interpreter on the host's filesystems: a real 3000-operation script, FICLONE among them on overlay over btrfs, and full ext4 and XFS on loop devices (16 seeds: 24818 and 374 refusals, the model held throughout). A planted bug shrinks to its pair, real runs each try. script_vm's tests shrink through a fake runner process, a planted pair in 48 tries, of 64 allowed, and a hang.

Deviations: ENOSPC does not always leave the model unchanged: fxfs_vfs_remap_file_range says what a refused clone got to stands, and fxfs_dax_punch_begin zeroes a punch's partial blocks before the rest, so the model takes those two from what the files read, within what they could have come to (clone_stopped_short, punch_stopped_short, after fxfs-posix's stopped_short); those paths are unit tested, but no filesystem here was seen refusing one. On a full volume every store is a child's, as a store with no room is a SIGBUS, which would kill the guest's init. FICLONE onto a longer file with a partial last source block is EINVAL (generic_remap_check_len), which the model expects. Whole-file checks are operations of the script, so a shrink can drop them; the interpreter always checks at the end. random_ops keeps one file and its mix; several files, clones and forks are the new cases fxfs-guest-script, -dax, -full, -full-dax, not yet in test-e2e.sh's list.

Unknown until GCE: whether 180 s holds a script, whether a script that refuses on Fxfs comes from LEFT = 1 MiB on the 64 MiB volume (a run refused nothing fails, kind 'nothing refused'), and whether replays reproduce: a timing bug may not, and then the report says it isn't shrunk.

Handed back 2026-11-02 (triage): steps 1-3 on main (7bbc99422, 73d396b24, 6d787c3aa, e2a22e24b), fxfs-guest-script* pass on GCE r429. Left: step 4, kill the VM at a random line, remount, check each file against the model (AC3's kill, AC4); step 5, a past DAX bug or mutant failing on GCE and shrinking (AC5). TASK-149.16 depends on it.
<!-- SECTION:NOTES:END -->
