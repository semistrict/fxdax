---
id: TASK-288
title: >-
  Guest Fxfs: parallel file creation stops scaling at ~105-135k creates/s from 2
  jobs on; ext4 scales to 311k
status: To Do
assignee: []
created_date: '2026-11-09 08:05'
updated_date: '2026-11-09 08:30'
labels:
  - perf
  - fxfs
dependencies: []
priority: medium
type: enhancement
ordinal: 713000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-276 baseline (GCE c3-standard-8, nested KVM, main 0f011561a, fs bench scaling group, 3 iterations, medians): N threads each creating 2000 empty files in a directory of its own (scale-create-jN, ops/s across all).

| vCPUs | case | j1 | j2 | j4 | j8 |
|---|---|---|---|---|---|
| 2 | ext4 on virtio-blk | 106k | 158k | 158k | 162k |
| 2 | fxfs-guest | 84k | 132k | 118k | 106k |
| 2 | fxfs-guest-dax | 87k | 128k | 102k | 117k |
| 4 | ext4 | 107k | 145k | 211k | 240k |
| 4 | fxfs-guest | 86k | 132k | 133k | 129k |
| 4 | fxfs-guest-dax | 86k | 118k | 130k | 119k |
| 8 | ext4 | 106k | 163k | 209k | 311k |
| 8 | fxfs-guest | 71k | 105k | 128k | 107k |
| 8 | fxfs-guest-dax | 70k | 135k | 124k | 108k |

Fxfs gains from a second thread, then holds or falls, at every vCPU count, while ext4 keeps scaling with vCPUs. The device is not it: create sends the extent device no requests at 1 job, and at most 2-3 of its queues see any at 8 jobs (flushes); TASK-276's queue and interrupt changes leave it within drift. So it is inside the guest's Fxfs: something every create in the filesystem serializes on (a store- or volume-wide lock, the journal, object-id allocation, the LSM's mutable layer), not per directory, as each thread has its own. The host profile (perf record-all) sees only the vCPU threads; a guest kernel profile (FXDAX_BENCH_GUEST_PERF) of scale-create-j1 against -j8 says where; see the notes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A guest kernel profile of parallel creates (1 vs 8 jobs, 4 vCPUs) names what they serialize on
- [ ] #2 Fxfs parallel creates in separate directories scale with jobs at 4 and 8 vCPUs as ext4 does, within the TASK-276 bench's interleaved A/B (scripts/bench-ab-gce.sh) against ext4 as control
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Guest kernel profile (2026-11-09, FXDAX_BENCH_GUEST_PERF=1, fxfs-guest, 4 vCPUs, TASK-276's build, GCE c3-standard-8, perf over each workload; results in the session's create-prof, logs/bench-fs-fxfs-guest/*/log.txt PROFILE/INCLUSIVE/CALLERS scale-create-j1 and -j8):
- j1: 75% of samples idle (pv_native_safe_halt): one thread creating, a CPU of four.
- j8 (8 threads, 4 vCPUs): 21% fxfs_rt_park called straight from fxfs_create (the guest's Rust runtime parking the thread while create's future waits: on a lock or the journal), 13% idle with 8 threads runnable, so they wait rather than run. Inclusive under fxfs_create (61%): Transaction::commit 27%, of it Journal::commit 17% and Journal::write_and_apply_mutations 16.6%; ObjectManager::apply_transaction 13%, apply_mutation 12.8%, ObjectStore::apply_mutation 11.4%; LSM skip-list layer insert and TreeCache 2.5% and 2.5%. Points at the commit path, every create's transaction applied through the journal one at a time, with the parks waiting their turn: a filesystem-wide serialization, as each thread has its own directory.
- Which lock or wait the parks are on is inlined into fxfs_create here: next, a kernel built with FXDAX_GUEST_KERNEL_PROFILE=1 (frames through Rust), or Fxfs's lock statistics, at j1 vs j8.
<!-- SECTION:NOTES:END -->
