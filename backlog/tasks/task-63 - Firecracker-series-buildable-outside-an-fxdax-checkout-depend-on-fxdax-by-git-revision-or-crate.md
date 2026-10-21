---
id: TASK-63
title: >-
  Firecracker series buildable outside an fxdax checkout: depend on fxdax by git
  revision or crate
status: To Do
assignee: []
created_date: '2026-10-21 19:54'
labels: []
dependencies: []
priority: high
ordinal: 172000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
vendor/firecracker src/vmm/Cargo.toml names fxdax by path = ../../../../crates/fxdax. The sandbox team keeps the series as commits in their firecracker-internal beside their patches; depend on fxdax by a pinned git revision or a published crate. Also: their cherry-pick onto their tag needed a 3-way merge (rpc_interface.rs). Production blocker.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
