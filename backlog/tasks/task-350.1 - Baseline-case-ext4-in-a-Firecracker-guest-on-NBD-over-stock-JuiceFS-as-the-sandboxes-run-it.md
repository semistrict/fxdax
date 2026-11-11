---
id: TASK-350.1
title: >-
  Baseline case: ext4 in a Firecracker guest on NBD over stock JuiceFS, as the
  sandboxes run it
status: To Do
assignee: []
created_date: '2026-11-11 10:55'
labels: []
dependencies: []
parent_task_id: TASK-350
priority: medium
ordinal: 705000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every scenario under TASK-350 compares against this. It must match production's setup (TASK-350's description), not an idealised JuiceFS: the JuiceFS Go library at v1.3.1 serving one image file over NBD without FLUSH, ext4 in the guest, 4 MiB blocks, no compression, no writeback, a local block cache, PostgreSQL metadata, the bucket shared with fxdax's case. JuiceFS (Apache-2.0), PostgreSQL (PostgreSQL License) and RustFS (Apache-2.0) are OSI-licensed. The guest's RAM, vCPUs and kernel match the fxdax case's, so that the guest's own page cache is what production gives it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A bench case (e.g. bench-fs-ext4-juicefs-nbd) boots a Firecracker guest with ext4 on an NBD device backed by a JuiceFS file and runs the existing fs-bench workloads on ubu1
- [ ] #2 Its JuiceFS settings are recorded with each run and match juicebox's init.go (block size, compression, cache size, MaxUpload, no writeback) unless a run says otherwise
- [ ] #3 Bucket and metadata latency are configurable; the defaults are recorded (bucket the same as fxdax's FXDAX_REMOTE_LATENCY_MS, PostgreSQL ~1 ms as Cloud SQL)
- [ ] #4 A test in the repo checks the case: an fsync'd file written in the guest is read back after the VM restarts from the JuiceFS file
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
