---
id: TASK-199
title: >-
  Guest Fxfs: xattr and ACL changes are lost behind fsync's 0 after a failed
  write-back, and pending changes grow without bound
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 06:47'
updated_date: '2026-11-06 09:41'
labels:
  - guest
  - fxfs
  - durability
dependencies: []
references:
  - guest/linux/fs/fxfs/super.c
  - crates/fxfs-posix/src/lib.rs
priority: high
type: bug
ordinal: 645000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the 2026-11-06 review of the guest's Fxfs (review-guest). Two failures of one structure: an extended attribute set through the guest VFS (setxattr, setfacl, `security.capability`) lives only in memory (`Fxfs::xattrs`, crates/fxfs-posix/src/lib.rs `change_xattr`) until the inode's `write_inode` writes it back (`write_xattrs`). `set_xattr` checks nothing but the name's and value's lengths, and takes no room.

**1. A change whose write-back failed is lost while fsync returns 0.** `write_xattrs` runs in a transaction with reserved room (`Op::WriteXattrs` is `Room::Reserved`; `set_extended_attribute` uses `Options::default()`), so on a full filesystem it fails NoSpace (or EIO on a failing device). When the flusher is the one to call `fxfs_vfs_write_inode` (super.c:2389), the error goes nowhere: `__writeback_single_inode` clears the inode's dirty bits before `write_inode` and `writeback_sb_inodes` ignores the result (fs/fs-writeback.c, 6.18). The inode is now clean. A later `fsync` runs `sync_inode_metadata`, which finds nothing dirty, then `fxfs_fsync`, and returns 0. The change stays pending in memory, so getxattr shows it. It is dropped when the inode is evicted (unmount, reboot, reclaim): fxfs-posix `evict` logs "inode N evicted with extended attributes not written back". For ACLs this means a permission someone took away comes back after a remount. The same holds for chmod, chown and utimes when `setattr` fails with EIO: the in-memory inode keeps the new mode, Fxfs keeps the old one, and fsync returns 0.

Sequence, as an unprivileged user on a writable file:
1. Fill the filesystem until a write gets ENOSPC.
2. `setxattr(f, "user.x", "1")` returns 0.
3. Wait more than 30 s, so the flusher writes the inode back and gets ENOSPC, or call `fsync(f)` twice: the first returns ENOSPC, the second returns 0.
4. `fsync(f)` returns 0.
5. umount and mount: `user.x` is gone.

**2. Nothing bounds the pending changes.** Each setxattr of a new name adds up to 64 KiB (XATTR_SIZE_MAX) to the map, with no I/O. Writeback only touches inodes dirty for 30 s (dirty_expire), and on a full filesystem it never frees them (see 1). Rust's allocations are `kvmalloc(GFP_KERNEL | __GFP_NOFAIL)` (fs/fxfs/rt.c), are not charged to any memcg, and are not throttled the way `balance_dirty_pages` throttles dirty pages. One unprivileged loop of setxattr calls with distinct names (roughly 100k/s × 64 KiB) pins guest kernel memory faster than the flusher frees it. The OOM killer can't free that memory, so a sandbox's guest wedges. ext4 and tmpfs have neither failure: ext4 writes xattrs in the caller's transaction and returns ENOSPC there, and tmpfs charges them to the caller's memcg (GFP_KERNEL_ACCOUNT) and caps user xattrs.

Not covered by any test: guest/sim doesn't build super.c, and the model tests never see the VFS's writeback.

