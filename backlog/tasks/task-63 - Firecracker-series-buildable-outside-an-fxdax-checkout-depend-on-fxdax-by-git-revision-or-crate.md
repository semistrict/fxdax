---
id: TASK-63
title: >-
  Firecracker series buildable outside an fxdax checkout: depend on fxdax by git
  revision or crate
status: Done
assignee: []
created_date: '2026-10-21 19:54'
updated_date: '2026-11-03 06:16'
labels: []
dependencies: []
priority: low
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-03: the user: the sandbox team has all the source and handles config and integration themselves; not ours to do.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Dropped, not done (2026-11-03): the sandbox team has all of fxdax's source and handles building and integrating it themselves; the user said not to work on it.
<!-- SECTION:FINAL_SUMMARY:END -->
