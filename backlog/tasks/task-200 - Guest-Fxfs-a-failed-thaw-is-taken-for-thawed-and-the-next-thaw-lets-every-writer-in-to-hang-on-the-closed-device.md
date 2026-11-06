---
id: TASK-200
title: >-
  Guest Fxfs: a failed thaw is taken for thawed, and the next thaw lets every
  writer in to hang on the closed device
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 06:50'
updated_date: '2026-11-06 09:40'
labels:
  - guest
  - fxfs
  - freeze
dependencies: []
references:
  - guest/linux/fs/fxfs/super.c
  - guest/rust/fxfs-linux/src/fs.rs
priority: high
type: bug
ordinal: 646000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the 2026-11-06 review of the guest's Fxfs (review-guest).

`fxfs_vfs_unfreeze_fs` (guest/linux/fs/fxfs/super.c:2462) decrements `volumes_frozen`, calls `fxfs_dax_thaw` and sets `filesystem->frozen = false` even when `fxfs_thaw` fails. The VFS does the opposite: when `unfreeze_fs` returns an error, `thaw_super_locked` leaves the superblock frozen (SB_FREEZE_COMPLETE) and restores the freeze holder's count. So C and the VFS now disagree about whether the filesystem is frozen.

Meanwhile Rust's `Filesystem::thaw` (guest/rust/fxfs-linux/src/fs.rs:282) returns early when `Freezer::unfreeze` fails. That happens if T_FREEZE(0) fails or if the INFO after it fails; INFO is `store.info_async()` on the host, which fails while extentd is unreachable, for example while it restarts as a restored VM thaws. In that case the device's gate stays closed, the task holding the commits (`self.frozen`) is still held, and compactions stay paused.

Retrying the thaw (`fsfreeze -u`, FITHAW, emergency thaw) decrements `volumes_frozen` again. With one volume it wraps to UINT_MAX. `filesystem->frozen` is false, so `fxfs_thaw` isn't called and the retry returns 0. The VFS thaws, and every writer then blocks for ever in D state: data writes wait at `Gate::pass`, and every transaction waits at `lock_commits`. sync, fsync and unmount hang too. The only way out is a reboot, with whatever wasn't durable lost. With the filesystem left frozen, the same retry would have called `fxfs_thaw` again and recovered once the daemon came back.

A failure after the gate opens, in `renew_journal_tail` or `record_journal_tail`, also returns an error while Rust is already thawed. Any fix has to settle which state an error means. Suggested: `fxfs_thaw` failing means "still frozen, retry", with the steps tracked (frozen with commits held, then journal tail renewed, then recorded) so that a retry resumes where the last attempt stopped and never repeats `unfreeze`/`open`. C keeps `frozen`, `volumes_frozen` and the DAX quiesced on error.

