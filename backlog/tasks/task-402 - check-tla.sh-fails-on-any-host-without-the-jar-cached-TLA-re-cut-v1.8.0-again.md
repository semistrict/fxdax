---
id: TASK-402
title: >-
  check-tla.sh fails on any host without the jar cached: TLA+ re-cut v1.8.0
  again
status: To Do
assignee: []
created_date: '2026-11-13 17:01'
labels:
  - tla
  - scripts
dependencies: []
priority: medium
ordinal: 785000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
scripts/check-tla.sh pins tla2tools v1.8.0 (sha256 011267fc...), a pre-release TLA+ re-cuts in place. A fresh download on ubu1 (2026-11-13) was e1f11b75... and the check refused it, so the TLA+ checks run only where an old jar is cached (the macOS laptop): ubu1, CI and any new host fail. Worked around for TASK-395.1's BucketTakeovers run by copying the laptop's pinned jar to ubu1's cache.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 check-tla.sh runs on a host with an empty ~/.cache/fxdax/tla
- [ ] #2 The pinned jar can't change under the pin again (a release that stays put, or the jar kept where fxdax controls it)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
