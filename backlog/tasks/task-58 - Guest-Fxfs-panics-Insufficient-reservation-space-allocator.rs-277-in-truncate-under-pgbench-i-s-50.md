---
id: TASK-58
title: >-
  Guest Fxfs panics: Insufficient reservation space (allocator.rs:277) in
  truncate under pgbench -i -s 50
status: To Do
assignee: []
created_date: '2026-10-21 19:54'
labels: []
dependencies: []
priority: high
ordinal: 167000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Kernel panic in a truncate from postgres: fxfs_rt_panic <- fxfs_vfs_write_attr <- fxfs_vfs_resize <- fxfs_vfs_setattr <- notify_change <- do_truncate <- vfs_truncate, 'panicked at crates/fxfs/src/object_store/allocator.rs:277:13: Insufficient reservation space', ~17 min into the load, filesystem far from full (~1.2 GB of 16 GiB). Two minutes before, postgres tasks were hung >120 s; a host image build shared the machine. Find the reservation accounting at fault in the truncate path and fix it. TASK-26.6.1 fixed a related metadata-reservation exhaustion without DAX. Reproduce (their steps): RustFS rustfs:1.0.0-beta.7, bucket fxdax, tc netem delay 25ms on its eth0; docker export of postgres:17; FXDAX_EXTENTD_REMOTE=s3://fxdax/<prefix> FXDAX_EXTENTD_TOKEN=1 fxdaxctl import stores/postgres 17179869184 postgres.tar; FXDAX_EXTENTD_ROOTS=stores FXDAX_EXTENTD_GRPC=extentd.grpc FXDAX_EXTENTD_CACHE=cache fxdax-extentd extentd.sock 8589934592; fxdaxctl place stores/postgres 1, sync, fork stores/postgres stores/vm1, place stores/vm1 1; boot 2 vCPUs, 4096 MiB, extent device tag root capacity 16 GiB dax_window_size 2 GiB, root=root rootfstype=fxfs rw; as postgres: initdb -D $D -A trust; pg_ctl -D $D -o '-c shared_buffers=256MB' -w start; createdb bench; pgbench -i -q -s 50 bench.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
