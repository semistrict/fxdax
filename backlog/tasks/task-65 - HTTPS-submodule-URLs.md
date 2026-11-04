---
id: TASK-65
title: HTTPS submodule URLs
status: Done
assignee: []
created_date: '2026-10-21 19:54'
updated_date: '2026-11-04 10:40'
labels: []
dependencies: []
priority: low
ordinal: 174000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
.gitmodules uses git@github.com:semistrict/...; use https://github.com/... so clones work without a GitHub SSH key (CI, build boxes).
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Firecracker's submodule URL is https; the nightly fetches it plainly. Verified: macOS scripts/test.sh on the tree merged with batch 2026-11-04 (workspace 2,083/2,083, Shuttle, scripts); GitHub CI on the branch, every job (run 41806511048, Linux workspace with the codec's tests); GCE r540 in the jailer under the production seccomp filter (fxfs-guest-dax, checks-dax, crash-dax, migrate-tenant-dax, pgbench pass; the refusal gate's failure is TASK-167's, the jailer never mounting /proc).
<!-- SECTION:FINAL_SUMMARY:END -->
