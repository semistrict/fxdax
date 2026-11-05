---
id: TASK-149.26
title: >-
  Scenarios for the mutants no seed finds: task-153.3-lag-under-a-pte and the
  rest
status: Done
assignee: []
created_date: '2026-11-03 12:42'
updated_date: '2026-11-05 03:07'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 570000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
guest/sim/mutants.sh at TASK-150.21 (patch 0012) leaves six mutants uncaught in 3,000 seeds: patch-0008-pmd-entry-not-split, task-150.14-unlisted-bits, task-153.1-fault-limit, task-153.3-refused-slice-stale, task-153.6-lag-past-the-end, and task-153.3-lag-under-a-pte, which interleave 7289 200 found before patch 0012 and no seed of 3,001 to 15,000 at 200 operations finds after: patch 0012's unmaps under the entry lock moved the interleavings. A catch that rests on one seed's interleaving is lost to any change that moves seeds; each needs steps played out (dax_pause), or a generator that reaches its interleaving often. task-79.28-range-too-small is equivalent (README).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each of those mutants fails a guest/sim scenario or a seed found in under 3,000
- [ ] #2 guest/sim/README.md's mutant table says how each is found
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Obsolete: made so by the deletion of writable DAX (37c331a6c). Five of the six mutants went with the code they mutated: patch-0008-pmd-entry-not-split (patch 0008 deleted), task-150.14-unlisted-bits, task-153.3-refused-slice-stale, task-153.6-lag-past-the-end and task-153.3-lag-under-a-pte (lagging pages and refused slices deleted); none is in guest/sim/mutants any more. The sixth, task-153.1-fault-limit, is recorded as equivalent under copy-on-write in guest/sim/README.md:186 (nothing else drops a fault's slice for ever; with TASK-140's bug, gone with writable DAX, it was a livelock), as task-79.28-range-too-small is at README.md:189. Nothing was run here, so the ACs stay unchecked.
<!-- SECTION:FINAL_SUMMARY:END -->
