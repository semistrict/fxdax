---
id: TASK-44.16.1
title: >-
  extentd: log each MAP with its window offset, extent and length, as the
  guest's fxfs_window_map_start traces it
status: To Do
assignee: []
created_date: '2026-11-08 14:53'
labels: []
dependencies: []
parent_task_id: TASK-44.16
priority: low
ordinal: 693000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-44.16's AC1 asks that extentd log its MAPs with the same offsets the guest traces, so that a run's guest trace (fs/fxfs/fxfs_trace.h's events, docs/guest-tracepoints.md) and extentd's log merge into one history for the trace checker (AC2). The guest side landed (commit ad266aff6, held for the boot-cost decision): fxfs_window_map_start gives the window offset, extent (branch:sequence), offset, length and MAP_MANY count. extentd logs no MAP, UNMAP or PRIVATE today (no log line in crates/fxdax's extentd or the VMM's virtio extent device mentions window_offset). The device's requests carry no id on the wire, so the window offset (and time) is what joins the two.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 extentd logs each MAP, UNMAP and PRIVATE with its VM, window offset, length and, for a MAP, its extent and offset, at a level off by default
- [ ] #2 A guest test merges a run's tracefs events with extentd's log and finds each guest MAP's window offset in it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
