---
id: TASK-26.18.7
title: 'Guest Fxfs: Fxfs per-operation CPU in the in-memory LSM layers and TreeCache'
status: To Do
assignee: []
created_date: '2026-10-15 08:19'
updated_date: '2026-10-20 22:06'
labels: []
dependencies: []
parent_task_id: TASK-26.18
priority: medium
type: enhancement
ordinal: 61000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Natively (fxfs-metabench with perf on GCE), after 089d6fc an unlink still spends about 17% in skip-list searches, 10% in TreeCache invalidation and lookup (LinkedHashMap, value clones), and compaction takes about a quarter of the CPU while it runs. Each replace_or_insert erases and reallocates a node. Shrinking these moves every metadata operation.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fxfs-metabench on GCE shows unlink + graveyard under 12 us per op (15.4 us at 089d6fc)
- [ ] #2 fxfs unit tests pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-15 (1.5x goal, measurement gce-fs-bench-2026-10-15T1720Z): journal records serialized before the writer lock (1460f95); file creates and unlinks leave directory times to the kernel (25ec30e, a2a38cf); unallocated objects tombstoned without an extent walk (927a929); TreeCache reserves no placeholder for missing names (9f95a0c); FxHash maps (ab4fb3d); compaction keeps its new layer cached (c046ce9). apply_transaction under the journal lock is still ~1/4 of create-parallel's CPU.

2026-10-20: fxfs-metabench on c3-standard-8 (us-east1) at 9d58058: unlink + graveyard 17.1 us (gce-fxfs-metabench-base-2026-10-20); fec30d4 (before reflinks) the same on the same host, so reflinks cost nothing here. perf (gce-fxfs-metabench-perf-2026-10-20): flat: transaction commits 37% (applying mutations 24%, the skip list's replace_or_insert 14%, journal serialization 7.5%), the flush task 27% (compaction 18%), the graveyard 9%; malloc/free ~15% spread. replace_or_insert erases and reallocates because lock-free readers may hold the old node.
<!-- SECTION:NOTES:END -->