No test can reach this: guest/sim doesn't build super.c, fxfs-linux has no host tests, and no guest check makes a thaw fail. Failing guest check, using a new fault-injection parameter `fxfs.fail_thaws=N` (the next N `fxfs_thaw`s fail before `unfreeze`, as `dax_fail_maps` fails maps). Guest tests are TASK-44.15's area. On main it hangs at the write, and the agent's budget kills it:
```rust
/// A thaw that failed leaves the filesystem frozen, and the next thaw thaws it.
pub(super) fn test_failed_thaw_thaws_on_retry() {
    let dir = fs::File::open(MNT).unwrap();
    assert_eq!(unsafe { libc::ioctl(dir.as_raw_fd(), FIFREEZE as _, 0) }, 0);
    let _fail = Param::set("fail_thaws", "1");
    let ret = unsafe { libc::ioctl(dir.as_raw_fd(), FITHAW as _, 0) };
    assert_eq!((ret, std::io::Error::last_os_error().raw_os_error()), (-1, Some(libc::EIO)));
    assert_eq!(unsafe { libc::ioctl(dir.as_raw_fd(), FITHAW as _, 0) }, 0);
    fs::write(path("after-thaw"), b"x").unwrap();           // main: hangs here
    fs::File::open(path("after-thaw")).unwrap().sync_all().unwrap();
}
```
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 fxfs_thaw failing leaves the filesystem frozen in C as in the VFS: frozen, volumes_frozen and the DAX unchanged
- [x] #2 A retried thaw after a failed one calls fxfs_thaw again, which resumes from where the failed one stopped, and the filesystem then takes writes and syncs
- [x] #3 volumes_frozen never wraps: a WARN if it would
- [x] #4 A guest check with a fault injection failing one thaw, failing on main
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Rust Filesystem::thaw as steps (Frozen: Closed, Opened, Renewed), each taken once; a failed step leaves the state for the next thaw.
2. C: fxfs_vfs_filesystem_thaw leaves frozen, volumes_frozen and the DAX on error; per-superblock frozen flag (fxfs_vfs_info::frozen) so the count never wraps (WARN).
3. fxfs.fail_thaw_at=STEP fault injection; guest check failed_thaw_thaws_on_retry (steps 1 and 2, device's freeze requests counted).
4. Verify as TASK-199, bug back.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done on guest-199, commit aa7073516 (with TASK-202, which shares the freeze code).

Fix: C's fxfs_vfs_filesystem_thaw leaves frozen, volumes_frozen and the DAX quiesced when fxfs_thaw fails, as the VFS keeps the superblock frozen; the next FITHAW calls fxfs_thaw again. Each superblock records whether its filesystem counts it frozen (fxfs_vfs_info::frozen), so volumes_frozen is a count of flags and can't wrap (WARN_ON_ONCE if a thaw finds it 0 or the volume not counted). Rust's Filesystem::thaw is steps (Frozen::Closed: tell the device and take its branch, open it; Opened: renew the journal's tail, then let commits go; Renewed: record the tail; then resume compactions), each taken once: a failure keeps the state at that step, commits held and compactions paused as long as the journal's tail isn't renewed and recorded. A failure after the gate opened (renewing or recording) means still frozen too. fxfs.fail_thaw_at=STEP (1, 2, 3) fails the next thaw before that step, then reads 0.

Blind spot: nothing failed a thaw: guest/sim doesn't build super.c, fxfs-linux has no host tests, and the snapshot suites thaw on a reachable daemon. Check added: failed_thaw_thaws_on_retry, for steps 1 and 2: freeze (device told once), a thaw made to fail (EIO), the device told of the thaw only if the failed step came after telling it, a writer waits while it is still frozen, the retry thaws, the writer writes, fsync, and the device was told of the thaw once in all (no step repeated). Bug back (GCE r763, the old C thaw, injection kept): the check's processes stayed in D state past their kill ('its processes still running 10s after they were killed'), as the task described.

Look around: the leave path, which froze the filesystem as its last unfrozen volume left, now never freezes one frozen already, and thaws a frozen filesystem whose last volume leaves before closing it (only reachable after an emergency thaw of read-only superblocks, TASK-202's limit). Step 3 (recording) is reachable only after a restore whose branch is new; it isn't injected by a check (fxfs-guest-snapshot's restores take it, passing). Structure: the class (C and Rust disagreeing on frozen) is closed by C deciding nothing on failure and Rust keeping its own step state; noted in OPEN_QUESTIONS.md.

Verified: macOS workspace green; Linux Docker (privileged) 2221/2225, the 4 Docker-only failures known on main; check-guest.sh arm64 ok; upstream-changes.py --check ok; GCE us-west1-b jailed, r762: workspace 2225/2225, fxfs-guest-checks, -checks-dax, -checks-dax-small, fxfs-guest, -dax, -snapshot-dax, -suspend-dax, -migrate-dax, -stats, -follow-remount, -suspend-read-only-volume, -suspend-remounted-read-only all pass.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A failed thaw leaves the filesystem frozen in C as in the VFS; Rust's thaw resumes from the failed step, never repeating one; volumes_frozen is per-superblock flags and can't wrap. Verified by failed_thaw_thaws_on_retry (steps 1 and 2), red with the bug back (GCE r763: hung writers), green on r762, plus the workspace and snapshot suites.
<!-- SECTION:FINAL_SUMMARY:END -->
