---
id: TASK-70
title: Documented VMM exit statuses per failure kind
status: Done
assignee: []
created_date: '2026-10-21 19:54'
updated_date: '2026-11-06 06:50'
labels: []
dependencies: []
priority: medium
ordinal: 179000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
extentd loss exits 158; a blocking store call on a reply thread now panics the VMM (TASK-56). Document an exit status for each kind of VMM failure, so a host can tell storage loss from VMM bugs.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-06 tidy: done by 6a6dea10b (2026-10-31): README.md's 'How a VM process exits' documents every status a VM process exits with, by failure kind and whose it is: 0, 1, 2, 148-157, 158 ExtentdLost (storage) and 159 AccessRefused (the guest), and SIGABRT for a panic. Checked against Firecracker's FcExitCode at vendor/firecracker 96cb97976 (src/vmm/src/lib.rs:185-209) on 2026-11-06.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Done by 6a6dea10b: README's 'How a VM process exits' table names each exit status by failure kind, storage loss (158) apart from the guest (159) and from VMM bugs; it matches FcExitCode at vendor/firecracker 96cb97976.
<!-- SECTION:FINAL_SUMMARY:END -->
