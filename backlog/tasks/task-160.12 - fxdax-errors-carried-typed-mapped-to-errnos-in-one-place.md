---
id: TASK-160.12
title: 'fxdax: errors carried typed, mapped to errnos in one place'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-03 13:08'
updated_date: '2026-11-03 13:42'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 583000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Failure::Other(String) turns errors into text; extent_remote::Error::Displaced becomes EIO though the docs treat it as losing the branch (EROFS); two downcast lists, any new error type quietly EIO. Explains 894b4c1a1, 0c7e7c8cc. Proposal: Failure::Remote(extent_remote::Error) and one errno mapping with a test per variant. See also the Fxfs errno table task.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One errno mapping for the host, tested per variant
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Verify: Failure::Other(String), Displaced -> EIO, the downcast lists.
2. Failure::Remote(extent_remote::Error), PointDeleted, Local(Arc<anyhow::Error>); Failure::errno exhaustive over extent_remote::Error; callers (remote_errno, extentd attach, host_store delete) use it.
3. Merge extents.rs's errno and outside_view into one store list (NotHeld -> EIO logged).
4. Tests per variant; scripts/test.sh, GCE.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Claims checked: Failure::Other(String) carried 30-odd errors as text (io errors, runtime, JoinError, extent_remote's Displaced/NoBranch/Point/Lost/Corrupt/Read); remote_errno mapped Other to EIO, so Displaced (which stops the writer for good, as Fenced) was EIO, not EROFS. extentd's attach mapped only Stale/PointDeleted; host_store's delete only Stale. The two downcast lists were extents.rs's errno and outside_view (a follower's: everything but CutOff and NoExtent EIO).
Now: Failure keeps extent_remote::Error as is and local errors as anyhow (Failure::local adds context, no format!), with PartialEq by value / by message. Failure::errno is exhaustive over extent_remote::Error: Fenced/Displaced/Deleted/Point EROFS, Stale ESTALE (only arises taking a branch: attach, delete), NoBranch/PointDeleted ENOENT, ChildTaken EEXIST, Lost/Corrupt/Read/Local None (caller's EIO, logged; doc-4 has no other status). The store list is one; it stays a downcast list because the types are storage_device's/extent_store's, shared with Fxfs and the guest; each_store_error_has_its_errno lists every type. Tests: each_uploader_failure_has_its_errno (red with Displaced back to EIO), each_store_error_has_its_errno.
<!-- SECTION:NOTES:END -->
