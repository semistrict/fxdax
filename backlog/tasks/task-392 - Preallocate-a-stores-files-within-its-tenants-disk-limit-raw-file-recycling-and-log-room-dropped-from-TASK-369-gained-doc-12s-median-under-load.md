---
id: TASK-392
title: >-
  Preallocate a store's files within its tenant's disk limit: raw-file recycling
  and log room, dropped from TASK-369, gained doc-12's median under load
status: To Do
assignee: []
created_date: '2026-11-13 00:02'
labels:
  - performance
  - extent_store
dependencies:
  - TASK-369
priority: medium
ordinal: 769000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-369 tried two kinds of preallocation, which ext4 rewards: a flush's sync of blocks already written commits nothing of the host's journal.
1. **Raw-file recycling:** a segment's raw file, framed in a copy, kept as the next segment's.
2. **Log room:** a log zero-filled past its records, 1 MiB = 4 x held_most, kept by spare and prepared logs too.

**Gains** (ubu1, 3 interleaved rounds against the same build without recycling, /mnt/fxdax-scratch/t369/ab6-pg):
- durable lag within 50 ms: 68.1% vs 55.9% (pgbench), 62.6% vs 51.5% (pgbench-dax);
- doc-12's median failed in 0 of 6 runs with recycling, 2 of 6 without;
- durable_commits_through_the_daemon on a quiet host: nothing (main 39.2-40.6 ms, with 39.6-42.9, without 39.8-42.2).

**Costs:**
- extentd's io per guest byte 1.44-1.48 vs 1.32-1.33: a recycled file's stale pages are written back, where main's raw files were removed first;
- up to a segment (32 MiB) of the tenant's disk per writing store, and 1 MiB per log.

The files are in the tenant's directory, which its limit counts as the daemon walks it, not as it admits writes. So Linux's tenant disk tests failed:
- extentd/tests/hostile.rs a_tenant_filling_its_disk_leaves_others_committing and a_compromised_vm_process_harms_no_tenant_beside_it: mallory past its limit;
- tests/disk.rs tenants_past_their_disk_limits_are_refused_while_others_write: freed data never leaving the disk.

A frugality switch (none kept from 3/4 of the limit until under 1/2, on branch t3/batch-syncs-off-critical-path, commit 0851d88f1) fixed the hostile tests, not disk.rs.

Find a way that keeps the gain within the limit. Options: admit the preallocation against the limit and give it a documented per-store allowance in disk.rs; drop spares once idle; or bound them.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A pgbench/pgbench-dax A/B on ubu1 keeps recycling's durable-lag gain (doc-12's median), io per guest byte within 0.02 of main's
- [ ] #2 Linux's tenant disk tests pass (hostile.rs, disk.rs), and serves_hundreds_of_forks_of_a_base
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
