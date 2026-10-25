---
id: TASK-97
title: >-
  GCE drivers: a region out of its SSD or CPU quota fails the create, though
  another region has room
status: Done
assignee: []
created_date: '2026-10-24 15:35'
updated_date: '2026-10-25 01:34'
labels:
  - bug
dependencies: []
priority: low
ordinal: 383000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
gce_create (scripts/lib/gce.sh) only tries the other zones of the first zone's region, and only on ZONE_RESOURCE_POOL_EXHAUSTED. A regional quota (Quota 'SSD_TOTAL_GB' exceeded, us-east1, 2026-10-24, two hosts of 200 GB running) fails the run at once, though us-central1 had no usage. Fall back to a next region on a quota error.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Again in us-central1 (2026-10-24): 500 GB of SSD quota a region fits two 200 GB hosts; the third create fails though us-west1 had none in use.

Also the project's global quota: CPUS-ALL-REGIONS 32 (2026-10-24, r81 refused with three n2-standard-8 hosts of this session up and another session's): a fourth host waits for one to go.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
gce_create falls back to the next region of FXDAX_GCE_REGIONS on a regional quota or a region out of room; the global CPU quota still fails. Pinned by scripts/tests (test_gce 32/32).
<!-- SECTION:FINAL_SUMMARY:END -->
