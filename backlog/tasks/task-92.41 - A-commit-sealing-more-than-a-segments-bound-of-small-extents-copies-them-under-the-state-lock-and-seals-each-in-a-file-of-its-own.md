---
id: TASK-92.41
title: >-
  A commit sealing more than a segment's bound of small extents copies them
  under the state lock and seals each in a file of its own
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-23 03:10'
updated_date: '2026-11-10 20:49'
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Segments framed in place when none of their parts compresses: the raw file, its tail appended and synced, hard-linked into segments/ (Dir::link), its raw path going once its commits are durable, as before; recovery reads a raw file holding a whole frame by its data.
2. Format (extent_frame Contents::continued, layout::FORMAT 3, store format 9): a segment's footer names the runs in its data of the commit it ends inside of, which the next segment goes on with (the user's decision: commits spanning segments).
3. Readers (extent_remote::State::apply_contents) hold a commit's runs begun until a segment ends it, dropped if the next applied neither continues nor ends it (a fence); writers acknowledge only at commit boundaries, so checkpoints and the acknowledged state never hold one begun; Wal::rebase lands only at boundaries; fetch reads continued runs.
4. The store: a segment filling between commits takes no more writes and waits (Phase::Full, its window kept) for the next commit, which seals what is one run in it in place, closes it with its continuation, the commit's runs there recorded in its log; copies that overflow the open segment fill it and go on in the next; the spill to raw files of their own goes. close_now closes the first full segment if it holds commits. Upload and raw removal wait for the continued commit's durability; local recovery drops a continuation no later segment completes.
5. Tests: extent_frame, extent_remote (apply, rebase, acknowledge), extent_store (bursts sealed in place, crash rounds through spanning commits, sim, Shuttle); store-bench burst before/after.
6. doc-7, crate docs; measure on ubu1 (shared-disk, -dax, pgbench, pgbench-dax) A/B vs main.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
User decision (2026-10-23): commits spanning segments (footers keep naming only their own segment's runs; a commit's runs may continue into the next segment), not footers naming runs in other objects.

