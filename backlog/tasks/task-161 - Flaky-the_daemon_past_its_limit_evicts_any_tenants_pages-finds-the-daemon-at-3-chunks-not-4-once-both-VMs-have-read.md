---
id: TASK-161
title: >-
  Flaky: the_daemon_past_its_limit_evicts_any_tenants_pages finds the daemon at
  3 chunks, not 4, once both VMs have read
status: To Do
assignee: []
created_date: '2026-11-04 03:57'
labels: []
dependencies: []
priority: medium
type: bug
ordinal: 606000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GitHub CI run 41762577720 (branch t1535-commit-after at 80cd4e16b, which changes no host crate): crates/fxdax/tests/memory.rs:147, assert_eq!(metric("fxdax_extentd_memory_bytes"), 4 * CHUNK) after read_through of a tenant's VM and the daemon's own, under a total limit of 4 chunks: left 6291456, right 8388608. The rerun of the same job passed; macOS and GCE r531 pass it. TASK-132 fixed this test's earlier failure (relief exhausting an arena while another's pages were quarantined); this is another: the daemon holding a chunk less than the two VMs read, whether evicted early, counted late, or one VM's chunk freed and not paged back in.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause found, and the test passing 50 runs in a row on Linux under load
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
