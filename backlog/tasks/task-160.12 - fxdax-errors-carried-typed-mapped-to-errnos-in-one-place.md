---
id: TASK-160.12
title: 'fxdax: errors carried typed, mapped to errnos in one place'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-03 13:08'
updated_date: '2026-11-03 19:43'
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
- [x] #1 One errno mapping for the host, tested per variant
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
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

Validation: scripts/test.sh on macOS passes on the rebased tree (2059 workspace tests, Shuttle, follower, scripts). GCE, before the rebase: fxfs-guest, fxfs-guest-checks, fxfs-guest-checks-dax and xfstests-fxfs-guest pass (fxdax-test-t16021-errno); fxfs-guest-shared-volumes and -dax pass with the garbage manifest's EINVAL (errno2). After the rebase: the workspace passes on Linux, 2230 tests (errno3), and the guest kernel builds with fxfs-linux. The guest cases did not run there: main's new guest/tests self-test script_run::tests::clones_do_as_the_model_says failed first with 'umount: target is busy' on the host's loop mount (TASK-160.27's, nothing of this change's). The rerun (errno4) hung when gcloud's auth expired (OPEN_QUESTIONS.md). Put the bug back and saw it red: Displaced back to EIO fails each_uploader_failure_has_its_errno; dax_map_write returning Inconsistent again fails a_dax_write_sealed_on_every_try_gives_up_with_the_seal. The rebase brought extent_remote::Error::Format (TASK-141.2), which Failure::errno's exhaustive match refused to compile without: ENOTSUP. scripts/upstream-changes.py --check passes.

A second rebase onto main (TASK-160.14's record kinds, TASK-160.10's store access) conflicted only in remote.rs's Failure::Other sites and OPEN_QUESTIONS.md; scripts/test.sh passes on it on macOS (2068 workspace tests; one run's extent_remote free_bytes_are_the_free_blocks_size missed by disk activity, passing alone and in the rerun). No Linux run of that tree: gcloud's login expired (OPEN_QUESTIONS.md).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Failure carries extent_remote::Error (Failure::Remote) and local errors as anyhow (Failure::Local) instead of text; Failure::errno is the one mapping, exhaustive over extent_remote::Error: Displaced now EROFS (was EIO), Format ENOTSUP, ChildTaken EEXIST at attach too; remote_errno, extentd's attach and host_store's delete use it. The extent store's two downcast lists are one (extents::errno), a follower's NotHeld EIO logged. Tests per variant (each_uploader_failure_has_its_errno, each_store_error_has_its_errno); scripts/test.sh and GCE (see notes).
<!-- SECTION:FINAL_SUMMARY:END -->