Measured for the TASK-165.9 agent (2026-11-08, TASK-221's agent): why a commit's sealing copies what it copies, under pgbench. Instrumentation: branch measure-92.41 (1ebc85922 and the next), a debug stat: with FXDAX_SEAL_STATS set, each commit that copies anything logs, by why, how many extents and bytes it copied (crates/extent_store/src/seals.rs; Segments::stage). The kinds, from each sealed extent's pieces, by the segment each is in: retired (some in segments set aside, full, since the last commit, holding no commit's data: TASK-79.2), framing (some in segments closed since the last commit, being framed), committed (some in segments closed and framed as of earlier commits: written over or grown since sealed; with how many bytes new in the open segment), and, all in the open segment: interleaved (in order, other writes between its pieces), rewritten (pieces out of order there: written over, or out of order, since the last commit), holes, adjacent. land-k6's code (write-behind, 981d28456 reverted), GCE fxdax-test-r956-k6, us-central1-b, jailed, 1 round each.
r961 (by kind), pgbench: 6603 commits copied something, 2859 MiB in all, mean 443 KiB; 22524 extents were one run already (not copied). Copied: interleaved 6067 extents, 1480 MiB (52%); retired 2713, 860 MiB (30%); committed 656, 416 MiB (15%), of which 9.6 MiB new (2.3%: a few KiB written over a committed extent copies it whole, up to 1-2 MiB); rewritten 1219, 77 MiB (3%); framing 211, 25 MiB (1%); holes 2. Per commit: p50 24 KiB, p90 1 MiB, p99 3.2 MiB, max 206 MiB; 42 commits past 8 MiB, which are the tail's (TASK-221: a commit copying 97 MiB, its flush 380 ms, a commit durable 396 ms after ordered, r958): in them retired 47% (2713 extents), interleaved 31%, committed 22%. The largest: 211 MB (703 retired extents, 196 MB: six segments' worth set aside since the last commit; committed 14, interleaved 2), 210 MB (351 retired, 196 MB), 206 MB (879 retired, 197 MB), 132 MB (352 retired, 97 MB; committed 31), 127 MB (332 retired, 98 MB). pgbench-dax: 7608 commits, 2975 MiB; interleaved 60%, retired 23%, committed 10% (4.2% new), rewritten 5%, framing 2%; 55 commits past 8 MiB: interleaved 43%, retired 39%, committed 16%; the largest 205 MB (873 retired, 197 MB), 181 MB (353 retired, 98 MB; committed 81, 83 MB), 159 MB (304 retired, 99 MB).
So: the biggest commits are the guest writing more than a segment (32 MiB) between commits, whose segments are set aside full and copied out whole at the commit (this task's case, ~97 or ~196 MB: three or six segments); the steady volume is extents interleaved with others' writes in the open segment (half of all bytes copied, 1 MiB extents written in pieces with other extents' writes between); overwrites of committed extents are a sixth or less, but copy ~40 times what changed. The copied extents are 1 MiB at most in nearly all (Fxfs's extent size here); none large (4 MiB), none own. r960 had committed and retired together (45-47%; its 'committed, new 0' commits were retired ones).

2026-11-10 (TASK-310): on ubu1's shared-disk case (fxfs-guest-shared-disk: 16 VMs 8 at a time, each 768 MiB of 1 MiB extents written before one fsync), this case was the daemon's double write: every segment filled was set aside and the fsync's commit copied ~1,300-1,600 extents out, ~790 MB, mostly into files of their own (own 10.55 GiB of 12.37 GiB guest data, besides the raw 12.49). TASK-310 now spills: once a segment is set aside, extents opened until the commit go to raw files of their own (at most Options::spilled_most, 2,048), which the commit frames in place if they don't compress (renamed into extents/, tail appended): copying nothing for them (own 1.2 GiB). Still a file per extent: the commit's flush and the framing of the segment it closes sync ~800 own files per VM, 7-25 s with 8 VMs at once (flush data phase, framing own phase), which this task's commits spanning segments would remove.

2026-11-10 (claude): implemented on branch t9241 (a5dd824c5, 27cdd97fc, ca1c208b7 after rebase), with TASK-310's in-place framing of segments.
Design: a segment that fills between commits takes no more writes and waits (segments::Phase::Full) for the next commit, which seals in place each extent one run in it and closes it, its footer naming those runs as the commit's (extent_frame Contents::continued, the commit's tag); the commit ends in the open segment. Copies past the open segment's room fill it and go on in the next: no own file for want of room; the spill (80e682ed5) and Options::spilled_most are gone. Each segment's log holds its part (a record of its continued runs, checked or held). Readers hold continued runs (State::begun) and seal them with the ending segment, dropped at a fence; Writer::acknowledge stops at segments ending commits (no checkpoint mid-commit); Wal::rebase lands only between commits; local recovery keeps a continuation only if consecutive segments complete it, and closes the first again without it otherwise. Upload and raw removal wait for the continued commit's durability. A filled segment holding commits waits too; the uploader's interval leaves a segment the guest is filling (filled, or the open one past 8 MiB written since the last commit) for its next commit up to upload::Options::filling_wait (30 s); waiters close at once. Formats: branches 3, store 9. Decisions in OPEN_QUESTIONS.md.
Tests: extent_frame continued_commits_end_in_their_writers_segments; extent_remote commits_spanning_segments_seal_once_ended (red acknowledging mid-commit), logs_of_commits_spanning_segments_land_whole (red landing mid-commit); extent_store segments_filled_wait_for_the_next_commit, bursts_past_a_segment_between_commits_are_sealed_where_they_are (red: 516 KB copied without in-place sealing in full segments), copies_past_the_open_segments_room_go_on_in_the_next, interval_closings_leave_segments_being_filled, commits_spanning_segments_are_kept_whole_or_not_at_all, segments_going_on_with_acknowledged_commits_go (red: the cut openings' error, below); Shuttle scenarios uploading_what_a_segment_filled_holds and uploading_what_a_spanning_commit_closed (needs_framings_to_end_themselves on it). The crash rounds' OPTIONS now have extents small enough that segments fill between commits (64 MiB extents: none ever filled); they count commits spanning segments: >300 in the prefix rounds, 31 in the kept rounds.
Found by the crash tests during the work (not on main): openings_cut_at_each_write_open_again (kept rounds, racy) hit a resurrected segment going on with a commit the base had acknowledged, taken for unended, recovery opening its gone raw file: acknowledged segments now skip the continuation check. Blind spot: the deterministic prefix rounds don't acknowledge; the unit test reproduces it deterministically. Structure: one place (wal::recover) decides a continuation's fate, by consecutive segments; fine.
store-bench burst (256 MiB of 256 KiB extents after a commit, ubu1, fxjob --bench, 3 runs each): main ordered 153-175 ms, durable 405-741 ms, framed 447-782 ms, 640 extents in own files, 96 MiB copied (main needs ulimit -n 65536: TASK-332); branch ordered 21-25 ms, durable 52-56 ms, framed 166-172 ms, no own files, nothing copied, 3 MiB framed (tails and the open segment's compressed copy).
<!-- SECTION:NOTES:END -->
