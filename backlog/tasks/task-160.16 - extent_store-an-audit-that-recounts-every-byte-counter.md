---
id: TASK-160.16
title: 'extent_store: an audit that recounts every byte counter'
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 587000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
About 15 running byte counters (Space, Uncommitted, Segment, PartCounts, Segments, OwnFiles, KeptExtents) are kept up by hand; nothing recomputes them, and a drift shows as a wrong ENOSPC. Explains TASK-79.2 (20+ commits), 79.89, 79.105. Proposal: a test-build State::audit() recomputing every counter after each public mutation under the Shuttle, crash and test builds.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 audit() runs after every mutation in tests
- [ ] #2 A mutant dropping one increment fails on the first seed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
