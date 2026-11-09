---
id: TASK-271
title: >-
  Firecracker v1.16.1 test_set_tsc fails on GCE hosts: KVM accepts the TSC
  frequency the test expects refused
status: To Do
assignee: []
created_date: '2026-11-08 23:59'
labels:
  - firecracker
dependencies: []
priority: low
type: bug
ordinal: 698000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Upstream v1.16.1, unchanged, fails vmm arch::x86_64::vcpu::tests::test_set_tsc on an n2-standard-4 GCE host (nested KVM, kernel 7.0.0-1011-gcp): vcpu.rs:1167 unwrap_err() on Ok. Found running the vmm suite on GCE for TASK-269 (the opt-in series fails it exactly as upstream does, both ways), and by the PCI agent (TASK-270) at 06c161526. Every full vmm run on GCE shows it red.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The vmm suite on a GCE host is green: the test skipped or adjusted where KVM allows any TSC frequency, with why, or the host fixed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
