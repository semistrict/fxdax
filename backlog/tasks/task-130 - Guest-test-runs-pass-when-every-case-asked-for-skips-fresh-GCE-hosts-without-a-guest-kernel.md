---
id: TASK-130
title: >-
  Guest test runs pass when every case asked for skips: fresh GCE hosts without
  a guest kernel
status: Done
assignee: []
created_date: '2026-10-29 06:58'
updated_date: '2026-10-29 06:58'
labels:
  - bug
  - testing
dependencies: []
priority: high
ordinal: 466000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
2026-10-29: scripts/test-gce.sh all on fresh hosts (r210, r212, r213, and the guest half of r211, r214's verify) ran no guest case: without FXDAX_GUEST_KERNEL_BUILD the host has no guest kernel, each case SKIPped (FXDAX_GUEST_KERNEL does not name a guest kernel), and the runner said OK - 0/0 passed (1 skipped), test-e2e.sh 'N rounds passed'. Hosts reused from earlier runs had one. Every result from those runs says nothing.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A case asked for by name that skips fails the runner, and so the round
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
runner: a case named by --test-case (not 'all') that skips fails the run, so test-e2e.sh fails its round; scripts/lib/test-linux-host.sh builds a release guest kernel when neither FXDAX_GUEST_KERNEL nor FXDAX_GUEST_KERNEL_BUILD is set. Verified in a rust:1 container: the runner exits 1 for a skipped named case. Runs invalidated: r210, r212, r213, r211's guest half, r214's verify; rerun with a kernel.
<!-- SECTION:FINAL_SUMMARY:END -->
