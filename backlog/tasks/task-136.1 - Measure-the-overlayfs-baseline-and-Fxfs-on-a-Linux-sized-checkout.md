---
id: TASK-136.1
title: Measure Fxfs on a Linux-sized checkout
status: Done
assignee:
  - '@claude'
created_date: '2026-10-30 09:08'
updated_date: '2026-11-07 11:13'
labels: []
dependencies: []
documentation:
  - backlog/docs/doc-10 - Shared-volumes-Fxfs-volumes-over-read-only-bases.md
parent_task_id: TASK-136
priority: medium
type: spike
ordinal: 468000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The design makes claims about cost that nothing has measured yet. The numbers decide how much per-guest metadata caching matters. There is no overlayfs comparison: doc-10.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The results are recorded in doc-10, with the kernel, host and checkout they came from
- [x] #2 A repeatable test in the repository measures mount time, first `git status`, first write and memory per sandbox for Fxfs on a Linux-sized checkout, and the same with hundreds of repos mounted at once
- [x] #3 The test also records the checkout's Fxfs volume layer bytes and allocator records
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Land the watchdog's log line alone: Acks::look gives a Silence (silent, behind) that says which limit a VM process passed, with a unit test.
2. Port t3/fxfs-shared-repo-overlays' measure onto main: shared_volumes.rs (the shared-volume tests' common steps), Filesystem::metadata (layer bytes, allocator records).
3. Extend it into linux-checkout-{ext4,fxfs,fxfs-dax} (Tag::Manual): clone and checkout timed apart, git status cold/warm/synced, a full-tree read cold and warm, in the checkout VM and a fresh VM on the same disk or store; host memory per VM through crate::measure (Requests::serve, a thread answering as the VM talks on another port); disk per store after each VM; the overlay sandboxes, 256 mounts and reboot as before.
4. Run on GCE (r870: the three cases on one n2-standard-8; r871: workspace tests, shared-volumes and bigmap cases).
5. Record numbers in docs/measurements/ and doc-10's Proof; say what doc-10 should change.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-06: branch t3/fxfs-shared-repo-overlays (worktree t3-7a447b3d, 11-01) has an unlanded start on this: the Linux-checkout shared-volume cost for doc-10, and an extentd watchdog log line saying why it kills a VM process. Start from it; the log line is small and worth landing alone.

2026-11-07 (GCE r870, n2-standard-8 us-central1-b, release guest kernel 6.18.54 amzn2023+series x86_64, VMs 4 vCPU/4 GiB, daemon 1 GiB, window 1 GiB): linux-checkout-{ext4,fxfs,fxfs-dax} all pass (83 s, 339 s, 525 s). Numbers in docs/measurements/linux-checkout-2026-11-07.md and doc-10's Proof.
- Checkout plus sync: ext4 20.9 s, Fxfs 48.6 s, DAX 48.2 s. Clone 11.6-12.7 s for all three.
- git status: cold in a fresh VM 0.68 / 0.83 / 1.02 s; warm 0.13-0.18 / 0.17-0.23 / 0.15-0.26 s.
- Full read of the 1.54 GB tree, fresh VM: cold 19.5 / 36.7 / 18.7 s; warm 0.58 / 0.63 / 25.6 s. DAX never warms: the window and the arenas (1 GiB each) are smaller than the tree.
- Host memory after one read: VM anon 2.1 GB without DAX (kept after drop_caches), 0.5 GB with DAX plus 0.2-0.45 GB shared. A status's metadata costs 130-185 MiB a sandbox, not shared.
- Overlay: first open 60 ms (attach 21-25, import 32-36, mount 1.5 ms). Statuses and writes cost what they do unshared. 1.7 MiB of disk and 1.2 MiB of guest memory per added checkout. Volume metadata: 18.9 MB of layers, 1,557 allocator records, 600-byte manifest.
- The Lima run's 30-40 s statuses, 0.3-0.5 s first writes and 91,727 records didn't reproduce on GCE.
- Unexplained: the daemon's PSS is 937-959 MiB after the checkout VM, with 2-6 MiB in its arenas.
- Landed from t3/fxfs-shared-repo-overlays: the watchdog's log line (Silence, a_silence_says_which_limit_it_passed) and shared_volumes.rs plus Filesystem::metadata. Added: the ext4 case, cold/warm/full-read steps, host memory per VM through crate::measure (Requests::serve), disk per store, and scripts/linux-checkout-report.py with its test.
- Not done: r872, the rerun with a 4 GiB window and 6 GiB of daemon memory for the warm DAX case, and r871 (shared-volumes and bigmap guest cases on the refactored helpers). gcloud's credentials expired and need gcloud auth login (OPEN_QUESTIONS). No change to crates/extent_store, extent_remote, remote.rs or stores.rs was needed.

