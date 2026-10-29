---
id: TASK-96.1
title: >-
  Delete the Zircon kernel port: the store, the DAX window and host Fxfs over
  plain memory
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-29 00:24'
updated_date: '2026-10-29 00:24'
labels: []
dependencies: []
parent_task_id: TASK-96
ordinal: 458000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxdax runs a port of the Zircon kernel (crates/zircon, ~40k lines) inside its processes. It came in for host-side Fxfs: fxfs-platform, Fuchsia's file layer, keeps file contents in pager-backed VMOs. The DAX window, the extent cache, tenant memory arenas, memory relief and page quarantine were then built on the kernel already in the process. None of them needs a general-purpose kernel: sealed extents never change, so a cache of them never invalidates or writes back. Writing Fxfs images does not need it either: the guest's Fxfs (guest/rust/fxfs-linux) uses the fxfs crate with no Zircon calls. The store layer of TASK-96 should not carry a kernel port, and nothing else should either. Decided with the user on 2026-10-28: delete it outright, on a branch.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 No workspace crate depends on zircon; crates/zircon, crates/zircon-core-tests, crates/zircon_macros and the crates that only it or fxfs-platform needed are deleted
- [ ] #2 Host Fxfs (images, OCI import, fxdaxctl) runs on the fxfs crate directly, without fxfs-platform, vfs or FIDL channels
- [ ] #3 The extent cache and the DAX window run over plain memory: one copy of each sealed extent in memory across VMs, tenant arenas a VM process alone reaches, a memory limit with eviction, and pages a VM may still reach kept until it cannot
- [ ] #4 The fxdax tests that covered those behaviours over Zircon (arenas, isolation, memory, memory pressure, shared pages, filesystem, image, import, DAX) cover them over the new code and pass on macOS and on GCE
- [ ] #5 The guest DAX, crash and pgbench suites pass on GCE, and pgbench is within noise of main
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
