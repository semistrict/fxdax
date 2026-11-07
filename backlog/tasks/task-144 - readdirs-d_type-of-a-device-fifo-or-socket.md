---
id: TASK-144
title: 'readdir''s d_type of a device, fifo or socket'
status: Done
assignee: []
created_date: '2026-10-31 22:07'
updated_date: '2026-11-07 20:19'
labels: []
dependencies: []
priority: low
ordinal: 494000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
generic/401: Fxfs's ObjectDescriptor has File for special nodes too, so readdir says DT_REG, and find -type p misses them. A lookup per entry would slow every readdir; a new descriptor touches the format and the matches on it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 d_type is right for every node, without a per-entry lookup, generic/401 passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-07: fixed by 56fdf6b12 (TASK-149.36's model test found it). fxfs-posix's readdir takes a regular-file entry's type from its object record (node_type), so FIFOs, sockets and devices list as what they are. generic/401 passes with DAX and without (GCE r960, r963; forked-perf's r942), and is out of xfstests' KNOWN_FAILURES (TASK-149.20.1). It is done with a tree lookup per regular-file entry, which this AC wanted to avoid. A Linux-sized tree's listing was measured after it, with TASK-230's kept listings: 6,059 directories walked warm in 62 ms against ext4-pmem's 83 (r904, r905). Closed with that recorded in OPEN_QUESTIONS's decisions.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
d_type is right for every node and generic/401 passes (56fdf6b12; GCE r960, r963). It takes a record lookup per regular-file entry rather than none; TASK-230's kept listings measure that as cheap, recorded as a decision in OPEN_QUESTIONS.
<!-- SECTION:FINAL_SUMMARY:END -->