Failing guest check, for the guest checks' filesystem suite (guest tests are TASK-44.15's to change). It is written for the design TASK-80 already uses for dirty pages: `set_xattr` reserves room for its write-back and fails NoSpace when it can't. It fails on main at its first assertion. If the fix chooses another design (for example, every fsync reporting the failure until the change is durable), rewrite it to that design's exact expectations, still unconditional:
```rust
/// A full filesystem refuses an xattr it couldn't write back, as ext4 does, rather than lose it behind fsync's 0.
pub(super) fn test_xattr_on_full_filesystem_is_refused() {
    let f = path("xattr-full");
    fs::write(&f, b"x").unwrap();
    fill_until_enospc(path("filler"));
    let set = xattr::set(&f, "user.x", b"1");
    assert_eq!(set.unwrap_err().raw_os_error(), Some(libc::ENOSPC)); // main: Ok(())
    fs::remove_file(path("filler")).unwrap();
    xattr::set(&f, "user.x", b"1").unwrap();
    fs::File::open(&f).unwrap().sync_all().unwrap();
    remount();
    assert_eq!(xattr::get(&f, "user.x").unwrap().as_deref(), Some(&b"1"[..]));
}
```
The bound in (2) needs a check of its own: a setxattr loop over distinct names, 64 KiB each, totalling twice the guest's memory, which must finish (written back, or refused at the bound) without the OOM killer running (`/proc/vmstat` oom_kill unchanged).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 An xattr or ACL change the filesystem has no room for is refused by setxattr (ENOSPC), or reported by every fsync until it is durable: none returns 0 while the change would be lost at eviction
- [x] #2 A failed write_inode (attributes or xattrs), including the flusher's, is reported by the next fsync of the file (or the inode stays dirty so that fsync retries it)
- [x] #3 Pending xattr changes are bounded, in bytes per volume, so an unprivileged setxattr loop can't pin unbounded kernel memory; at the bound, setxattr writes back or fails
- [x] #4 A guest check covering both, failing on main
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. fxfs-posix: set_xattr and remove_xattr commit at once (Op::SetXattr Reserved, Op::RemoveXattr Borrowed); the pending map, its writeback and evict's log go; write_inode is setattr; create writes large inherited xattrs in its transaction (fxfs set_extended_attribute_impl public, remove_extended_attribute_with_options).
2. guest: fxfs_vfs_write_inode marks a failed inode dirty again (fsync retries and reports); fxfs.fail_write_inode=<ino> fault injection.
3. Tests: fxfs-posix xattrs_are_made_at_once, a_full_filesystem_refuses_xattrs_and_removes_them, the op table; model_tests sets refused when full; guest checks xattr_on_full_filesystem_is_refused, xattrs_bounded, failed_write_inode_reported.
4. Verify: macOS, Linux Docker, check-guest, GCE jailed; a bug-back run showing the checks red.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done on guest-199, commit e33ebef98 (tree verified as aa7073516).

Design (OPEN_QUESTIONS.md, Decisions): extended attributes are written through, each set or removal a transaction of its own (Op::SetXattr Room::Reserved: NoSpace, so setxattr ENOSPC, on a filesystem without room; Op::RemoveXattr Room::Borrowed, as what frees). The pending map (Fxfs::xattrs), its writeback in write_inode and evict's 'not written back' log are gone; write_inode is setattr (fxfs_write_inode calls it; the unused fxfs_setattr FFI went). A new object's inherited ACLs too large to keep inline are written in its creation's transaction (fxfs set_extended_attribute_impl made public; remove_extended_attribute_with_options added: upstream deviations, documented in store_object_handle.rs's header). The guest's write_inode marks a failed inode dirty again (not one being freed), so the next writeback retries it and every fsync reports the failure until it is durable. fxfs.fail_write_inode=INO fails an inode's next writeback (fault injection). The task's suggested design (reserve room for the pending change, plus a byte bound, plus fsync reporting) was rejected: three mechanisms for what one transaction does. Cost: a setxattr is now a transaction, not a map insert.

Blind spot: no test saw the VFS's writeback fail: guest/sim doesn't build super.c, and fxfs-posix's model tests called write_inode themselves and dropped what it failed to write, as the model did, accepting the loss. Tests added: fxfs-posix xattrs_are_made_at_once (set, replace, remove, inherited large ACL; closed with no write_inode and reopened) and a_full_filesystem_refuses_xattrs_and_removes_them (small sets until NoSpace, the refused one absent, a large one removed on the full filesystem); the op table's every_change_fails_once_a_batch_failed and each_operation_comes_after_the_unlinks now cover SetXattr and RemoveXattr; model_tests take a set refused NoSpace as unchanged. Guest checks: failed_write_inode_reported (chmod, sync's writeback failed by injection, fsync, remount: mode kept; an fsync's own failure returns EIO), xattr_on_full_filesystem_is_refused (fill, small setxattrs until ENOSPC, the refused one ENODATA, every accepted one read back after a remount), xattrs_bounded (64,000-byte xattrs of distinct names until twice MemTotal or ENOSPC: oom_kill unchanged, refused once full).

Bug back (GCE r763, old fxfs-posix and write_inode, injections and checks kept): failed_write_inode_reported red (mode 0644 after remount, not 0600); xattr_on_full_filesystem_is_refused red ('a full filesystem took 100,000 extended attributes'); xattrs_bounded: the guest OOM-killed everything and panicked ('System is deadlocked on memory'), the log saying 'inode 4 evicted with extended attributes not written back'.

