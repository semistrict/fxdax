---
id: TASK-50
title: 'Production readiness: the sandbox team''s open asks'
status: Done
assignee: []
created_date: '2026-10-19 20:10'
updated_date: '2026-10-20 19:23'
labels: []
dependencies: []
priority: high
ordinal: 149000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sandbox team reviewed fxdax at 8d31ab2 for production (their vendor/fxdax-requirements.md, 'Open asks, in full') and listed seven open items, four of them blockers: running under Firecracker's real seccomp filter and the jailer, what VMs see when extentd dies, the host mapping limit (TASK-12), and the daemon's memory caps. The others: userfaultfd without the node-wide sysctl, store API gaps, and the guest filesystem's lost-fsync bug (TASK-26.6.2) and missing features (TASK-26.19). Each is a subtask or an existing task.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Every subtask, TASK-12 and TASK-26.6.2 are Done
- [x] #2 The sandbox team's open asks each have an answer in the repository: done, tested, or explicitly unsupported with what programs see
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-20: all subtasks, TASK-12, TASK-26.6.2 and TASK-26.19 Done. Answers in the repository: seccomp/jailer (50.1, test-e2e.sh FXDAX_FIRECRACKER_SECCOMP/JAIL), extentd death (doc-4, fxfs-guest-daemon-lost), host mappings (TASK-12 test), daemon memory (README, extentd memory tests), /dev/userfaultfd and min kernel (README), store API (proto, ListStores/StatStore), Docker (fxfs-guest-docker-dax), guest Fxfs features and what's unsupported (README 'Guest Fxfs: what it does not do'). Final verification at bb41664: 27/27 guest tests and 2243 workspace tests on GCE, macOS suite passing.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
All seven of the sandbox team's open asks are done and tested, or documented as unsupported with what programs see (README, doc-4); verified at bb41664 on GCE and macOS.
<!-- SECTION:FINAL_SUMMARY:END -->
