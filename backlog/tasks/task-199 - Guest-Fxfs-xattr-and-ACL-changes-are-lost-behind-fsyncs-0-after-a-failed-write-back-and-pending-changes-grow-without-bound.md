---
id: TASK-199
title: >-
  Guest Fxfs: xattr and ACL changes are lost behind fsync's 0 after a failed
  write-back, and pending changes grow without bound
status: To Do
assignee: []
created_date: '2026-11-06 06:47'
updated_date: '2026-11-06 06:48'
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
- [ ] #1 An xattr or ACL change the filesystem has no room for is refused by setxattr (ENOSPC), or reported by every fsync until it is durable: none returns 0 while the change would be lost at eviction
- [ ] #2 A failed write_inode (attributes or xattrs), including the flusher's, is reported by the next fsync of the file (or the inode stays dirty so that fsync retries it)
- [ ] #3 Pending xattr changes are bounded, in bytes per volume, so an unprivileged setxattr loop can't pin unbounded kernel memory; at the bound, setxattr writes back or fails
- [ ] #4 A guest check covering both, failing on main
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
