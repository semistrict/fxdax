---
id: TASK-10
title: End-to-end guest tests on macOS and Linux
status: To Do
assignee: []
created_date: '2026-09-28 02:41'
updated_date: '2026-09-28 04:35'
labels: []
milestone: m-2
dependencies:
  - TASK-8
  - TASK-9
type: task
ordinal: 10000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Unit tests exercise the server and kernel directly; only a real guest proves the whole stack. Tests boot a libkrun guest with an fxdax share and run filesystem workloads with and without DAX.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An e2e test boots a guest on macOS and on Linux, mounts the share, and runs create/read/write/rename/unlink/mmap workloads with DAX enabled and disabled
- [ ] #2 Data written by the guest survives guest shutdown, host remount and fsck
- [ ] #3 The tests run from the repo with one documented command per host
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/kernel-cfg.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
vendor/libkrun branch fxdax still carries the RedoxFS e2e test (tests/test_cases/src/test_virtiofs_redoxfs.rs, and a redoxfs path dependency in tests/test_cases/Cargo.toml that does not resolve in the fxdax repo); port it to fxdax here.
<!-- SECTION:NOTES:END -->
