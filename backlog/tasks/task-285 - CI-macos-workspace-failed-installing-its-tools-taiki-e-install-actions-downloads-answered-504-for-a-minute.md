---
id: TASK-285
title: >-
  CI: macos (workspace) failed installing its tools: taiki-e/install-action's
  downloads answered 504 for a minute
status: To Do
assignee: []
created_date: '2026-11-09 07:45'
updated_date: '2026-11-09 08:07'
labels:
  - ci
dependencies: []
priority: low
ordinal: 706000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Main CI run 42579391459 (f824ffd50): macos (workspace) failed in 'Run taiki-e/install-action@v2', curl (56) 'The requested URL returned error: 504' on 11 tries over 62 s, exit 56; every other job passed. An outage of the download host, not fxdax's code; a run dispatched by hand reran it. Consider caching the installed tools, or a retry over a longer window, so that a short outage doesn't fail the job.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Again in run 42581320255 (dispatched, f824ffd50): macos (shuttle), the same 504s for 61 s, while macos (workspace) passed; in 42579391459 it was the other way round. Rerun of the failed job asked for.

The rerun of macos (shuttle) in 42581320255 hit the same 504s at 08:05-08:06 UTC: the download host was still down 40 min after the first failure.
<!-- SECTION:NOTES:END -->
