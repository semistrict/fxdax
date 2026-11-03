---
id: TASK-160.22
title: 'Fxfs: one rule for what DAX may write in place, and DAX in the model test'
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 593000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Three places decide whether blocks may be written in place, by different rules: extents_in (open and unshared, whatever the checksum mode), txn_map_write_keeping (ExtentMode::Raw too), overwrite (unshared). Latent, unproven: Fxfs's growth zeroing writes the tail with multi_write, a checksummed extent left open, which on a DAX file extents_in hands out writable, the window's stores leaving its checksums stale (TASK-153.4 widened where that zero happens, to clones onto DAX files). The model test has no DAX operation. Proposal: one ExtentAccess decision; DaxWrite and DaxExtents ops in the model.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A red test: a growth zeroes a DAX file's tail, then extents_in doesn't say Open
- [ ] #2 One predicate at all three sites
- [ ] #3 Model hunts with DAX operations
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
