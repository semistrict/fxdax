---
id: TASK-136.1
title: Measure Fxfs on a Linux-sized checkout
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-30 09:08'
updated_date: '2026-11-07 07:45'
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
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
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
<!-- SECTION:NOTES:END -->
