---
id: TASK-183
title: >-
  Two deletions of a branch at once: the second, recovering past a fork segment
  that landed after the first marked the branch deleted, records the fork's
  child after the collector let go of what it needs
status: To Do
assignee: []
created_date: '2026-11-05 10:53'
labels:
  - remote
  - collector
dependencies: []
priority: high
type: bug
ordinal: 638000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by specs/bucket's TLA+ (TASK-165.2, the model BucketForks), once a deletion's recovery was two steps (its checkpoints listed, then the rest), as branch::delete is. Branch S's writer commits a fork of child C and uploads its segment, which is in flight, never acknowledged. Two deletions of S run (a stale placement's and the latest's, as in TASK-178): each ends the lease and reads the record live. The first recovers S to the gap where the fork's segment is to land, finds no forks and marks S deleted. A pass reads S's record deleted and no record of C, so S is not live, and goes on to delete S's segments. The fork's segment lands; the second deletion recovers S past it, finds the fork, and record_fork makes C's record: a live branch whose origin, S's segments to the fork, the pass deletes. Every later pass stops on it (Error::Lost). The collector relies on a deleted branch's children getting their records before it is marked deleted (Collector::records); a second deletion breaks that, since segments its writer never acknowledged land after the lease is ended (end_lease fences acknowledgements, not creates). The spec models the fix as the rule deletionsFence: a deletion's recovery creates an empty segment, of the final epoch, at the end it found (a segment that landed there first applied, the fence after it), as a writer taking over does, so that every deletion of the branch recovers the same segments; mutations/NoDeletionsFence.cfg is the trace. Code: crates/extent_remote/src/branch.rs, delete; recovery.rs; collector.rs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test with two deletions of a branch and a fork segment landing between their recoveries, red with the code as it is: the child recorded after the branch was marked deleted, and a pass losing it
- [ ] #2 The fix is a field of extent_remote's Rules, with a simulation test turning it off, and its TLA+ doc line names specs/bucket/mutations/NoDeletionsFence.cfg
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
