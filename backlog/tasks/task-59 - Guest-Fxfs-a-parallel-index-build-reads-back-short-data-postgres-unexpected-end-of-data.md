---
id: TASK-59
title: >-
  Guest Fxfs: a parallel index build reads back short data (postgres 'unexpected
  end of data')
status: Done
assignee: []
created_date: '2026-10-21 19:54'
updated_date: '2026-10-22 10:39'
labels: []
dependencies: []
priority: high
ordinal: 168000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Clean rerun of pgbench -i -s 50: data generation and vacuum finished, then 'alter table pgbench_accounts add primary key (aid)' failed in a parallel worker with ERROR: unexpected end of data. No panic, hung task or Fxfs message. Parallel index builds have workers write sorted runs to temporary files the leader reads back: likely a read returning less than another process wrote (file size or data visibility across processes, DAX). Find it, with a test that reproduces it. Reproduce (their steps): RustFS rustfs:1.0.0-beta.7, bucket fxdax, tc netem delay 25ms on its eth0; docker export of postgres:17; FXDAX_EXTENTD_REMOTE=s3://fxdax/<prefix> FXDAX_EXTENTD_TOKEN=1 fxdaxctl import stores/postgres 17179869184 postgres.tar; FXDAX_EXTENTD_ROOTS=stores FXDAX_EXTENTD_GRPC=extentd.grpc FXDAX_EXTENTD_CACHE=cache fxdax-extentd extentd.sock 8589934592; fxdaxctl place stores/postgres 1, sync, fork stores/postgres stores/vm1, place stores/vm1 1; boot 2 vCPUs, 4096 MiB, extent device tag root capacity 16 GiB dax_window_size 2 GiB, root=root rootfstype=fxfs rw; as postgres: initdb -D $D -A trust; pg_ctl -D $D -o '-c shared_buffers=256MB' -w start; createdb bench; pgbench -i -q -s 50 bench.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Reproduced at scale 50 on fxfs-guest-dax with a GCS bucket (after TASK-58's fix): the primary-key build failed 'Key (aid)=(4831399) is duplicated' (gce-fs-bench-s50-fix-2026-10-22T0925Z), the sort reading back other bytes than written, as the sandbox team's 'unexpected end of data'. Cause: TASK-79.10, a seal remapping a reused window range to the wrong offset of its open extent (the database and sort runs outgrow the 2 GiB window). Fixed in b5bc162; pgbench -i -s 50 then completes 4 of 4 runs (gce-fs-bench-s50-seal-2026-10-22T0951Z, gce-fs-bench-s50-2026-10-22T1002Z). Their exact message wasn't reproduced; the mechanism fits it.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The short or wrong read in the parallel index build was a seal remapping a reused DAX window range to the wrong offset of its open extent (TASK-79.10); fixed in b5bc162. pgbench -i -s 50 completes on Fxfs with DAX and a bucket, 4 of 4 runs.
<!-- SECTION:FINAL_SUMMARY:END -->
