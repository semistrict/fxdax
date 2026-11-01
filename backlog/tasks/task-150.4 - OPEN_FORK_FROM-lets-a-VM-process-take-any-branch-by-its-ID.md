---
id: TASK-150.4
title: OPEN_FORK_FROM lets a VM process take any branch by its ID
status: In Progress
assignee: []
created_date: '2026-11-01 16:50'
updated_date: '2026-11-01 19:11'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 519000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed. crates/fxdax/src/extentd/server.rs:1176-1179, :1256-1260; stores.rs:561-572, :664: the paths are authorized, args[1], the branch, is not. A compromised VM process of tenant A, knowing B branch ID, adopts it: its child reads B data, B writer is fenced and its commits fail EROFS. What the check should be may need a decision.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test fails on the code as it stands, or shows the finding false or unreachable, which closes this as not a bug
- [ ] #2 Fixed, the test red again with the fix taken out
- [ ] #3 The notes name the blind spot, the test of its class, and what a search around it found
- [ ] #4 Passes on Linux in cargo nextest run --workspace
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Confirmed: tenants_take_no_branch_by_naming_it. Fixed conservatively: a tenant's named branch is ignored, an absent point gives ENOENT. Side effect for the user to decide: Firecracker always passes point_branch, so a tenant's restore where the point isn't local fails unless its VM manager takes it first. The default tenant can still take any branch (snapshot_points_are_taken_from_the_bucket). Blind spot: the fuzz opened a store first, so OPEN_FORK_FROM was always EBUSY. Class test: fuzz starts unopened, red with the bug back. Structure: branches have no owner: raised with the user. Branch fix/host-audit, 3c90f8f7b, merged locally; workspace on Docker arm64 only, 2070 of 2071 (the Docker-only failure), Shuttle 60 of 60, not yet on GCE.
<!-- SECTION:NOTES:END -->
