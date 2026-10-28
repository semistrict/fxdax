---
id: TASK-112
title: >-
  Sandbox team, round 7b (2026-10-28 report): DAX sharing across VMs, and the
  gap to static virtio-pmem
status: To Do
assignee: []
created_date: '2026-10-28 02:30'
labels: []
dependencies: []
priority: high
ordinal: 423000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox density test at c6d9b7e (GCE n2-standard-8 us-central1-a nested, 8 GiB extentd, RustFS 25 ms; 2 vCPU 2 GiB VMs, DAX window 2 GiB; postgres:17 base, each VM a fork; eight VMs booted in turn and kept running, each running find /usr /etc -xdev -type f -print0 | xargs -0 cat twice: 445 MB in ~14,000 files). Runs 2026-10-28 02:14-02:20 UTC.

Per VM: base files in the guest page cache DAX 22 MB, no DAX 469, JuiceFS 469; VMM proportional share VM 1 -> 8 DAX 477 -> 306 MB, no DAX 635 -> 662, JuiceFS 642 -> 645; host memory added per VM (VMs 2-8) DAX ~285 MB, no DAX ~640, JuiceFS ~635; 8 VMMs + extentd DAX 2.5 + 0.8 GB, no DAX 5.2 + 1.2 GB. DAX VMM private memory ~256 MB; resident ~695 MB of which 300-390 MB shared window pages. Boot 2.2-2.9 s (no DAX 2.0-3.0). First read: DAX VM 1 8.9 s, VMs 2-8 4.5 s (every base page already in extentd memory, ~100 MB/s, ~40 us per 4 KiB page); no DAX 4.3-4.5 s; JuiceFS 5.9-8.5 s. Second read 0.22 s for all.

Their estimate against ext4 dax=always on a read-only virtio-pmem base: 1-3 us per page bare metal, 10-20 us nested; fxdax ~20-30 us extra per page nested. Ideas: map a fork base read-only at boot; map whole extents or 2 MiB per read fault; window reclaim keeps base mappings over open ones; 2 MiB pages (FXDAX_GUEST_HUGE_PAGES=2M: numbers?); prefault boot-time base maps despite 1cd30919. Write-path questions: does every fsync need a seal (durability without immutability, sealing at fork, snapshot or reclaim); can the seal MAPs (1.8 evacuation + ~1 restage) leave the commit path; what sets no-DAX ceiling (0.32 ms per seal, ~49% of native). They ask for ext4-dax-pmem vs ext4-pmem vs fxfs DAX read and pgbench in the same bench VMs, bare metal and nested separately. Repro: TestIntegration_Density via ci/fxdax-pgbench.sh, SMITHBOX_TEST_DENSITY=1 SMITHBOX_TEST_DENSITY_VMS=8 SMITHBOX_TEST_DENSITY_RUNTIMES=fxdax-dax,fxdax-nodax,juicefs-block SMITHBOX_TEST_PGBENCH_RUNTIMES=none.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
