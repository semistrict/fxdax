---
id: TASK-149.8
title: >-
  Directories, exchange, tmpfiles and open unlinked files across cuts in the
  model test
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-01 06:13'
updated_date: '2026-11-01 21:42'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 509000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The model has one directory and no open files. Unlinked but open files and tmpfiles are graveyard work, the area of the trim bug that spun forever and of orphans. Exchange and renames across directories are multi-object transactions.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Sequences use subdirectories, exchange, tmpfiles and files unlinked while open
- [x] #2 Cuts check each against the model
- [ ] #3 A hunt has run, its findings fixed or filed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Model: directories, Exchange, Tmpfile, Open/Close, queued unlinks, each as the guest's VFS asks. 2. Clones at awkward offsets across huge holes: plans as sparse runs. 3. Cuts: open files gone, nothing left (object count, graveyard, fsck warnings). 4. Hunt, fix, record.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Results (branch test/model-namespace)

Model now: subdirectories (names 0..NAMES stay the root's, so pinned sequences mean what they did, and shrinking reaches the root), renames across directories, directories moved, replaced and removed, exchange, tmpfiles and linking one, hard-linked symlinks, directory xattrs, files and directories open with no name, unlinks queued as a guest's (unlink_later, every subset of the queue a cut may find undone), a graveyard in batches as a guest's, clones at awkward offsets across holes of any size, clones to a source's end, punches likewise. Only what a VFS asks is asked (may_rename, may_exchange, Link's I_LINKABLE; read from guest/linux/fs/fxfs/super.c and guest/rust/fxfs-linux). Cuts check: fsck with warnings failing, object count, graveyard entries and extents, so nothing of what no name linked stays. Workers apply the queue only with FXDAX_MODEL_WORKERS: a clock wakes them and a seed wouldn't replay. crates/fxfs-posix/mutants.sh puts 15 bugs back: all caught at the suite's own 96 cases.

Hunts on 87b9cb43b (default, every cut, device failures, fsck each, 200 ops): 4200, 4200, 4200, 4680, 1440 cases; 313,430 cuts. FXDAX_MODEL_WORKERS 4200 cases, 8993 cuts, on c9ce14efe, whose non-test code is the same. No panic. Earlier models: 12,600 more. One failure on 87b9cb43b, long4/11, and one the commit before, long3/12, both finding 3 below; false alarms: 3.

## Bug 1: a punch spun as long as a file may be (fixed)
fallocate(PUNCH_HOLE) across a huge hole took a transaction a MiB of its range, with the inode locked: 2^30 of them for a hole of 1 PiB. Found looking around generic/303, not by a hunt. Blind spot: the model punched only small ranges. Red with the bug back: a_punch_crosses_holes_of_any_size is aborted by the watchdog at 120 s. Fix: zero_in_commits, by extents. Structure: repeats generic/303, the same mistake in the clone path, as each caller picked its stride. Small: refactor first (30c3745e4), one place splits a punch, end_of_extents stays private, so no caller names a stride. Then the fix (90cb66ca4). See the open question below.

## Bug 2: the generic/303 blind spot went further than the task said
Put back, the model passed: the clone ran until the device filled, about 100 s, and NoSpace is an answer a model must accept. Offsets alone never go red. Closed by bounding what clones and punches journal by the extents they cross (journaled_for): red in under a minute, a_clone_crosses_holes_of_any_size and the random test.

## Finding 3: bytes a truncate cut come back on a full filesystem (narrowed, NOT fixed)
Found by 200-op hunts on the tiny device. Truncate's eager zero_tail finds no room, setattr logs it and goes on, and a later growth shows the old bytes, the file's tail read back as data. Cause: frees wait for a sync. Fix (lib.rs setattr): sync and retry, pinned by a_truncate_on_a_full_filesystem_leaves_nothing_past_the_end, red with the retry out. Not closed: with no block free even after a sync, shared with a clone so the cut freed nothing, the bytes still come back: 2 in 2880 sequences of 200 ops (long3/12, long4/11), none in 16,800 of 48 ops, long3/12 probed as that branch, long4/11 by its cause, a truncated size read back after a clone grew the file. A long hunt may go red on it, and the default test could, rarely. Not pinned, as a red test can't land, and no task filed, as this subtask was the only one to touch. Replay: seed 0, TIGHT, Create(1), Exchange(0,0), Read{0,0,1}, Open(0), Write{0,0,1,1}, Remount, Truncate{15,4294967297}, Fill{13,58,2051,2,139}, Fill{12,5,2519,2,254}, Create(14), Write{1,52953,4096,55}, Truncate{4,4294971308}, Write{2,32768,4096,7}, CloneToEnd{0,9,508,17}, Truncate{15,54584}, UnlinkLater(7), Clone{2,14,5,5,515}.
Structure: architectural, for Ramon. See below.

## False alarms, in the model, not fixed in code
A punch of many commits taken for one transaction (now a Plan; a_punch_cut_short_leaves_what_it_got_to pins it). NoSpace from a rename over a non-empty directory before it could look. Watchdog aborts of 200-op sequences: slow, not hung: gone with the limit scaled.

## Blind spots of the model itself, found by mutants
Cuts took a rename overtaking a queued unlink for an unlink not done: the ordering rule is now asserted (keeps_the_order_of). Three mutants were found only by chance (rename and fsync overtaking, a directory's move): pinned. tombstone-now-leaves-the-entry was missed by 6000 sequences: a Trim left by a truncate short of room is too narrow, so a test makes that state (tests.rs).

## For Ramon: two structural questions, nothing restructured
1. Nothing owns how much a transaction holds. Each multi-commit operation guesses: MAX_ZEROED_PER_COMMIT (two now), MAX_CLONED_PER_COMMIT, MAX_CLONED_BYTES_PER_COMMIT, WRITE_MANY_BYTES, BATCH_MUTATIONS, TRANSACTION_MUTATION_THRESHOLD counting mutations, not bytes. Bugs it explains: TASK-139's panic, a clone of thousands of fragments, a dozen 1 MiB extents past TRANSACTION_MAX_JOURNAL_USAGE, generic/303, bug 1. What I'd change: the transaction reports the room it has left, by serialized size, and every such loop adds until it says stop and commits, as trim_some nearly does.
2. Nothing owns what lies past a file's end. Upstream's grow zeroes the old tail, in the transaction that grows, where NoSpace is a legal answer; writes, clones and DAX grow without it, so generic/130's fix zeroes eagerly at truncate, where a shrink may not refuse. Bugs it explains: generic/130, finding 3, TASK-79.53's stores past the end. What I'd change: every non-DAX growth goes through one path that zeroes the old tail, checking it first as it is nearly always zero, and the eager zero goes; TASK-80's reserved writeback must pay for that block. The narrower alternative: a shrink to mid-block refuses with NoSpace when no block exists, reversing TASK-60 for that case.

## Unverified
No guest ran. The sync in setattr hasn't run under a kernel's inode lock. KEEPS_DIRECTORY_TIMES is false in the model, true in a guest: flipping it would change what pinned seeds reproduce. Concurrency: workers only, by the clock, and no VFS calls at once (TASK-149.10). DoD 1: Docker, not Lima. Workspace on Linux at 87b9cb43b: 2063 of 2067; tenant_sockets_work_bind_mounted_into_jails fails only in Docker, and guests_forks_failing_at_each_step_harm_nothing, hundreds_of_writers_flush_on_threads_they_share and serves_hundreds_of_forks_of_a_base failed once under that run's load, passed the run before and passed alone after; this branch changes nothing in crates/fxdax. macOS workspace 1926 of 1926 before 87b9cb43b, its last test-only commit; fxfs-posix 39 of 39 at 87b9cb43b. AC 3 stays open for finding 3.
<!-- SECTION:NOTES:END -->
