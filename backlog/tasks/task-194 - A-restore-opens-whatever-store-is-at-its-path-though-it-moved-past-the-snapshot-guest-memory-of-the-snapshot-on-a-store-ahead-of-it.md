---
id: TASK-194
title: >-
  A restore opens whatever store is at its path, though it moved past the
  snapshot: guest memory of the snapshot on a store ahead of it
status: To Do
assignee: []
created_date: '2026-11-06 06:43'
updated_date: '2026-11-06 06:52'
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
- [ ] #1 A restore into a path whose store has commits since the snapshot's point fails, with an errno the integrator guide documents, and changes nothing there
- [ ] #2 A restore into the store a snapshot was taken of, unwritten since, and a retried restore over a fork it left unwritten, still succeed
- [ ] #3 restores_refuse_a_store_written_since_its_point passes, un-ignored
- [ ] #4 docs/integrator-guide.md says what path_on_host a restore may name
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
