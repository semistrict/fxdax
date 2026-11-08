---
id: TASK-92.41
title: >-
  A commit sealing more than a segment's bound of small extents copies them
  under the state lock and seals each in a file of its own
status: To Do
assignee: []
created_date: '2026-10-23 03:10'
updated_date: '2026-11-08 06:52'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 325000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-79.2 bounded segments at segment_size + max_extent. A commit sealing more small extents than that, written since the last commit, copies those left in closed or set-aside segments, under the state lock, into the open segment, and seals the rest in one own file each. store-bench burst (256 MiB of 256 KiB extents after a commit): ordered in about 210 ms, durable in about 2.5 s with 640 own files, each its own create, frame, fdatasync and PUT; main sealed them in place in one unbounded segment. With one footer per commit naming runs only in its own segment, avoiding it needs a format change: commits spanning segments, or footers naming runs in other uploaded objects, which touches extent_frame, extent_remote recovery and the collector.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A commit after a burst larger than a segment seals the burst's extents where they are, copying nothing and making no file per extent
- [ ] #2 store-bench's burst case shows it before and after, and object-storage recovery and the collector handle the new format, with tests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
User decision (2026-10-23): commits spanning segments (footers keep naming only their own segment's runs; a commit's runs may continue into the next segment), not footers naming runs in other objects.

Measured for the TASK-165.9 agent (2026-11-08, TASK-221's agent): why a commit's sealing copies what it copies, under pgbench. Instrumentation: branch measure-92.41 (1ebc85922 and the next), a debug stat: with FXDAX_SEAL_STATS set, each commit that copies anything logs, by why, how many extents and bytes it copied (crates/extent_store/src/seals.rs; Segments::stage). The kinds, from each sealed extent's pieces, by the segment each is in: retired (some in segments set aside, full, since the last commit, holding no commit's data: TASK-79.2), framing (some in segments closed since the last commit, being framed), committed (some in segments closed and framed as of earlier commits: written over or grown since sealed; with how many bytes new in the open segment), and, all in the open segment: interleaved (in order, other writes between its pieces), rewritten (pieces out of order there: written over, or out of order, since the last commit), holes, adjacent. land-k6's code (write-behind, 981d28456 reverted), GCE fxdax-test-r956-k6, us-central1-b, jailed, 1 round each.
r961 (by kind), pgbench: 6603 commits copied something, 2859 MiB in all, mean 443 KiB; 22524 extents were one run already (not copied). Copied: interleaved 6067 extents, 1480 MiB (52%); retired 2713, 860 MiB (30%); committed 656, 416 MiB (15%), of which 9.6 MiB new (2.3%: a few KiB written over a committed extent copies it whole, up to 1-2 MiB); rewritten 1219, 77 MiB (3%); framing 211, 25 MiB (1%); holes 2. Per commit: p50 24 KiB, p90 1 MiB, p99 3.2 MiB, max 206 MiB; 42 commits past 8 MiB, which are the tail's (TASK-221: a commit copying 97 MiB, its flush 380 ms, a commit durable 396 ms after ordered, r958): in them retired 47% (2713 extents), interleaved 31%, committed 22%. The largest: 211 MB (703 retired extents, 196 MB: six segments' worth set aside since the last commit; committed 14, interleaved 2), 210 MB (351 retired, 196 MB), 206 MB (879 retired, 197 MB), 132 MB (352 retired, 97 MB; committed 31), 127 MB (332 retired, 98 MB). pgbench-dax: 7608 commits, 2975 MiB; interleaved 60%, retired 23%, committed 10% (4.2% new), rewritten 5%, framing 2%; 55 commits past 8 MiB: interleaved 43%, retired 39%, committed 16%; the largest 205 MB (873 retired, 197 MB), 181 MB (353 retired, 98 MB; committed 81, 83 MB), 159 MB (304 retired, 99 MB).
So: the biggest commits are the guest writing more than a segment (32 MiB) between commits, whose segments are set aside full and copied out whole at the commit (this task's case, ~97 or ~196 MB: three or six segments); the steady volume is extents interleaved with others' writes in the open segment (half of all bytes copied, 1 MiB extents written in pieces with other extents' writes between); overwrites of committed extents are a sixth or less, but copy ~40 times what changed. The copied extents are 1 MiB at most in nearly all (Fxfs's extent size here); none large (4 MiB), none own. r960 had committed and retired together (45-47%; its 'committed, new 0' commits were retired ones).
<!-- SECTION:NOTES:END -->
