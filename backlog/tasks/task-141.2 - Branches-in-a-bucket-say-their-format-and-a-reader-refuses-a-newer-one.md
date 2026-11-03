---
id: TASK-141.2
title: 'Branches in a bucket say their format, and a reader refuses a newer one'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-31 20:30'
updated_date: '2026-11-03 13:39'
labels: []
dependencies: []
parent_task_id: TASK-141
ordinal: 489000
---

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Each branch's objects carry a format version
- [x] #2 A test: a daemon refuses a branch of a newer format, naming it, and reads none of it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
With TASK-153.7-9's format change (2026-11-03). doc-11 rule 3: every persistent format says its version, a reader refuses a newer one; the bucket's branches don't.
1. extent_remote::layout::FORMAT, the first field of every branch's record (bincode puts it first, so any later reader reads it before the rest); Record::decode refuses a newer one, Error::Format naming the branch and format. Every reader of a branch reads its record first: a writer taking it, fxdax attaching it, the collector (which stops its pass: it deletes nothing in a bucket it can't read).
2. Framed objects (segments, extents on their own, checkpoints, moves) say their frame's format in their trailer's magic, fxdaxob1: a reader names a newer one rather than calling it corrupt.
3. Leases: the compat decode of leases before tokens goes (AGENTS.md: no compat).
4. Tests: Writer::open and fxdax's attach refuse a branch of FORMAT+1, naming it, the lease untouched; the collector refuses; extent_frame names a newer frame.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Verified: macOS scripts/test.sh at e8e186485: workspace 2051/2051, Shuttle 61, 7, 6, 21, 10, 18, scripts 135 OK; the extent_remote simulation 37/37. GCE fxdax-test-t1537-r1 (us-east4-a, n2-standard-4): Linux workspace 2215/2215 and the same Shuttle suites; its guest stage stopped at a test_cases host test (XFS loop umount busy, no fxdax code), so r2 ran the guest cases alone: fxfs-guest-dax, -live-fork, -migrate-dax, -migrate-tenant-dax, -snapshot, -snapshot-dax, -snapshot-volumes, -snapshot-volumes-dax, -suspend-dax all OK, test_cases 53/53. Both hosts deleted. scripts/upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Branch records say layout::FORMAT first, and framed objects' trailers their frame format; a reader refuses a newer branch with Error::Format naming branch and format, reading and writing nothing else of it (fxdax branches_of_newer_formats_are_refused: bucket unchanged), and the collector stops without deleting (passes_stop_at_branches_of_newer_formats, which found records decoded unchecked there). Verified macOS and GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
