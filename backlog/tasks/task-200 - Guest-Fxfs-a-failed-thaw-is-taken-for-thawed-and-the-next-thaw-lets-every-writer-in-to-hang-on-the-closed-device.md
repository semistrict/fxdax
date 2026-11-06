---
id: TASK-200
title: >-
  Guest Fxfs: a failed thaw is taken for thawed, and the next thaw lets every
  writer in to hang on the closed device
status: To Do
assignee: []
created_date: '2026-11-06 06:50'
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
- [ ] #1 fxfs_thaw failing leaves the filesystem frozen in C as in the VFS: frozen, volumes_frozen and the DAX unchanged
- [ ] #2 A retried thaw after a failed one calls fxfs_thaw again, which resumes from where the failed one stopped, and the filesystem then takes writes and syncs
- [ ] #3 volumes_frozen never wraps: a WARN if it would
- [ ] #4 A guest check with a fault injection failing one thaw, failing on main
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
