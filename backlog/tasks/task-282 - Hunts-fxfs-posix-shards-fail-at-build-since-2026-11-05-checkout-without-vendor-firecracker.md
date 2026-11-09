---
id: TASK-282
title: >-
  Hunt's fxfs-posix shards fail at build since 2026-11-05: checkout without
  vendor/firecracker
status: Done
assignee: []
created_date: '2026-11-09 07:26'
updated_date: '2026-11-09 08:21'
labels:
  - ci
  - bug
dependencies: []
priority: high
ordinal: 704000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every scheduled Hunt since 2026-11-05 (runs at b57d57e4a, d4d473576, b6ba50fb3...) fails all 4 fxfs-posix shards in seconds: 'no matching package named seccompiler found, location searched: vendor/firecracker/src/seccompiler, required by package fxdax'. cargo resolves the whole workspace and fxdax depends on Firecracker's seccompiler by path, but hunt.yml's fxfs-posix job checked out without submodules (its extentd job already had submodules: true, TASK-149.28). So the nightly property hunt over fxfs-posix has run nothing for 5 days. Fix: submodules: true on that job's checkout.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Hunt's fxfs-posix shards build and run their cases (a dispatched Hunt run shows them green or failing on a test, not at build)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
hunt.yml's fxfs-posix job now checks out vendor/firecracker (f824ffd50). Dispatched Hunt run 42579403046 on main: all four fxfs-posix shards built and passed their cases; before, every scheduled Hunt since 2026-11-05 failed them at cargo's workspace resolution.
<!-- SECTION:FINAL_SUMMARY:END -->
