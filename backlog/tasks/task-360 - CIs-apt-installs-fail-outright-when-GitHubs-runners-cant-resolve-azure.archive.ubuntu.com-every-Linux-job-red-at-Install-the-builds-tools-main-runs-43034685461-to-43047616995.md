---
id: TASK-360
title: >-
  CI's apt installs fail outright when GitHub's runners can't resolve
  azure.archive.ubuntu.com: every Linux job red at 'Install the build's tools'
  (main runs 43034685461 to 43047616995)
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-11 15:32'
updated_date: '2026-11-11 15:54'
labels:
  - ci
dependencies: []
priority: medium
type: bug
ordinal: 745000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
2026-11-11, from 15:00 UTC: main's CI runs 43034685461 (7ad3c5236), 43037954231, 43041159143 and 43047616995 (cf757b22e) failed every Linux job at 'Install the build's tools' (.github/workflows/ci.yml): apt-get update warned 'Temporary failure resolving azure.archive.ubuntu.com' and apt-get install then failed (E: Failed to fetch ... exit 100), before any of fxdax's code built. An outage of GitHub's runners' DNS or Azure's mirror, not fxdax's; but one failed fetch fails the whole run, and the runs went red for hours. Options: apt's own retries (-o Acquire::Retries=5) and a retry of the step; or the packages from a cache (actions/cache of /var/cache/apt/archives, or a prebuilt image).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A transient mirror failure retries rather than failing the job
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
