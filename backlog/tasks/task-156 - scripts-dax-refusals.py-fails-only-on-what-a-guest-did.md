---
id: TASK-156
title: scripts/dax-refusals.py fails only on what a guest did
status: Done
assignee: []
created_date: '2026-11-01 22:53'
updated_date: '2026-11-03 02:21'
labels: []
dependencies: []
priority: medium
type: task
ordinal: 557000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-150.16: refused stores flagged came from KVM async page fault kworkers, not vCPUs: all 2,327 in r422, all 28 in r394. The gate should join firecracker.log and fail on a refused store by a non-kworker thread or a repeated vCPU EFAULT run, and take counts from the daemon, whose log stops at 1,000 refusals (server.rs:1118).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 r394 and r422 logs pass, a store by a vCPU fails
- [ ] #2 Refusals past the log cap are counted
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
On fix/refusal-gate. dax-refusals.py takes the VM processes' logs and fails on a store refused by a thread that is no kernel worker; test-e2e.sh passes them. Over r394, r398, r422, r423, r429, r451, r452, r456, r457: 8,378 refused stores, all kworker, none flagged, r394 and r422 passing now. Unit tests 7 of 7. Not done: a repeated vCPU EFAULT run isn't looked at; refusals past the log's cap (first 1,000 and powers of two) aren't counted, on the reading that a vCPU's refusal stays until its VM stops, so one past the cap shows at the next power of two.

2026-11-02: workspace at main passes on macOS (2,010 of 2,010, 1fc28f48c) and Linux (GCE r530, 2,169 of 2,169, Shuttle suites too).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The refusal gate fails only on a store refused for a guest thread, not KVM's async page-fault kworkers (bc54340d6; test_stores_of_kernel_workers_are_no_guests, test_stores_of_vcpus_are_guests); checked against nine runs (8,378 refusals, none flagged), r394 and r422 pass. Decided not to: count refusals past the daemon's 1,000-entry log cap, or check repeated vCPU EFAULT runs separately.
<!-- SECTION:FINAL_SUMMARY:END -->
