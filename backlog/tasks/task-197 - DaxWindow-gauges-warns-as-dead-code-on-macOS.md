---
id: TASK-197
title: 'DaxWindow::gauges warns as dead code on macOS'
status: Done
assignee: []
created_date: '2026-11-06 06:46'
updated_date: '2026-11-07 23:50'
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
- [x] #1 scripts/test.sh's clippy part on macOS prints no dead-code warning
- [x] #2 DaxWindow::gauges is still used by extentd on Linux (Linux clippy clean)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Landed in 5f2e45de1 (land-fmt, CI 42423685730 green on af5b0cb72). DaxWindow::gauges, import::store_keeping and the store_requests module, which only extentd (Linux) calls but whose tests run everywhere, carry #[cfg_attr(not(target_os = "linux"), allow(dead_code))] as their neighbours do. FXDAX_TEST_PARTS=clippy scripts/test.sh on macOS: rc=0, no warning. CI's Linux clippy, Linux and macOS workspace jobs green. Same branch: rustfmt on 22 of fxdax's own files; Fuchsia-cited files, guest/rust/std and vendor/ keep upstream formatting.
<!-- SECTION:FINAL_SUMMARY:END -->
