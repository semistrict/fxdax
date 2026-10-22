---
id: TASK-61
title: >-
  40 s COMMITs and half-second WRITEs under the sandbox team's pgbench (object
  store with 25 ms latency, nested)
status: To Do
assignee: []
created_date: '2026-10-21 19:54'
updated_date: '2026-10-22 13:01'
labels: []
dependencies: []
priority: high
ordinal: 170000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Their device counters at VM exit (queue 0): COMMIT 7 at 40.2 s avg, WRITE 2,578 at 518 ms, READ 346 at 100 ms (q1 317 ms), UNMAP 1,808 at 4.7 ms, FREE 116 at 4.8 ms, MAP 23,690 at 4.2 ms. Our GCE measurements: WRITE 108-278 us. The guest's busiest vCPU ran near 100% the whole load while the VMM's fault thread and queues idled. Explain and fix: durable commits waiting for uploads at 25 ms per object-store response, MAP churn (23,690 MAPs), the busy vCPU. Reproduce (their steps): RustFS rustfs:1.0.0-beta.7, bucket fxdax, tc netem delay 25ms on its eth0; docker export of postgres:17; FXDAX_EXTENTD_REMOTE=s3://fxdax/<prefix> FXDAX_EXTENTD_TOKEN=1 fxdaxctl import stores/postgres 17179869184 postgres.tar; FXDAX_EXTENTD_ROOTS=stores FXDAX_EXTENTD_GRPC=extentd.grpc FXDAX_EXTENTD_CACHE=cache fxdax-extentd extentd.sock 8589934592; fxdaxctl place stores/postgres 1, sync, fork stores/postgres stores/vm1, place stores/vm1 1; boot 2 vCPUs, 4096 MiB, extent device tag root capacity 16 GiB dax_window_size 2 GiB, root=root rootfstype=fxfs rw; as postgres: initdb -D $D -A trust; pg_ctl -D $D -o '-c shared_buffers=256MB' -w start; createdb bench; pgbench -i -q -s 50 bench.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-22: fxfs-guest-pgbench-dax (TASK-62) runs the sandbox team's load, pgbench -i -s 50 on a fork through a 2 GiB DAX window, against a bucket 25 ms away (FXDAX_REMOTE_LATENCY_MS=25, object_store's ThrottledStore, on every request). On a GCE n2-standard-8 (one level of nesting) it took 25.8 s: generate 14.75 s, vacuum 3.35 s, primary keys 7.62 s. So bucket latency alone doesn't make it slow; their triple nesting (MAP 4.2 ms against our 48 µs) remains the lead.
<!-- SECTION:NOTES:END -->
