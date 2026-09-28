---
id: TASK-11
title: Re-enable test_pager_range_bad_expand_panics
status: To Do
assignee: []
created_date: '2026-09-28 02:41'
labels: []
dependencies: []
priority: low
type: chore
ordinal: 11000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The test expects a panic inside a pager packet handler to fail the test. Fuchsia's test runner supports that; ours aborts the process instead (receiver panics abort, as on Fuchsia), so kernel-cfg.py disables it everywhere. Running it as a subprocess test (like the packets.rs abort test) would restore coverage.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The test runs on the kernel and asserts the process aborts with the expected panic message
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/kernel-cfg.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
