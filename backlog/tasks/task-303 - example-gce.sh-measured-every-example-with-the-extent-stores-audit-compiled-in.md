---
id: TASK-303
title: >-
  example-gce.sh measured every example with the extent store's audit compiled
  in
status: To Do
assignee: []
created_date: '2026-11-09 12:36'
labels:
  - bug
dependencies: []
priority: high
ordinal: 702000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
scripts/lib/example-linux-host.sh built examples with `cargo build --release --example NAME` at the workspace root, selecting every member, so that cargo unified features with extent_store's own dev-dependency on itself with `audit`: the store recounted its state after each critical section in every example run on GCE (sqlite-bench, sqlite-commit-profile, extentd-mapbench, store-read and others). Found by perf on 2026-11-09 (TASK-301): State::audit, KeptExtents::audit and Recount took ~8% of a one-row commit's CPU. Fixed in the same change (-p the example's package). Earlier example measurements in docs/measurements include that cost.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 example-linux-host.sh builds the example's package alone, so that no audit is compiled into a measured example
- [ ] #2 docs/measurements notes of example runs before the fix say they ran with the audit
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