Verification: GCE r870 passed linux-checkout-{ext4,fxfs,fxfs-dax}. macOS scripts/test.sh clippy+workspace: 2053/2053 passed. scripts/tests: 172 passed. upstream-changes --check passed. fxdax unit tests a_silence_says_which_limit_it_passed and volume_metadata pass on Linux (aarch64 container). After rebasing onto main 394fe7535, fxdax and test_cases check clean on macOS and Linux. Landing candidate: land-r1 2e91b42af, CI run 42352478087. Linux workspace DoD waits on that CI run. Kept In Progress for r871 and r872.

2026-11-07, GCE r872 (the user's ask: TASK-227's target). linux-checkout-ext4-pmem was added: the same checkout on an 8 GiB ext4 image on virtio-pmem, mounted dax=always (checked in /proc/mounts). All four cases ran on one n2-standard-8 with an 8 GiB window and 6 GiB of daemon memory, and all passed.
- Warm full-tree reread, checkout VM / fresh VM: ext4-pmem 729 / 712 ms, Fxfs DAX 801 / 751 ms (5-10% slower), sandboxes 759-781 ms. ext4 on virtio-blk 763 / 766 ms, Fxfs 858 / 791 ms.
- The gap to pmem matches Fxfs's gap to ext4 without DAX, and is within the spread between VMs. Inferred, not profiled: it is most likely Fxfs's per-file open and lookup cost, not the DAX mapping.
- Cold read in a fresh VM: Fxfs DAX 5.6 s against pmem 8.8 s. A sandbox over the base: 34-35 s.
- Host memory after one read: pmem RSS 2,478 MiB (588 MiB anon). DAX RSS 2,318 MiB (626 MiB anon, 843 MiB shared).
- Hosts r871 and r872 were deleted after the login. r873 (r871's rerun) was deleted by the user for CPUs.

Validation at the close: land-r1 870f3de7c is on main, rebased onto c9207e5d4 (TASK-165.9). CI 42360840152 was green on it, Linux workspace and guest parts included. macOS scripts/test.sh clippy+workspace+scripts passed: 2080/2080 and 173/173. Unmeasured: fxfs-guest-shared-volumes(-dax) and fxfs-guest-bigmap(-dax) haven't run on GCE with the refactored shared_volumes.rs and measure.rs (r871 was stopped by gcloud's expired login; the user deleted r873, in us-central1-a after a fallback, for quota). The linux-checkout Fxfs cases exercise the same Vms, run_consumer and Requests::serve paths, passing in r870 and r872 before the rebase. The shared-volume cases are in the kernel and nightly suites, so the next run of either covers them.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Measured a Linux-sized checkout (v6.18: 91,166 files, 1.83 GB) on ext4 (virtio-blk and virtio-pmem with DAX) and on the guest's Fxfs with and without DAX, and shared as a volume over a read-only base. linux-checkout-{ext4,ext4-pmem,fxfs,fxfs-dax} (tag manual) time clone, checkout, git status cold and warm, and a full-tree read cold and warm. They measure host and guest memory per VM through crate::measure (Requests::serve), disk per store, the overlay's first open, 256 mounts and a reboot. scripts/linux-checkout-report.py makes the tables. Also landed: the extentd watchdog's log line (Silence, tested) and Filesystem::metadata. GCE r870 (1 GiB window): the overlay adds nothing a status or write sees; first open 60 ms; 1.7 MiB of disk and 1.2 MiB of memory per added checkout; a status's 130-185 MiB of metadata is per sandbox; DAX keeps about 1.6 GB of a read tree out of each VM but never warms with a window smaller than the tree. GCE r872 (8 GiB window, 6 GiB of daemon memory): the warm reread takes 751-801 ms on Fxfs DAX against 712-729 ms on ext4-pmem, TASK-227's baseline; cold in a fresh VM 5.6 s against 8.8 s. Results in doc-10's Proof and docs/measurements/linux-checkout-2026-11-07.md. Verified by GCE r870 and r872 (all cases passed), macOS 2080/2080 and CI 42360840152. Not run: the shared-volume and bigmap guest cases on the refactored helpers (see notes).
<!-- SECTION:FINAL_SUMMARY:END -->
