---
id: TASK-160.10
title: 'extentd: access decided once, at open; two suspected holes'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-03 13:08'
updated_date: '2026-11-06 06:56'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 581000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The default tenant is None, special-cased in four places; whether a store is a point, public base or follower is worked out by each caller. Suspected, unconfirmed: a control ResizeStore can change a point's capacity locally (ExtentStore::writable checks only followers; is_point is true only once an uploader is attached; stores::resize opens without a profile); and public_branches treats a read_dir error as 'no public branches', failing open, the base opened private and writable (extents.rs). Explains TASK-79.17, 150.4, 43.7, 154. Proposal: a real default Tenant with a policy, an Access fixed at open and enforced in ExtentStore's mutating methods, and reading the public markers failing the open.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Red tests: ResizeStore on a point gets EROFS; a failing read_dir fails the open
- [x] #2 No tenant.is_some() left
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Red tests: grants.rs points_are_resized_nowhere (ResizeStore of a point, closed and kept open, on two hosts); extents.rs stores_whose_public_markers_go_unread_fail_to_open (failpoint extents::public_branches, EMFILE).
2. Refactor: a real default Tenant with a Policy (stores: Dir|Roots, names_branches, follows_public_bases) in place of Option<Arc<Tenant>> at memory(), authorize, follow, open.
3. Refactor: StoreAccess {role: Writer|Follower|Point, public} from ExtentStore::access(), fixed once open and attached; may_write/may_manage checked inside every mutating method; callers (Connection::open/follow, stores::take_over, finish_connection) use it.
4. Fix hole 1: stores::resize opens with its remote attached (open_here); EROFS -> FAILED_PRECONDITION; fxdaxctl local resize attaches too. Class test: remote.rs points_refuse_what_would_change_them.
5. Fix hole 2: public_branches returns Result, EIO logged; every open fails on it.
6. Prove: macOS scripts/test.sh; GCE scripts/test-gce.sh all (workspace + guest snapshots, migration, tenants).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Hole 1 REAL. Red: grants.rs points_are_resized_nowhere failed on GCE before the fix ('the call succeeded: ()': ResizeStore grew a point). Blind spot: a store learns it is a point only once its remote is attached; stores::resize opened it unattached (open_without_profile), and ExtentStore's writable() checked only followers, so nothing refused; point tests covered a VM's open (vm_processes_write_no_point) but no control-plane mutation. Class test: remote.rs points_refuse_what_would_change_them, every mutating method of an attached point (set_capacity, write, free, commit, commit_ordered, writable map, clean_behind, take_over) EROFS, fork and make_public allowed. Look-around: fxdaxctl's local resize had the same hole (fixed: it attaches); stores::make_public and stat open unattached but change no extent (make_public allowed on points by decision); a point open for followers (an instance) was also resizable, now refused by the store itself.
Hole 2 REAL. Red: extents.rs stores_whose_public_markers_go_unread_fail_to_open (failpoint extents::public_branches, EMFILE) got Ok(false) before the fix: the public base opened private. Blind spot: an error swallowed into a default (per-entry errors too). Look-around: other read_dir uses in fxdax/extent_store propagate errors; recorded_bases already fails EIO; profile reads fail open but only cost speed.
Structure: Tenant default is real (Policy: Stores Dir|Roots, names_branches, follows_public_bases); no tenant.is_some() left. StoreAccess {role Writer|Follower|Point, public} from ExtentStore::access(), checked in every mutating method via may_write / may_resize / may_manage. Deviation found by GCE: commands_through_the_daemon grows a public base, so resize got its own check (public bases grow, points don't). writers_take_over_stores_followed now expects EROFS for a follower's take_over (was EINVAL). EROFS maps to FAILED_PRECONDITION. Decisions in OPEN_QUESTIONS.md.
Validation: macOS scripts/test.sh: clippy rules pass; workspace 2040 passed, 8 long tests cut off by an external SIGTERM, rerun and passed; Shuttle suites and scripts tests pass; fxdax package rerun after the resize split 298/298. GCE (fxdax-test-t16010-access): workspace 2213/2214 (the public-base resize, fixed after), guest cases fxfs-guest-snapshot, -snapshot-dax, -snapshot-volumes, -snapshot-volumes-dax, -migrate-dax, -migrate-tenant-dax, -shared-volumes, -shared-volumes-dax all OK. scripts/upstream-changes.py --check passes.

GCE rerun after the resize split (94 tests: fxdaxctl, grants, control, tenants, bases binaries and every resize/capacity/point test): all passed. Host deleted.

Rebased onto main past TASK-153.15 (seals): its ask_to_seal used is_follower() and arena_id() an Option, both folded into the commits that changed them (access().role, ArenaId). Each commit checked to compile on Linux. Rebased tree: GCE -p fxdax 2228/2228; guest fxfs-guest-host-seal, -migrate-tenant-dax, -snapshot-dax, -shared-volumes-dax OK; macOS -p fxdax 299/299. Seen once, not this change's: guest/tests' own script_run::tests::clones_do_as_the_model_says failed on 'umount: target is busy' (exit 32), passing on the next host and the earlier run: an intermittent race in the harness's test.

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**What a store may be asked to do is the store's, fixed as it opens (TASK-160.10, 2026-11-03)**

Two holes were real: ResizeStore grew a point (the daemon opened it without its remote, which alone says it is one), and a failed listing of a store's public markers opened a public base private and writable. Decided: `ExtentStore::access()` (`StoreAccess`: writer, follower or point, and public) is fixed once the store is open and attached, and its methods check it: what changes what it holds (write, free, commit, map writably, clean, take over) needs a writer's store that isn't public (`EROFS` for a follower or point, `EPERM` for a public base); a resize needs a writer's, public or not, as no fork shares a capacity, but a point's forks take its; forking, making public and attaching a base, which change no extent, need any but a follower's, so a point may still be made public or have a base attached. A public base now refuses writes at the store too, not only a VM's open of it. Resizing a closed store attaches its remote, as every other daemon open does, so it takes the branch's lease (`ESTALE` if a later placement holds it). EROFS maps to FAILED_PRECONDITION. A marker listing that fails fails the open with `EIO`, logged. The default tenant is a real `Tenant`, nameless and unregistered for limits, with a policy of its own (roots, names branches, follows public bases).
**Instead:** keep each caller's checks and fix the two holes alone; or record point-ness locally, which would be a second copy of what the bucket's record says.
**Undoing it:** `StoreAccess::may_write` lets public bases through; `stores::resize` back to `open_without_profile`.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Both suspected holes were real and are fixed with red-first tests: ResizeStore grew a point (the daemon opened it without its remote; now every daemon open attaches and set_capacity refuses points, EROFS/FAILED_PRECONDITION; fxdaxctl's local resize too), and a failed listing of public markers opened a public base private and writable (now the open fails, EIO). Structure: a real default Tenant with an explicit Policy replaces Option<Tenant> (no tenant.is_some() left); StoreAccess (writer/follower/point, public) fixed at open and checked inside ExtentStore's mutating methods replaces callers' checks. Verified on macOS (scripts/test.sh) and GCE (workspace, guest snapshot/migrate/tenant/shared-volume cases, rerun of affected tests); upstream-changes --check passes. Decisions in OPEN_QUESTIONS.md.
<!-- SECTION:FINAL_SUMMARY:END -->
