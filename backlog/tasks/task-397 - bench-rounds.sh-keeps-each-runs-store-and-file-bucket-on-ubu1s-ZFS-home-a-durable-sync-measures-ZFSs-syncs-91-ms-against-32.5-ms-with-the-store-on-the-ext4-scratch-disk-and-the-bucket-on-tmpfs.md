---
id: TASK-397
title: >-
  bench-rounds.sh keeps each run's store and file bucket on ubu1's ZFS home: a
  durable sync measures ZFS's syncs, 91 ms against 32.5 ms with the store on the
  ext4 scratch disk and the bucket on tmpfs
status: To Do
assignee: []
created_date: '2026-11-13 10:20'
labels:
  - bench
  - ubu1
dependencies: []
priority: medium
ordinal: 781000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-371's AC #4 on ubu1, main 10f613290, FXDAX_REMOTE_LATENCY_MS=25: bench-fs-fxfs-guest-dax's sync-loop runs 10.8/s with bench-rounds.sh's default (results dir, so the store and FXDAX_EXTENTD_REMOTE=file://$out/bucket, under ~/src on ZFS home), 15.9/s with the bucket on /dev/shm, and 30.1/s with the store on /mnt/fxdax-scratch and the bucket on /dev/shm (one bucket round trip, ~30 ms, a sync). The file bucket's create and the store's syncs on ZFS home add ~60 ms a sync: what the measurement then says is ZFS's, not fxdax's or object storage's (a remote bucket's cost is the injected latency). Make bench-rounds.sh's default the representative layout: the run's store on the scratch disk (ext4, as a host's local disk) and its file bucket on tmpfs (the latency injected stands for the remote), results copied back; or refuse a store on ZFS. The earlier 3/s that looked like a borked machine was partly this.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 bench-rounds.sh with no overrides puts the store on /mnt/fxdax-scratch and the file bucket on tmpfs (or a documented equivalent), and the sync-loop on ubu1 at 25 ms reads within 10% of 30/s
- [ ] #2 Earlier ubu1 measurements that ran with the store on ZFS home are noted where they're cited (doc-12's lag figures, TASK-322)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
