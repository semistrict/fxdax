---
id: TASK-197
title: 'DaxWindow::gauges warns as dead code on macOS'
status: To Do
assignee: []
created_date: '2026-11-06 06:46'
labels:
  - cleanup
dependencies: []
priority: low
ordinal: 645000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
crates/fxdax/src/dax.rs's DaxWindow::gauges (TASK-111.5) is called only by extentd's server (server.rs, Linux alone), so every macOS build and scripts/test.sh's clippy part warn 'method gauges is never used'. Its neighbours that only Linux calls (make_private) carry #[cfg_attr(not(target_os = "linux"), allow(dead_code))]. Left out of the 2026-11-06 tidy (tidy-code) because dax.rs was being rewritten by another agent at the time; fxdaxctl's json, the other warning, was fixed there.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 scripts/test.sh's clippy part on macOS prints no dead-code warning
- [ ] #2 DaxWindow::gauges is still used by extentd on Linux (Linux clippy clean)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
