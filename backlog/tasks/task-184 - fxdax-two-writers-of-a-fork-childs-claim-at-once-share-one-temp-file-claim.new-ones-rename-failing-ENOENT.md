---
id: TASK-184
title: >-
  fxdax: two writers of a fork child's claim at once share one temp file
  (claim.new), one's rename failing ENOENT
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 15:19'
updated_date: '2026-11-05 19:10'
labels:
  - remote
  - flaky
dependencies: []
priority: medium
type: bug
ordinal: 639000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen once on macOS (collector-fixes, 2026-11-05, load average 53): fxdax remote::fork_faults::each_write_of_a_failed_forks_cleanup_going_wrong failed in case [Fails("remote::marked"), Write(1, DiesAnswered)], Deleted, at 'a fork to a new child': Err("forking branch fee755b7f211548: recording the fork's origin: No such file or directory (os error 2)"). The test passed 7 of 7 alone (about 40 s; 222 s under load). crates/fxdax/src/remote.rs: claim() and claim_origin() (through write_durably) both write the child's claim through dir/<claim>.new; claim() removes it after linking, write_durably renames it. Any two at once for one child (a retried write, an uploader and a fork) lose one's file: ENOENT. Not touched by the collector fixes of TASK-173/174/183, whose changes only shift timings.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 #1 A test running two writers of one child's claim at once, red now
- [x] #2 #2 Each writer's temporary file is its own (or the writes are serialized), and the test passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Read the writers of a child's claim and markers in remote.rs; confirm the shared {name}.new.
2. Make each write's new file its own (write_new, new_file with a random part, create_new); unmark removes a branch's new files by listing.
3. Test: a failpoint between a writer's new file and its putting it in place, a second writer running there: fork vs origin, origin vs fork, marker vs marker. Red with the shared name, green with the fix.
4. Stress the fork_faults test under load; the fxfs, fxfs-posix and fxdax suites; the macOS workspace.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Cause (confirmed by reading crates/fxdax/src/remote.rs)
Every writer of a branch's files beside its store wrote its next bytes to one fixed new file, {name}.new: write_durably (markers remote-*, fences fence-*, and claim_origin's rewrite of a claim origin-*), and claim(), which links its new file into place and then removes it. Two writers of one file at once shared it: the second's File::create truncated the first's and wrote its own bytes there, then removed it (claim) or renamed it (write_durably) from under the first, whose rename failed ENOENT. The parent's uploader failing there (claim_origin: 'recording the fork's origin: No such file or directory') stops its uploads, and the next fork reports it, as in the failure. The writers that meet: a fork claiming its child (main thread, under the store's committing lock) and the parent's uploader naming the origin of a fork to that ID (a fork retried to the same child while the first's segment uploads, or a segment of an earlier fork to the ID gathered meanwhile); the parent writing a child's marker and the child's own uploader; and unmark, which removed {name}.new too.
Which pair met in the failing run isn't shown: the run's trace isn't kept, and 30 full runs of the test (30 at once, load 45 to 75) and 480 runs of its failing case alone (40 at once) passed on the unfixed code. The test's world has one main thread and the parent's uploader writing in the restarted directory, and the code above is their only shared file.

## Fix
One helper, write_new, writes every new file, created exclusively (create_new) under a name of its own, {name}.new-{16 hex digits, random} (new_file), synced, removed again if the write fails; write_durably renames it over the file (removing it if the rename fails), claim links it. unmark lists the directory and removes the branch's files and every new file of them (is_of), which a crash left; a shared name had let it find the one by name.

## Proof
- writers_of_one_file_at_once_each_write_their_own (remote.rs): a failpoint, remote::written, where write_new has written its new file, runs a second writer of the same file there: the uploader naming the origin while a fork to the child claims it; a fork claiming while the uploader names; two marker writes. Red with the shared name (new_file back to {name}.new, File::create): Err("recording the fork's origin: No such file or directory (os error 2)"), the failure's own message. Green with the fix.
- unmarking_removes_new_files_a_crash_left: a branch's files and their new files go, another branch's stay.
- host_store's deletions_stopped_at_a_branchs_records_finish leaves a new file of the new name.
- cargo nextest run -p fxfs -p fxfs-posix -p fxdax: 1126 of 1126 (macOS).

## Blind spot
No test ran two writers of one file of a store's directory at once: each writer's tests ran it alone, and the fault tests run one fork at a time, so their writers meet only as timing lets them, under load. The new test pauses one writer where a second does harm, for each pair, through the one helper every writer now uses.

## Structure
The class is gone for these files: one helper, a new file per write. What's left is the read-then-write of a claim by the uploader, which nothing orders with the claim's removal, nor with a later fork's claim of the ID: TASK-186. The bases file (extents.rs write_bases) and a fork handle's description (extent_store describe) keep a fixed {name}.new, noted in TASK-160.13, one durable write for a store directory's records, which this helper starts.

## Verified (macOS, 2026-11-05)
- writers_of_one_file_at_once_each_write_their_own and unmarking_removes_new_files_a_crash_left: 500 of 500 (nextest --stress-count 500).
- fork_faults::each_write_of_a_failed_forks_cleanup_going_wrong, the fixed build: 500 of 500, 30 at once, with 8 busy loops (load 12 to 22). Unfixed: 30 of 30 and its failing case alone 480 of 480 passed too, so the stress alone doesn't show it; the deterministic test does.
- cargo nextest run -p fxfs -p fxfs-posix -p fxdax: 1126 of 1126. FXDAX_TEST_PARTS=workspace scripts/test.sh --no-fail-fast: 2133 of 2133. scripts/upstream-changes.py --check passes. GCE (Linux) not run: DoD #1's Linux half open.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Each durable write of a file beside a store (markers, fences, claims) now writes a new file of its own ({name}.new-{random}, create_new, write_new) before renaming or linking it into place; unmark removes a branch's new files by listing. A shared {name}.new let two writers of one file at once truncate and remove each other's, the uploader failing ENOENT naming a fork's origin. Red/green: writers_of_one_file_at_once_each_write_their_own, a failpoint pausing one writer while a second writes, red with the shared name with the failure's own error. fork_faults' test 500 of 500 under load; macOS package suites and workspace green; GCE not run. The claim's read-then-write races left are TASK-186.
<!-- SECTION:FINAL_SUMMARY:END -->
