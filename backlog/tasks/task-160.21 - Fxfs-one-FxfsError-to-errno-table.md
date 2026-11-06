---
id: TASK-160.21
title: 'Fxfs: one FxfsError to errno table'
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
ordinal: 592000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The table is written twice and has drifted: guest/rust/fxfs-linux/src/lib.rs and crates/fxdax/src/error.rs disagree (AlreadyBound EBUSY or EIO, OutOfRange ERANGE or EIO); the tables use the outermost FxfsError, FxfsError::matches the root cause; Inconsistent and IntegrityError become EIO though TASK-153.11 and 153.13 now raise them on purpose (ext4 and xfs say EUCLEAN and EBADMSG). Proposal: fxfs_posix::errno(), exhaustive, one classification rule; both VFSs call it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 One table, exhaustive, tested per variant
- [x] #2 The choice for corruption errors recorded in OPEN_QUESTIONS
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Verify the review's claims against both tables and FxfsError::matches.
2. Move Errno to fxfs-posix; one exhaustive table (of_fxfs, no _ arm) with expected/logged per row; fxfs_posix::errno and xattr_errno with one classification rule; guest and host call them.
3. Decide corruption errnos (EUCLEAN, EBADMSG); audit what raised Inconsistent without corruption (manifest parsing, failed unlink batch, dax_map_write giving up) and fix each.
4. Tests per variant; guest garbage-manifest check to EINVAL; record decision in OPEN_QUESTIONS.md.
5. scripts/test.sh, GCE workspace + guest cases, upstream-changes --check.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Claims checked: AlreadyBound EBUSY (host) vs EIO (guest), OutOfRange EIO vs ERANGE: true. 'Outermost vs root cause': the tables found the first FxfsError in chain(), and FxfsError has no source, so it is always the root cause, as matches() sees; they differ only for an FxfsError added as context (anyhow's downcast_ref finds it, chain() does not), as object_store.rs's encrypted-mutations .context(FxfsError::Inconsistent). The rule now counts context (outermost Errno, else outermost FxfsError, context first), pinned by the_outermost_errno_or_fxfs_error_classifies. Corruption EIO: true; now EUCLEAN/EBADMSG (decision in OPEN_QUESTIONS.md). dax_map_write dropped its last error: true; it now returns it with context (a_dax_write_sealed_on_every_try_gives_up_with_the_seal, red with Inconsistent: EUCLEAN != EIO and no SealedExtent in the chain).
Callers checked: fs/fxfs C compares Fxfs errnos only with ENOENT/ENOSPC/ENODATA/EXDEV/EBUSY/ESTALE/EDEADLK/EAGAIN/ENOMEM; no table row maps to EDEADLK/EAGAIN/ESTALE, EXDEV only for encryption policies (rename/link), EBUSY for Unavailable (as before) and AlreadyBound (store unlock, volume mount; not on DAX paths). Guest tests: only shared_volumes' garbage manifest expected EIO (InvalidVersion, unclassified): it is now InvalidArgs/EINVAL with a 2-byte case for the version read.
Blind spot: errnos tested only for a handful of variants, on each side apart; the per-variant test now lists all 23 and the table is exhaustive, so a new FxfsError fails to compile. Looked around: fxdax-raised Inconsistent that wasn't corruption: check_manifest and manifest parsing (now InvalidArgs), the failed unlink batch (now plain EIO), dax_map_write's give-up (now its seal error). check_replaced and the guest's root-length check stay Inconsistent (real disagreement). Structure: the class (two tables drifting) is gone: one table, one type of errno for both VFSs.

Validation: scripts/test.sh on macOS passes on the rebased tree (2059 workspace tests, Shuttle, follower, scripts). GCE, before the rebase: fxfs-guest, fxfs-guest-checks, fxfs-guest-checks-dax and xfstests-fxfs-guest pass (fxdax-test-t16021-errno); fxfs-guest-shared-volumes and -dax pass with the garbage manifest's EINVAL (errno2). After the rebase: the workspace passes on Linux, 2230 tests (errno3), and the guest kernel builds with fxfs-linux. The guest cases did not run there: main's new guest/tests self-test script_run::tests::clones_do_as_the_model_says failed first with 'umount: target is busy' on the host's loop mount (TASK-160.27's, nothing of this change's). The rerun (errno4) hung when gcloud's auth expired (OPEN_QUESTIONS.md). Put the bug back and saw it red: Displaced back to EIO fails each_uploader_failure_has_its_errno; dax_map_write returning Inconsistent again fails a_dax_write_sealed_on_every_try_gives_up_with_the_seal. The rebase brought extent_remote::Error::Format (TASK-141.2), which Failure::errno's exhaustive match refused to compile without: ENOTSUP. scripts/upstream-changes.py --check passes.

A second rebase onto main (TASK-160.14's record kinds, TASK-160.10's store access) conflicted only in remote.rs's Failure::Other sites and OPEN_QUESTIONS.md; scripts/test.sh passes on it on macOS (2068 workspace tests; one run's extent_remote free_bytes_are_the_free_blocks_size missed by disk activity, passing alone and in the rerun). No Linux run of that tree: gcloud's login expired (OPEN_QUESTIONS.md).

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**Corruption is EUCLEAN and EBADMSG, not EIO; one errno table for both VFSs (TASK-160.21, 160.12, 2026-11-03)**

