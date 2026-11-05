---
id: TASK-174
title: >-
  A deleted root's record the collector drops is made again, as a fork's child,
  by its parent's slow writer, naming segments collected
status: To Do
assignee: []
created_date: '2026-11-05 07:38'
labels: []
dependencies: []
priority: high
type: bug
ordinal: 634000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by specs/bucket's TLA+ (TASK-165.2), the model BucketForks, before its fix was modelled. Branch S's writer forks child C (an ID a guest chose); another branch takes C's ID (branch::create) and is deleted (its record deleted, kind Root). S's writer acknowledges the fork (its lease read before S's deletion ends the lease) but is slow to record C. S's deletion recovers S, finds the fork, record_fork finds C's record deleted and stops; S is marked deleted. A pass drops C's record (Record::may_drop: a deleted root's) and S's lease and segments, nothing live needing them. S's slow writer's record_forks then creates C's record (Kind::Fork, origin S at the fork): a live branch whose recovery reads segments the pass deleted. Every pass stops on it (Error::Lost) for good. doc-7 keeps deleted forks' records for this reason (a parent's writer slow to record a fork could make the record again); a deleted root's record, whose ID a fork named, is the same case. The model keeps every record (specs/bucket/Bucket.tla, GcFinish's doomedKey). Fixes possible: keep deleted roots' records too (their number is bounded by the branches made, as forks' is), or drop one only once no writer that could name it as a child can still record. crates/extent_remote/src/record.rs, Record::may_drop; collector.rs, doomed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test that makes C's record again after the pass dropped it with the code as it is, and finds no record made again after the fix
- [ ] #2 The rule the fix adds is in extent_remote's Rules, with its simulation test and specs/bucket's mutation
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
