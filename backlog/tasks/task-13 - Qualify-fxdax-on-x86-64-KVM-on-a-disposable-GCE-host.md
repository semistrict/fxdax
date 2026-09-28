---
id: TASK-13
title: Qualify fxdax on x86-64 KVM on a disposable GCE host
status: Done
assignee:
  - '@claude'
created_date: '2026-09-28 22:55'
updated_date: '2026-09-28 22:55'
labels: []
milestone: m-2
dependencies: []
ordinal: 13000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Port butter's disposable GCE host pattern (scripts/bench-memory-gce.sh) to fxdax. The host runs the workspace's tests and libkrun's virtiofs-fxdax guest test on x86-64 with nested KVM.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 scripts/test-gce.sh all creates a labelled, self-expiring host, runs the workspace tests and virtiofs-fxdax, copies the logs back, and deletes the host
- [x] #2 Both pass on x86-64
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/kernel-cfg.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- scripts/test-gce.sh [all|create|run|delete]: n2-standard-8 (Cascade Lake, nested virtualization), Ubuntu 26.04, 200 GB disk. The host carries purpose=fxdax-test/lifecycle=temporary labels, max-run-duration 8h with DELETE, no service account, and zone fallback. Actions refuse unlabelled instances, and deletion is verified. `all` always deletes the host.
- scripts/lib/test-linux-host.sh runs as root on the host. It installs packages, Rust 1.97.1, nextest 0.9.143 and libkrunfw 5.6.2 (built from its checksummed tarball), then runs scripts/test.sh and libkrun make + make test TEST=virtiofs-fxdax (FXDAX_E2E_TEST overrides the test).
- Found on the way:
  - krun-hvf didn't build on x86-64 Linux (libkrun 3bbc65f).
  - x86 KVM's async page faults fault pages in for writing whatever the guest's access. UffdSecondStage now resolves a refused write as a read and makes the page read-only, and documents that loads may dirty pages in writable mappings. Test: dax_uffd_second_stage_refused_write_resolves_the_read.
- Validation: `scripts/test-gce.sh all` exit 0. 1740 tests passed and virtiofs-fxdax OK 1/1 on x86-64. The host deletion was verified.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxdax now runs its qualification on a disposable x86-64 GCE host with nested KVM (scripts/test-gce.sh), modelled on butter's benchmark hosts. The first x86-64 runs found a libkrun build break and an x86 KVM async-page-fault interaction with the userfaultfd second stage; both are fixed and covered. The workspace tests and the virtiofs-fxdax guest test pass there.
<!-- SECTION:FINAL_SUMMARY:END -->