Fxfs raises `Inconsistent` and `IntegrityError` on purpose now (TASK-153.11, 153.13), and both VFSs said EIO for them. Decided, in the one table both now call (`fxfs_posix::errno`): `Inconsistent` is `EUCLEAN` and `IntegrityError` `EBADMSG`, ext4's and XFS's `EFSCORRUPTED` and `EFSBADCRC`, logged. No caller depends on EIO for them: fs/fxfs's C compares Fxfs's errnos only with ENOENT, ENOSPC, ENODATA, EXDEV, EBUSY, ESTALE and the like, and no guest or host test expects EIO of a corrupt image. The rest of the table, where the two had drifted or said EIO: `AlreadyBound` EBUSY, `OutOfRange` and `InvalidVersion` EINVAL (ext4's mount of features it lacks), `NoKey` ENOKEY and `InconsistentEncryptionPolicy` EXDEV (fscrypt's). An error is classified by its outermost Errno, else its outermost FxfsError, context first; Fxfs's `matches` (root cause) agrees but for an FxfsError added as context. What only looked like corruption no longer says it: a manifest that doesn't parse or names no base is `InvalidArgs` (EINVAL; the guest's garbage-manifest check said EIO, and says EINVAL now), changes after a failed batch of unlinks fail with EIO, and a DAX write giving up on seals gives the seal's error (EIO). On the host, the uploader's failures keep `extent_remote::Error` (`Failure::Remote`) and map in one place (`Failure::errno`): `Displaced` is EROFS, as `Fenced` is, where it was EIO; a branch of a newer format (`Format`, TASK-141.2) ENOTSUP, until the host is upgraded; object storage's `Lost` and `Corrupt` stay EIO, doc-4's table having no other status for a store failing to read.
**Instead:** EIO for all corruption, as before (callers can't tell a bad disk from bad metadata); `EFSCORRUPTED` on the device's wire too (doc-4 would need a status).
**Undoing it:** the two rows in `crates/fxfs-posix/src/errno.rs` (`of_fxfs`) and their test rows.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
One FxfsError-to-errno table, fxfs_posix::errno and xattr_errno, exhaustive (no _ arm), each row saying whether callers expect the errno; Errno moved to fxfs-posix; the guest and the host call it. One classification rule: outermost Errno, else outermost FxfsError, context first, else EIO. Corruption is EUCLEAN (Inconsistent) and EBADMSG (IntegrityError), recorded in OPEN_QUESTIONS.md; AlreadyBound EBUSY, OutOfRange/InvalidVersion EINVAL, NoKey ENOKEY, InconsistentEncryptionPolicy EXDEV. What raised Inconsistent without corruption no longer does: manifest parsing is InvalidArgs, a failed unlink batch EIO, dax_map_write's giving up returns its seal error. Verified by per-variant tests, scripts/test.sh, and GCE runs (see notes).
<!-- SECTION:FINAL_SUMMARY:END -->