Look around: setattr's failures in the flusher (chmod, chown, utimes with EIO) were the same blind spot, fixed by the same re-dirtying, which failed_write_inode_reported covers. fileattr_set and the resize path write attributes synchronously and return their errors already. Structure: the class (in-memory state outliving the failure of its writeback) is gone for xattrs; for the kernel's own inode attributes the VFS keeps the state, and re-dirtying keeps it until durable. Coverage not rerun.

Verified: macOS workspace (scripts/test.sh workspace) 2032/2033 then the one failing test (my first version of the full-filesystem unit test, assuming a small inline xattr couldn't fit) fixed and passing; Linux Docker (privileged) 2221/2225, the 4 Docker-only failures known on main (build stamp, three socket grants); guest/tests harness 135/135; check-guest.sh arm64 ok; upstream-changes.py --check ok; GCE (us-west1-b, jailed): r762 workspace 2225/2225 (run d), fxfs-guest-checks, -checks-dax, -checks-dax-small all pass (run c), fxfs-guest, -dax, -snapshot-dax, -suspend-dax, -migrate-dax, -stats, -follow-remount pass (run a).

Decision moved here from OPEN_QUESTIONS.md (2026-11-06):
## Extended attributes are written through, each set in a transaction of its own; write_inode keeps a failed inode dirty (TASK-199, 2026-11-06)

An xattr or ACL change lived in fxfs-posix's memory (`Fxfs::xattrs`) until the kernel wrote the inode back: a writeback that failed (a full filesystem's NoSpace, a device's EIO), as the flusher's or sync's that nothing reports, left the inode clean, the next fsync returned 0, and eviction dropped the change; and the map grew without bound. Decided: **`set_xattr` and `remove_xattr` commit at once**, as ext4's setxattr journals its change: a set's transaction reserves its room (`Op::SetXattr`, `Room::Reserved`) and fails NoSpace on a full filesystem, which setxattr returns (ENOSPC); a removal borrows its room (`Op::RemoveXattr`, `Room::Borrowed`), as what frees does. Nothing is pending, so nothing is unbounded and nothing is lost at eviction; `write_inode` writes the kernel's attributes alone (fxfs_write_inode is `setattr`). A new object's inherited ACLs too large to keep inline are written in its creation's transaction (fxfs's `set_extended_attribute_impl`, public), where they were pending. **The guest's write_inode marks a failed inode dirty again**, so the next writeback retries it and every fsync reports the failure until it is durable. A set or removal is refused once a batch of queued unlinks failed, as every change is (the TASK-160.23 entry no longer lists them as not refused). *Instead:* the task's suggestion, keeping the pending map with room reserved as each change is made (as TASK-80 reserves dirty pages') and a byte bound per volume, plus fsync reporting failed writebacks: three mechanisms for what one transaction does. Cost: a setxattr is a transaction (and, for a value over 256 bytes, a write of its blocks), not a map insert; fs-bench's setxattr measured the map. *Undoing it:* the map, its writeback in `write_inode`, and a reservation and bound for it.

A failed batch leaves Fxfs maybe holding names the kernel took for gone; changes then failed with EIO until a remount, but only those through `transaction_with`. Clone (`clone_range`), writes (`write`, `write_many`), the page cache's writeback (`write_dirty`), `allocate`, truncates (shrinking or not) and the writeback of extended attributes made their transactions elsewhere and went on. Decided: each operation's row says whether it changes what the filesystem holds (`Op::rules`' `changes`), and every one that does fails: those seven too. The filesystem fails stop, as ext4 does once its journal aborts, by one rule rather than a list of exceptions. A writeback refused leaves the room its dirty data reserved as one that fails does: the page cache's pages are no longer dirty either way. Not refused: lookups, readdir, reads, `sync`, `fsync`, `statfs` (making what's there durable, or reading it), `evict` (frees nothing once a batch failed, as before), and the queue's own batches, which bring Fxfs back to the kernel's view.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
xattrs and ACLs are written through (setxattr ENOSPC on a full filesystem, nothing pending, nothing unbounded); write_inode re-dirties a failed inode so fsync reports it until durable. Verified by fxfs-posix unit tests, three guest checks red with the bug back (GCE r763) and green with the fix (r762), macOS, Linux Docker and GCE workspace.
<!-- SECTION:FINAL_SUMMARY:END -->
