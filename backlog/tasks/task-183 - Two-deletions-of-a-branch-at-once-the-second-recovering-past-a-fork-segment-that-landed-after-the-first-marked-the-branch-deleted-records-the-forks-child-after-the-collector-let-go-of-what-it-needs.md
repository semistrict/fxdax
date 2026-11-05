---
id: TASK-183
title: >-
  Two deletions of a branch at once: the second, recovering past a fork segment
  that landed after the first marked the branch deleted, records the fork's
  child after the collector let go of what it needs
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-05 10:53'
updated_date: '2026-11-05 14:08'
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Rules::deletions_fence: branch::delete, having recovered the branch, creates an empty segment of the final epoch at the end it found; a segment there first is applied (its forks recorded too) and the fence goes after it. Points and claims lost need none.
2. Direct test: a fork segment in flight; two deletions; the first recovers to the gap and marks deleted; a pass; the segment lands (or fails, fenced); the second deletion. Red with the rule off: the child recorded after the pass let go of its origin, the next pass Lost.
3. specs/bucket: deletionsFence becomes a field's rule; the header and doc-7 say so.
4. Simulation, if cheap.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fixed as the spec models it: Rules::deletions_fence. branch::delete, having recovered the branch (not a point, nor a claim found lost), fences it: fence_deleted creates an empty segment of the final epoch (Lease::FINAL) at the end it found, applying a segment landed there first and fencing after it, before it records the forks' children. The create is recovery::fence, which Writer::take's fence now uses too (one place makes and applies an empty segment at a branch's end).
Red/green: collector::tests::deletions_at_once_fence_out_forks_landing_between_them (the fork's upload finds the fence, Err(Fenced); no child record; the next pass deletes the second deletion's fence, Ok(deleted 1)) and deletions_at_once_fencing_out_forks_landing_between_them_need_deletions_fence (rule off: the upload lands, the second deletion records Fork(1 at 2), the next pass fails Lost("branch 2: what its recovery starts from keeps going")). The second deletion waits in a Gated bucket at its checkpoints listing; the pass runs the landing and the second deletion at its second read of gc/lease, before it deletes (Hook::Getting now counts from when the hook is armed).
Spec: deletionsFence is now a field's rule (header: 28 fields, then deletionsDoneByOthers); NoDeletionsFence violates NothingNeededGone in 1s.
Simulation: profile deleted_forking (forks, stale and latest deletions, takeovers, collectors): branches_deleted_twice_while_forking holds 200 seeds; deletions_need_fences finds seed 4505 with the rule off (a fork's child lost: 'branch 1b does not open: lost: ... keeps going', the collector failing likewise).
passes_read_records_and_checkpoints_once_and_delete_at_once now counts the deleted forks' fences among what a pass deletes (10, then 2, where 8 and 1).
Blind spot: the simulation's stale deletions ran only in stale_placements, which forks little, so no fork segment was in flight across two deletions; and the collector's assumption that a deleted branch's children have records before it is marked deleted had no test with two deletions. Class: deleted_forking and the direct test. Look around: other places a branch's end is read without a fence: delete_fork's claim (recovery::holds, read once, its record a claim that lost_is_ok covers), the collector (reads, never relies on an end not moving: may_land keeps segments at or past it), points (no writer). Structure: the deletion's fence makes the deletion a writer taking the branch over for the last time, the one rule that settles a branch's end, now one function for both.
<!-- SECTION:NOTES:END -->
