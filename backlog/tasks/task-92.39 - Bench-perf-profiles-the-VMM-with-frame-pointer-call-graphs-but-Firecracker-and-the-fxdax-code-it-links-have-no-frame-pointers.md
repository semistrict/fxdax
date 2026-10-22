---
id: TASK-92.39
title: >-
  Bench perf profiles the VMM with frame-pointer call graphs, but Firecracker
  and the fxdax code it links have no frame pointers
status: To Do
assignee: []
created_date: '2026-10-22 21:06'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 314000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Only extentd is rebuilt with force-frame-pointers (test-e2e.sh:47-50). Firecracker, which links the extentd client and the uffd second stage, is built plainly (64), yet it is recorded with perf record -g (test_fs_bench.rs:625). The repo's VMM profile shows garbage frames after Vcpu::running and KvmPrefault::prefault, leaving about 30% of samples, including the request-path hand-offs, unattributable.

Where: scripts/test-e2e.sh:47-50, scripts/test-e2e.sh:64, vendor/firecracker/.cargo/config.toml:3-5, guest/tests/test_cases/src/test_fs_bench.rs:625.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present. Under FXDAX_BENCH_PERF only fxdax-extentd is rebuilt with -C force-frame-pointers=yes (scripts/test-e2e.sh:44-53). Firecracker is built with only -Ccodegen-units=1 (scripts/test-e2e.sh:62; vendor/firecracker/.cargo/config.toml rustflags; Cargo.toml profile.release has none). The VMM is still recorded with perf record -g (guest/tests/test_cases/src/test_fs_bench.rs:630-631). Not tracked. Related: TASK-41, TASK-25.2, TASK-26.18.3.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [ ] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
