---
id: TASK-395.7
title: >-
  Shared points: base images and snapshots other tenants may clone, by a flag in
  their record
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-13T08:53:00Z'
updated_date: '2026-11-13 21:55'
labels:
  - bucket
  - collector
  - security
dependencies:
  - TASK-395.3
parent_task_id: TASK-395
priority: medium
ordinal: 779000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Today any tenant can be given another tenant's point to restore, and ForkStore makes a cell of the caller's own whatever the parent's, so inheritance crosses tenants freely. The user (2026-11-13): sharing across real tenants is not a requirement, except a set of base images and snapshots that many tenants clone. Those are marked in the point's record (decided over a reserved tenant ID: no special tenant, any point can be published in place, and tenant 0 already means the daemon's own stores). A record format change.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A point's record says whether it is shared; the trusted control API (and fxdaxctl) makes a point shared at its making or later, and unshares it, which stops new clones only
- [x] #2 A clone of a point of another tenant's, by restore, ForkStore or any path, is refused unless the point is shared, checked at the read of the point's record the clone already makes; clones within a tenant are unchanged
- [x] #3 A clone of a shared point writes its reference under the point's cell as any clone does, and the point's cell keeps the point's state while any reference stands, deleted point or not
- [x] #4 Tests: a tenant clones a shared base; a tenant cannot clone another's unshared point (EACCES); deleting a shared base with live clones in other tenants loses none of their data
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_remote: Record.shared (Flags.shared, only on points, else Corrupt); FORMAT 7 -> 8, 7's records refused (Error::Format); Error::NotShared(point) and Error::NotPoint(branch).
2. branch::share(bucket, point, shared): CAS of the point's record; NoBranch, Deleted, NotPoint refused.
3. branch::finish_point_fork: the read of the point's record the fork already makes (now made also when the child's tenant differs from the point's, whatever forks_check_points says) refuses a child of another tenant's cell unless shared (NotShared). Every clone of a point (restore via fork_from, ForkStore from a point copy, fxdaxctl, kept images) goes through Remote::forked -> fork_point -> finish_point_fork.
4. fxdax: NotShared -> EACCES, NotPoint -> EINVAL; host_store::share / ExtentStore::share(path, shared) from the store's description (branch, bucket); daemon: SharePoint rpc, ForkStore.shared (point made, then shared; EINVAL without point); EACCES not the point store's fault (no let-go, no retry); fxdaxctl share/unshare/point --shared; go/ regenerated.
5. Tests: record flags/format, branch::share and refusal (extent_remote); a point forks into another tenant's cell only while shared, and its clone keeps its data after the point is deleted and collected (fxdax remote.rs); fxdaxctl local share/unshare; Linux grants: restores of another tenant's point refused until shared, ForkStore of shared points into other tenants.
6. AC #3 (references) left to TASK-395.3. Writable cross-tenant forks (ForkStore of a tenant-0 public base into a tenant's directory) left as they are: follow-up task.

7. AC #3 (with TASK-395.10): collector test of a shared point cloned into another tenant: reference under the origin's cell, the point and origin deleted and swept twice keeping the clones' data, references retired once the clones go, nothing left.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Landed on main as a19ec824a (rebased from 88f81c2d1).
- extent_remote: Record.shared (a point's alone; shared on any other kind is corrupt); FORMAT 7 -> 8, format 7's records refused by name (Error::Format); branch::share (CAS of the point's record; NoBranch, Deleted, NotPoint); finish_point_fork's read of the point's record now refuses a child of another tenant's cell unless shared (Error::NotShared), the read made for such a child even with forks_check_points off. Every clone of a point (restores via stores::fork_from, ForkStore from a point copy, fxdaxctl, images) goes Remote::forked -> fork_point -> finish_point_fork, so one check covers them.
- fxdax: NotShared -> EACCES, NotPoint -> EINVAL (remote::errno; host_store::fork_in keeps EACCES); ExtentStore::share(path, shared) from the store's description; control API SharePoint and ForkStore.shared (point made, then shared; EINVAL without point); EACCES is no fault of a kept point (stores::store_at_fault: no let-go, no retry); fxdaxctl share / unshare / point --shared; go/ regenerated; doc-7 Points (Shared points) and Kinds of record updated. Mutant forks-take-branches-claimed.patch re-anchored (its context moved).
- Tests: extent_remote record flags (128 combinations, 14 records) and format 7 refused; branch::forks_of_points_into_other_tenants_need_them_shared; fxdax remote::points_fork_into_other_tenants_cells_while_shared (refused EACCES leaving nothing, same tenant unchecked, shared/unshared, then the point deleted and two collector passes: the other tenant's clone still reads all it inherited, so AC #4's third test passes today without references); fxdaxctl points_are_shared_and_unshared_locally; Linux only, not run here: grants.rs tenants_restore_points_given_them_into_cells_of_their_own (restore refused until SharePoint, again after unshare) and bases_made_shared_points_fork_into_other_tenants (ForkStore, shared at making, EINVAL cases).
- Verified on macOS: clippy (test.sh part and default) clean; scripts/test.sh whole: workspace 2290/2290, sim 55/55, shuttle parts all passed, scripts and go passed, mutants 1 broken (fixed, scripts part re-run: 0 broken). After the rebase onto TASK-395.5's collector/bucket changes: clippy clean, nextest extent_remote+extent_store+fxdax 782/782.
- Not checked: AC #1 and #2 wait on ubu1's run of the Linux daemon tests (grants.rs); AC #3 needs fork references (TASK-395.3, not built); AC #4 partly (the daemon tests on Linux). Writable cross-tenant forks (tenant-0 public bases forked into tenants' directories) left unchecked: TASK-395.9. Decisions in OPEN_QUESTIONS.md. TASK-395.2's layout is now format 9.

ubu1 run t395w1 on main 09863b785 (a1348601d + a19ec824a, under fxjob): workspace 2563/2563 passed (86 skipped), sim 55/55, the other nextest parts all passed, the grants.rs daemon tests (restore refused until shared, ForkStore with shared points) among them; guest tests exit 0; scripts 307 OK. One failure: crates/fxdax/mutants/forks-take-branches-claimed.patch didn't apply under GNU patch: this task's hand re-anchoring left its first hunk counting 9 of its 10 lines, with 4 lines of context before and 2 after; BSD patch on macOS took it. Fixed in 7ed0b0290. Blind spot: the mutant scripts' check ran on BSD patch on macOS, laxer than GNU's -F0. Class test: 50512c5bf, scripts/lib/patches.sh's patch_well_formed (header counts, even context away from a file's ends) in all four crates' mutant scripts, scripts/tests/test_patches.py; red on the old patch, all 68 committed patches pass. Looked around: guest/sim/lib.sh applies patches only on Linux (GNU patch), not affected.

ACs #1, #2, #4 checked on t395w1's evidence: branch::tests::forks_of_points_into_other_tenants_need_them_shared, remote::tests::points_fork_into_other_tenants_cells_while_shared, fxdaxctl points_are_shared_and_unshared_locally, grants bases_made_shared_points_fork_into_other_tenants and tenants public_bases_are_shared_read_only passed on ubu1 as on macOS. #3 waits on TASK-395.3.

AC #3 (@claude, 2026-11-13; 218be7c5a): collector::tests::clones_of_shared_points_in_other_tenants_keep_what_they_inherit_by_references: a shared point cloned into two cells of tenant 2 writes each reference under the origin's cell (tenant 1); point and origin deleted, two sweeps, both clones read what they inherited; clones deleted, a sweep retires both references and testing::uncollected finds nothing. Also restore_faults now unexcused, and clones write their reference after their record (TASK-395.10).

Merged with TASK-395.5/.6/.8 on t3/395-integrated (cherry-picked unchanged; conflicts only in collector tests and docs). Verified on t3/395-integrated (macOS): cargo clippy --workspace --tests (nothing new); cargo nextest -p extent_remote 296/296 (with the 2 new tests); scripts/test.sh: clippy gate, workspace 2352/2352, sim 61/61 (deletions_need_fences re-seeded, see TASK-403), Shuttle 37+10+6+33+10+18, scripts 313 OK, mutants 0 broken, go ok; Linux zig cross-check clean; TLA+ specs/bucket models but BucketForks/Overlap/Strays/Takeovers hold (16) and all 72 mutations fail as expected. Flake TASK-404 filed (commits_reopen_from_their_logs under load, unrelated). Not run: ubu1.
<!-- SECTION:NOTES:END -->
