---
id: TASK-34
title: 'extentd: serve a few hundred open stores per host'
status: To Do
assignee: []
created_date: '2026-10-17 15:26'
labels:
  - performance
  - extentd
dependencies: []
priority: medium
ordinal: 103000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Forks of one base image are expected to run a few hundred at a time on one host, each a VM whose extent store extentd serves. Today each connection takes 8 worker threads (server.rs WORKERS) and a flush thread, each store may keep 64 files open (Options::open_files), and each holds its own copy of its branch's extent index in memory. None of it was tried with more than a handful of VMs. Scope: the disk image (extentd, extent_store).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 extentd serves 300 open stores, forks of one base, on one host, with threads, open files and memory per store measured and bounded, shown by a test or bench that opens them
- [ ] #2 Threads do not grow linearly with connections: requests are served from a shared pool sized by the host
- [ ] #3 A store's open files and its memory but for what it changed are bounded per store, and documented
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
