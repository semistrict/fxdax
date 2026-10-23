---
id: TASK-91.3
title: 'extentd exports upload lag (bytes pending, oldest pending age) as metrics'
status: Done
assignee:
  - '@main-session'
created_date: '2026-10-22 18:22'
updated_date: '2026-10-23 09:50'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: medium
ordinal: 268000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Durable commits wait for everything pending upload ahead of them. The sandbox team can't see whether the uploader keeps up at 25 ms object-store latency (550-930 MB written per 60 s pgbench run) and asks for upload lag as a metric.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The metrics endpoint exports bytes pending upload and the age of the oldest pending, per store
- [x] #2 Through three 60 s pgbench runs at 25 ms, the lag stays bounded (measured)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
ed66287: /metrics exports per-branch upload_pending_bytes, upload_oldest_pending_seconds and upload_stalls_total. AC2 (lag bounded through three 60 s runs at 25 ms) is measured by fxfs-guest-pgbench's samples.txt (StatStore): after 4317177/fd7a384, without DAX pending stayed under ~25 MB and commit wait ~0 at 25 ms; to confirm against these metrics on the sandbox team's rerun.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
ed66287 exports per-branch upload_pending_bytes, upload_oldest_pending_seconds and upload_stalls_total; 4317177/fd7a384 stopped the checkpoint stalls that let the lag grow. The sandbox team's round 5 at a8e2e02 (2026-10-23): over five 60 s pgbench runs at 25 ms, pending peaked at 33-41 MB with DAX and 265-290 MB without, the oldest under 1.6 s, upload_stalls_total 0, commit wait under 0.6 s a run.
<!-- SECTION:FINAL_SUMMARY:END -->
