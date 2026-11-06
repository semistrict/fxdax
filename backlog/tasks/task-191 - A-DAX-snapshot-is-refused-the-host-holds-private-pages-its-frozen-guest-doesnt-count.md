---
id: TASK-191
title: >-
  A DAX snapshot is refused: the host holds private pages its frozen guest
  doesn't count
status: To Do
assignee: []
created_date: '2026-11-06 03:14'
updated_date: '2026-11-06 04:00'
labels:
  - bug
  - dax
dependencies: []
references:
  - TASK-187
  - TASK-164.1
  - backlog/docs/doc-13 - Copy-on-write-DAX.md
priority: high
type: bug
ordinal: 643000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-92.14's test (2026-11-06, GCE r685 to r688, main at 752515020 with Firecracker e64c4b283). The guest maps a DAX file of 1,024 pages, each an extent of its own (written page by page, last first, each fsync'd, then read in again by a remount), MAP_SHARED with PROT_READ|PROT_WRITE, reads all of it, stores to every eighth page (128), and freezes: FIFREEZE writes them back and rebinds every private page (doc-13). The guest's /proc/fs/fxfs/extents then says dax_private_pages 0 and dax_dirty_pages 0 (128 and 128 before the freeze), yet the daemon refuses the snapshot's point (EBUSY): 'the window has 2 private pages' (snapshot-dax), '1 private pages' (suspend-dax), logged by extentd since TASK-92.14. 10 of 12 DAX rounds failed so in r685 and r686 (fxfs-guest-snapshot-dax, -suspend-dax, -migrate-dax, -migrate-tenant-dax, jailed, n2-standard-4); the rounds that did snapshot restored the stored bytes intact. The VMM's log has ~950 'DAX window offset ... restricted to reads for a worker's write' lines, one per page read through the writable mapping: KVM's async page-fault worker writes. Suspected: a page made private on the host by an access the guest never iomapped as a write (a worker's write taken as a store, or a store after a rebind through a PTE the guest thinks protected), so the guest's count and the host's part. Related: TASK-187 (a kworker's refused access, private and withheld pages, since copy-on-write DAX). Reproduction: TASK-92.14's change to guest/tests/test_cases/src/test_fxfs_guest_snapshot.rs with its stores put back (the guest's map_pieces mapping PROT_READ|PROT_WRITE and storing piece(page, true) to pages where page % 8 == 3; the check after the restore reading pieces_stored()), FXDAX_E2E_TEST=fxfs-guest-snapshot-dax FXDAX_E2E_ROUNDS=4 FXDAX_FIRECRACKER_JAIL=1 scripts/test-gce.sh all.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause named, with the run that shows it
- [ ] #2 fxfs-guest-snapshot's pieces stored to through a writable shared mapping before the freeze (TASK-92.14's reproduction) pass 20 rounds of each DAX snapshot suite, jailed, the restored bytes checked
- [ ] #3 A guest check or unit test that fails with the cause put back
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-06, GCE r689 (us-east1-b, jailed, n2-standard-4; main 752515020 + TASK-92.14): no store is needed. The guest reads its 1,024-page DAX file through a MAP_SHARED, PROT_READ mapping only; its /proc/fs/fxfs/extents says dax_private_pages 0, dax_dirty_pages 0 before and after FIFREEZE; extentd still refuses the point, 'the window has 1 private pages', in round 1 of all four DAX snapshot suites (fxfs-guest-snapshot-dax, -suspend-dax, -migrate-dax, -migrate-tenant-dax), each failing 0 of 3 rounds; fxfs-guest-snapshot (no DAX) passes 3 of 3. So any DAX VM whose guest has read a DAX file through a shared mapping is very likely unsnapshottable on main; main's suites miss it because their only mapped file is created in the same mount, a page-cache file, so the window maps nothing a vCPU reads. In r687 one such round did snapshot and restore, bytes intact, so it is a race. Reproduction: in fxfs-guest-snapshot's guest, before the freeze, write a file page by page (each fsync'd, last first), unmount and mount again, mmap it MAP_SHARED PROT_READ and read it all; FXDAX_E2E_TEST=fxfs-guest-snapshot-dax FXDAX_E2E_ROUNDS=3 FXDAX_FIRECRACKER_JAIL=1 scripts/test-gce.sh all. TASK-92.14's version of the test (write_pieces, map_pieces, the freeze's counts asserted) is the one to land with the fix.

TASK-92.14's guest test, held back for this task (apply to guest/tests/test_cases/src/test_fxfs_guest_snapshot.rs at 05c4b92a0; the version that failed r689 with no store):

```diff
diff --git a/guest/tests/test_cases/src/test_fxfs_guest_snapshot.rs b/guest/tests/test_cases/src/test_fxfs_guest_snapshot.rs
index 4d1ffef23..7ca3f2b5d 100644
--- a/guest/tests/test_cases/src/test_fxfs_guest_snapshot.rs
+++ b/guest/tests/test_cases/src/test_fxfs_guest_snapshot.rs
@@ -29,6 +29,13 @@
 //! through a mapping made before it, which the restored window maps again), writes its own file,
 //! which mints extents of its store's branch, and remounts.
 //!
+//! fxdax TASK-92.14: before the snapshot, the guest maps a file of `PIECES` pages, each an extent
+//! of its own, read in from a mount before, so that the window maps each page apart (with a DAX
+//! window), and reads it; frozen, it holds no private page. Restored, the mapping reads the same.
+//! The host says how long each restore (`/snapshot/load`) took, which maps the window's mappings
+//! again. Stores through the mapping before the freeze wait for TASK-191: the host keeps a private
+//! page or two its frozen guest doesn't count, and refuses the snapshot.
+//!
 //! fxfs-guest-snapshot-volumes(-dax): a VM with `VOLUMES` volumes mounted on one device freezes,
 //! suspends, thaws and uses each (TASK-136.2). Mounted first, with nothing in the journal, the
 //! default volume reads the device fewer times than there are volumes: the rest stay closed.
@@ -113,6 +120,23 @@ fn mapped_pattern() -> Vec<u8> {
     (0..MAPPED_LEN).map(|i| (i % 251) as u8).collect()
 }
 
+/// The pages of the file whose pages are each an extent of their own (TASK-92.14): many window
+/// mappings, as a zygote's are.
+#[cfg_attr(feature = "host", allow(dead_code))]
+const PIECES: usize = 1024;
+
+/// What the file of `PIECES` pages holds at its page `page`.
+#[cfg_attr(feature = "host", allow(dead_code))]
+fn piece(page: usize) -> Vec<u8> {
+    (0..4096).map(|i| ((page * 7 + i) % 253) as u8).collect()
+}
+
+/// What the file of `PIECES` pages holds.
+#[cfg_attr(feature = "host", allow(dead_code))]
+fn pieces() -> Vec<u8> {
+    (0..PIECES).flat_map(piece).collect()
+}
+
 #[host]
 mod host {
     use super::*;
@@ -353,11 +377,17 @@ mod host {
                 });
                 restored.push(firecracker);
             }
-            // All at once: no restore of one point waits for another's.
+            // All at once: no restore of one point waits for another's. fxdax: each timed, as it
+            // maps the window's mappings again before the VM resumes (TASK-92.14).
             std::thread::scope(|scope| {
-                let loading: Vec<_> = (restored.iter().zip(&loads))
-                    .map(|(firecracker, load)| {
-                        scope.spawn(move || firecracker.request("PUT", "/snapshot/load", load))
+                let loading: Vec<_> = (restored.iter().zip(&loads).zip(self.restores(phase)))
+                    .map(|((firecracker, load), name)| {
+                        scope.spawn(move || {
+                            let start = std::time::Instant::now();
+                            firecracker.request("PUT", "/snapshot/load", load)?;
+                            eprintln!("{name}: restored in {:?}", start.elapsed());
+                            anyhow::Ok(())
+                        })
                     })
                     .collect();
                 for load in loading {
@@ -443,6 +473,7 @@ mod host {
                 assert_eq!(read_file(&fs, "before"), BEFORE, "store {name}: before");
                 assert_eq!(read_file(&fs, "who"), name.as_bytes(), "store {name}: who");
                 assert_eq!(read_file(&fs, "after").len(), 4 << 20, "store {name}: after");
+                assert!(read_file(&fs, "pieces") == pieces(), "store {name}: pieces");
             }
             TestOutcome::Pass
         }
@@ -547,11 +578,16 @@ mod guest {
         }
     }
 
+    /// What the filesystem says of itself as `name`, a count, since it opened.
+    fn stat(name: &str) -> Option<u64> {
+        let stats = fs::read_to_string("/proc/fs/fxfs/extents").unwrap();
+        let line = stats.lines().find_map(|line| line.strip_prefix(name)?.strip_prefix(' '));
+        line.map(|count| count.parse().unwrap())
+    }
+
     /// How many reads of the device the filesystem made since it opened.
     fn device_reads() -> u64 {
-        let stats = fs::read_to_string("/proc/fs/fxfs/extents").unwrap();
-        let line = stats.lines().find_map(|line| line.strip_prefix("device_reads "));
-        line.expect("device_reads").parse().unwrap()
+        stat("device_reads").expect("device_reads")
     }
 
     /// A connection to the host on `PORT`.
@@ -595,6 +631,40 @@ mod guest {
         panic!("every connection to the host was reset");
     }
 
+    /// Writes the file of `PIECES` pages at `path`, each page an extent of its own: written alone,
+    /// last first, and made durable before the next, so that no two follow one another on the
+    /// device, whose window maps each apart (TASK-92.14).
+    fn write_pieces(path: &str) {
+        use std::os::unix::fs::FileExt;
+        let file = fs::File::create(path).unwrap();
+        for page in (0..PIECES).rev() {
+            file.write_all_at(&piece(page), (page * 4096) as u64).unwrap();
+            file.sync_data().unwrap();
+        }
+    }
+
+    /// Maps the file of `PIECES` pages at `path`, shared and read-only, which stays mapped; and
+    /// reads it, so that the window maps each of its pages (TASK-92.14).
+    fn map_pieces(path: &str) -> &'static [u8] {
+        let len = PIECES * 4096;
+        let file = fs::File::open(path).unwrap();
+        let map = unsafe {
+            libc::mmap(
+                std::ptr::null_mut(),
+                len,
+                libc::PROT_READ,
+                libc::MAP_SHARED,
+                file.as_raw_fd(),
+                0,
+            )
+        };
+        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
+        // SAFETY: the mapping, never unmapped.
+        let mapped = unsafe { std::slice::from_raw_parts(map as *const u8, len) };
+        assert!(mapped == &pieces()[..], "the pieces' mapping before the snapshot");
+        mapped
+    }
+
     /// Maps the `MAPPED_LEN` bytes of `path`, shared and read-only, which stays mapped.
     fn map(path: &str) -> &'static [u8] {
         let file = fs::File::open(path).unwrap();
@@ -623,6 +693,11 @@ mod guest {
                 eprintln!("{reads} reads, with {volumes} volumes closed");
                 assert!(reads < volumes as u64, "{reads} reads with {volumes} volumes");
             }
+            // Read in again by the next mount, a DAX file, mapped through the window.
+            write_pieces(&format!("{MNT}/pieces"));
+            unmount();
+            mount();
+            let mapped_pieces = map_pieces(&format!("{MNT}/pieces"));
             for volume in 0..volumes {
                 fs::create_dir(at(volume)).unwrap();
             }
@@ -635,7 +710,17 @@ mod guest {
             let mapped = map(&format!("{MNT}/mapped"));
             // Read through the mapping, which with a DAX window maps the window's pages.
             assert!(mapped == &mapped_pattern()[..], "the mapping before the snapshot");
+            let counts = || (stat("dax_private_pages"), stat("dax_dirty_pages"));
+            eprintln!("before the freeze: (private, dirty) pages {:?}", counts());
             freeze(volumes, FIFREEZE);
+            // Frozen, the window holds no private page, its dirty pages written back and every
+            // one rebound, which a snapshot's fork requires (backlog doc-13; TASK-92.14).
+            let (private, dirty) = counts();
+            assert_eq!(
+                (private.unwrap_or(0), dirty.unwrap_or(0)),
+                (0, 0),
+                "frozen with private and dirty pages"
+            );
             {
                 let mut host = connect();
                 host.write_all(b"frozen\n").unwrap();
@@ -665,6 +750,13 @@ mod guest {
             assert!(mapped == &mapped_pattern()[..], "the mapping after the restore");
             // SAFETY: the mapping `map` made, which nothing uses any more.
             assert_eq!(unsafe { libc::munmap(mapped.as_ptr() as *mut _, MAPPED_LEN) }, 0);
+            // Through the mapping, which the window maps again, and read.
+            let written = pieces();
+            assert!(mapped_pieces == &written[..], "{name}: the pieces' mapping after the restore");
+            assert!(fs::read(format!("{MNT}/pieces")).unwrap() == written, "{name}: pieces read");
+            // SAFETY: the mapping `map_pieces` made, which nothing uses any more.
+            let len = mapped_pieces.len();
+            assert_eq!(unsafe { libc::munmap(mapped_pieces.as_ptr() as *mut _, len) }, 0);
             fs::write(format!("{MNT}/who"), &name).unwrap();
             eprintln!("{name}: wrote");
             // New extents, of the restored store's branch.
@@ -678,6 +770,7 @@ mod guest {
             mount();
             assert_eq!(fs::read(format!("{MNT}/who")).unwrap(), name.as_bytes());
             assert_eq!(fs::read(format!("{MNT}/after")).unwrap(), after);
+            assert!(fs::read(format!("{MNT}/pieces")).unwrap() == pieces(), "{name}: pieces");
             mount_volumes(volumes);
             for volume in 0..volumes {
                 assert_eq!(fs::read(format!("{}/who", at(volume))).unwrap(), name.as_bytes());
```
<!-- SECTION:NOTES:END -->
