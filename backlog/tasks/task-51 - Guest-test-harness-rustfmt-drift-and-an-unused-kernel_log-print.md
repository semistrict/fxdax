---
id: TASK-51
title: 'Guest test harness: rustfmt drift and an unused kernel_log::print'
status: Done
assignee: []
created_date: '2026-10-20 12:12'
updated_date: '2026-10-20 14:59'
labels: []
dependencies: []
priority: low
type: chore
ordinal: 159000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
guest/tests/test_cases is not rustfmt-clean (common.rs, guest_init.rs, test_fxfs_guest_checks.rs differ from cargo fmt), and the host build warns that kernel_log::print is never used (seen in every e2e run's build).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 cargo fmt --check passes for guest/tests
- [x] #2 The host and guest builds of test_cases have no warnings
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
kernel_log is compiled for the guest alone, HELLO allowed dead in the host's build (commit after 1efad43); cargo fmt over guest/tests (fmt commit). The host build on GCE showed no test_cases warnings.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
guest/tests is rustfmt-clean (cargo fmt --check passes) and test_cases' host build has no warnings.
<!-- SECTION:FINAL_SUMMARY:END -->
