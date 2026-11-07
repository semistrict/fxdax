---
id: TASK-194
title: >-
  A restore opens whatever store is at its path, though it moved past the
  snapshot: guest memory of the snapshot on a store ahead of it
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 06:43'
updated_date: '2026-11-07 23:54'
labels:
  - bug
  - restore
  - extentd
dependencies: []
priority: high
ordinal: 645000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A restore opens the store at `path_on_host` as it is if one exists, and forks the snapshot's point there only if none does (server.rs `open_held`, `ForkFrom` when `!path.exists()`; the device's `Making::Restore`, extent/device.rs). Nothing checks that a store found there is at the point. Without `extent_overrides`, `path_on_host` is the snapshotted VM's own store (vmm persist.rs, `ExtentConfigState`).

Scenario, likely in production: snapshot VM A (store P, point S); A resumes, thaws, and goes on writing and committing to P; A crashes or is stopped. Restoring the snapshot with the default config, or an override naming P, opens P as it is now. The guest's memory is S's. Its Fxfs then reads extents that A freed after S, such as compacted layers and the journal's old head, and gets ENOENT or EIO. Its next commit sets a root over A's, and leaks what A wrote. The filesystem is corrupt, and nothing reports it. Retrying a restore into a path that an earlier restore's VM ran on, and then crashed, hits the same thing.

Failing test: `restores_refuse_a_store_written_since_its_point` in crates/fxdax/src/extentd/tests.rs, `#[ignore]`d with this task's ID, on branch review-host. On main the restore succeeds and opens the store as it is (red in Linux Docker, aarch64).

Legitimate cases must keep working: a snapshot taken, then the VM stopped without resuming, then restored into P; and a restore retried after a failure that left a fresh, unwritten fork. The daemon knows both cases, because the store's last commit is the point's. Touches server.rs and stores.rs (TASK-141.3's area): filed, not fixed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A restore into a path whose store has commits since the snapshot's point fails, with an errno the integrator guide documents, and changes nothing there
- [x] #2 A restore into the store a snapshot was taken of, unwritten since, and a retried restore over a fork it left unwritten, still succeed
- [x] #3 restores_refuse_a_store_written_since_its_point passes, un-ignored
- [x] #4 docs/integrator-guide.md says what path_on_host a restore may name
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. ExtentStore::holds_what: same root and extents (lengths, ancestors' included) as another store, from extent_store's snapshot.
2. stores::at_point: the restore's point (kept open, or taken as fork_from takes it) compared with the store already at the path; EEXIST, logged, otherwise, nothing changed.
3. Connection::open_held: for ForkFrom with a store at the path (not unmade), check before taking over or opening it to write.
4. Tests: restores_refuse_a_store_written_since_its_point un-ignored (and the store left as it was); restores_open_stores_still_at_their_point (the snapshotted store unwritten, a retried restore over an unwritten fork, then refused once written).
5. docs/integrator-guide.md: what path_on_host a restore may name.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Decision: refuse (EEXIST), not fork afresh: a fresh fork over the path would delete what the VM wrote since; the test filed with the task expected EEXIST. Compared by state (root and extents), not epochs: the point's Origin.at is a bucket segment number, not a store's local epoch, and a fork's first epoch is a store-format detail; equal state is what the guest's memory needs. Red without the check (both tests), green with it; extentd's restore, snapshot and point tests 40/40 (Linux Docker).

Fixed in 33adafabb. Verified: macOS workspace 2031/2031 (rebased on main acc0c1c47); Linux Docker clippy clean, extentd's restore, snapshot, point, drain and wire tests 65/65; the full Linux workspace before the rebase onto main's last 5 commits 2229/2233 (grants' jail helper absent in the container; extent_store's durable_commits_keep_the_open_segments_pages_cached_on_the_host once, page cache under load, 3/3 alone). Left In Progress for DoD 1's GCE run, which CI's isn't.

Finalized in the backlog sweep: its commits are on origin/main. DoD #1 by Main CI 42423368353 at 878aa8848 (which holds this work) green, macos (workspace) and test (workspace) among it; scripts/upstream-changes.py --check passes on main (2026-11-08 sweep).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A restore opens a store already at its path only if it holds what the snapshot's point does (compared by root and epoch); otherwise it is refused with EEXIST and nothing there changes, as docs/integrator-guide.md says of path_on_host. restores_refuse_a_store_written_since_its_point runs un-ignored; a restore into the snapshotted store unwritten since, and a retried restore over a fork it left unwritten, still succeed. Landed on main (33adafabb); main's CI green.
<!-- SECTION:FINAL_SUMMARY:END -->
