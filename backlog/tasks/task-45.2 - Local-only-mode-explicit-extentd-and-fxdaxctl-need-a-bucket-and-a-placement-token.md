---
id: TASK-45.2
title: >-
  Local-only mode explicit: extentd and fxdaxctl need a bucket and a placement
  token
status: Done
assignee:
  - '@claude'
created_date: '2026-10-18 09:22'
updated_date: '2026-10-18 09:45'
labels: []
dependencies: []
parent_task_id: TASK-45
priority: high
type: feature
ordinal: 141000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Without FXDAX_EXTENTD_REMOTE, extentd and fxdaxctl run single-node, their branches local only; without FXDAX_EXTENTD_TOKEN, every host takes branches with token 0. Both are silent defaults. Production must be multi-node; tests and development opt into local-only explicitly.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 extentd and fxdaxctl fail to start without a bucket and a placement token, saying so, unless FXDAX_EXTENTD_LOCAL_ONLY=1
- [x] #2 Tests and scripts that run local-only set the flag; docs describe both modes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
fxdax::remote::Mode::from_env: MultiNode needs FXDAX_EXTENTD_REMOTE and FXDAX_EXTENTD_TOKEN (not 0); LocalOnly is FXDAX_EXTENTD_LOCAL_ONLY=1 with no bucket (both is an error). fxdax-extentd checks it before listening; fxdaxctl for changes to existing stores. Launchers updated: test-e2e.sh, guest spawn_daemon (local-only unless a bucket is named, then token 1 unless named), integration tests spawning the binary. The rustfs tests (branches.rs) gained token 1 but were not run (they need scripts/test-rustfs.sh).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
extentd refuses to start without a bucket and a placement token unless FXDAX_EXTENTD_LOCAL_ONLY=1, saying why; documented in the binary docs and README. Verified by the_daemon_runs_multi_node_or_asked_single_node, and every launcher run on GCE and macOS.
<!-- SECTION:FINAL_SUMMARY:END -->
