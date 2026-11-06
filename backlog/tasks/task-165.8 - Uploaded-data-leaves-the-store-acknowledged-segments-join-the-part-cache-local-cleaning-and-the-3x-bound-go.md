---
id: TASK-165.8
title: >-
  Uploaded data leaves the store: acknowledged segments join the part cache;
  local cleaning and the 3x bound go
status: To Do
assignee: []
created_date: '2026-11-05 03:16'
updated_date: '2026-11-06 06:56'
labels: []
dependencies:
  - TASK-165.4
  - TASK-165.7
documentation:
  - doc-14
parent_task_id: TASK-165
priority: high
ordinal: 630000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 8. Two cleaners (Store::clean_behind and the collector) and a bound on local dead space (TASK-79.89) exist because the store keeps every segment after it is uploaded. Once acknowledged segments move into the part cache, the collector is the only cleaner and local disk is the WAL, the open segment and the cache. Lands in one batch with TASK-165.7.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An acknowledged segment and its own extents move into the part cache (Cache::adopt) and their extents are read through the fetcher; nothing else of them stays in the store
- [ ] #2 Store::clean, clean_behind, cleaning_due, the files bound and their callers (host_store, extentd's cleaning behind commits) are gone; the extentd cleaning tests assert the collector compacting what a guest freed
- [ ] #3 Views read extents freed after their commit from the WAL or the bucket, and a view whose extent the collector reclaimed is cut off (a test)
- [ ] #4 Local disk stays within the lag bound, the open segment's room and the cache's capacity under a churning guest (a test)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented on branch task-165-7, one commit after TASK-165.7's (doc-14: they land together). Decisions: OPEN_QUESTIONS.md, "Acknowledged segments leave the store".
- extent_store: Store::acknowledged(number, key) replaces Store::uploaded: flushes the commits so far, records Record::Acknowledged { segment, key, own } in the index (format 7), turns the segment's sealed extents and the own extents its footer names (held or kept) into Data::Remote, and their files leave: Cache::adopt (renamed, Dir::host_path) when Store::upload was given the uploader's cache, else removed; the segment's once no write is in progress in it (Segments::reap_acknowledged). Gone: Store::clean, clean_behind, cleaning_due, cleaning_changes, Cleaned, Victim, PartCounts, the 3x files bound (FILES_PER_CAPACITY, Space::check_files, State::file_bytes/garbage/taken_back, keep_for_uploader), cleaning's copies (Copied, make_moves, copy_out), raw-file accounting, Usage::dead_bytes, Rules commits_wait_for_copies, moves_into_the_open_segment, raw_files_count_as_writes_begin; store-bench's cleaning case.
- fxdax: the uploader calls Store::acknowledged with layout::segment(branch, seq) and gives Store::upload its cache; host_store's clean_after_commit/cleaning_due/clean_behind, Kept::committed's cleaning, ExtentStore cleaning_*/dead_bytes, extentd's cleaning behind commits (CLEANINGS, "extentd clean" threads) gone. A follower whose fetch finds its extent Lost (the collector reclaimed it) gets ESTALE.
AC evidence: #1 frees_kept_in_segments_acknowledged_hold_no_file, uploaded_segments_are_the_stores_files (the store's files leave whole into the cache; reads served from the cache with the bucket's objects deleted), views_read_frees_acknowledged_in_the_bucket. #2 what_guests_free_the_collector_compacts (extentd: no segment left locally once uploaded; collector Report { deleted: 2, compacted: 2 }; extents read where they moved). #3 views_read_frees_acknowledged_in_the_bucket, followers_are_cut_off_once_the_collector_reclaims_what_they_read (ESTALE after a pass that deleted 65 objects). #4 acknowledged_segments_leave_a_churning_guests_files (store level: no data file left after each acknowledgement, 100 cycles of a pinning churn); not under a guest VM, which needs GCE.
Tests deleted: extent_store cleaning, cleaning_in_memory, cleaning_on_the_host, cleaning_behind_the_commits, cleaning_between_commits_bounds_what_segments_hold, cleaning_changes_move_with_what_may_make_cleaning_due, cleaning_copies_no_flipped_byte, cleaning_counts_the_disk_each_part_takes, cleaning_finds_the_extents_sealed_in_its_segments, cleaning_is_due_for_the_disk_it_takes_back, cleaning_is_due_for_what_it_takes_back_alone, cleaning_is_due_once_garbage_is_past_the_capacity, cleaning_takes_back_the_most_disk_for_what_it_copies_first, cleaning_takes_no_wholly_live_segment, cleaning_takes_not_the_commits_lock, garbage_is_past_the_capacity_for_the_disk_it_takes, parts_count_what_no_extent_is_in_part_by_part, segments_cleaning_closed_for_room_go_to_the_uploader_with_no_commit_after, the_write_a_commit_comes_with_takes_what_the_commit_takes_back, files_past_three_times_the_capacity_refuse_writes, cleaning_at_its_pace_keeps_a_churning_guest_from_the_bound (replaced by acknowledged_segments_leave_a_churning_guests_files); Shuttle cleaning_during_a_fork_by_pct, cleaning_during_closings_by_pct, writer_cleaner_and_uploader_randomly/_by_pct, writes_racing_at_the_bound_by_pct, needs_commits_to_wait_for_copies, needs_moves_into_the_open_segment, needs_raw_files_counted_as_writes_begin (their rules gone); fxdax stores_clean_once_their_segments_hold_much_freed, stores_clean_behind_their_commits_once_much_is_freed, stores_are_cleaned_behind_their_guests_commits (replaced by what_guests_free_the_collector_compacts). Renamed: what_an_uploader_keeps_counts_not_towards_the_bound -> what_an_uploader_keeps_stays_until_acknowledged, frees_an_uploader_keeps_count_not_towards_the_bound -> frees_kept_in_segments_acknowledged_hold_no_file.
Tests rewritten: rewrites_past_the_capacity_count_as_used, freed_space_is_dead_space_taking_no_room, dead_space_takes_none_of_the_room_writes_need, dead_space_the_uploader_lets_go_takes_none_of_the_room, closed_segments_hold_the_branchs_history, views_read_the_commit_they_are_of (no cleaning); Shuttle run_with reads through a modelled bucket (Bucket) once acknowledged; crash rounds commit where they cleaned; fxdax uploaded_segments_are_the_stores_files, uploads_done_close_no_segment_for_commits_no_one_waits_for (count the bucket's segments), hundreds_of_writers_flush_on_threads_they_share (no cleaning pool); handover's daemon keeps its branches in a file bucket, which the other process reads (ordered_commits_are_whole_in_another_process_once_their_vm_goes failed with EIO on memory://; 11 s on Linux with the file bucket, as before).
Runs at 42e484d75: macOS FXDAX_TEST_PARTS="workspace shuttle sim" scripts/test.sh --no-fail-fast all green (workspace 2024/2024, sim 47/47, shuttle 38/38 + 6 + 6 + 32 + 10 + 18); SIM_SOAK=100 sim::everything passed (243 s); Linux (OrbStack rust:1) extent_store+extent_remote+fxdax 829/835, failing only eu-stack (3) and grants ENOENT (3). GCE not run (no credentials): forks, snapshots, volumes, migration and live fork guest suites must run there before landing.

Rebased onto main 752515020 (land-l landed): 8fa81ef66 (Data::Remote), bb8c0f4e2 and 42db93a24 (TASK-165.7), b447c610b (TASK-165.8); hashes above are before the rebase. At b447c610b: macOS FXDAX_TEST_PARTS="workspace shuttle sim" scripts/test.sh --no-fail-fast green (workspace 2027/2027, sim 47/47, shuttle 38+6+6+32+10+18); Linux extent_store+extent_remote+fxdax 834/840, failing only eu-stack (3) and grants ENOENT (3).

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**Acknowledged segments leave the store, recorded in its index; nothing cleans a store but the bucket's collector (TASK-165.8, 2026-11-05)**

Once the uploader's writer acknowledges a segment (`Store::acknowledged`), its sealed extents, and those stored on their own its footer names, held or kept, are `Data::Remote` at their places in the bucket, and their files leave the store: renamed into the host's cache (`Cache::adopt`) if the uploader has one, else removed; the segment's once no write is in progress in it. `Store::clean`, `clean_behind`, `cleaning_due`, `cleaning_changes`, the bound of three times the capacity on the branch's files, `Usage::dead_bytes`, extentd's cleaning behind commits (`CLEANINGS`) and images' cleaning after commits are gone. Decided where doc-14 left it open:
- **The acknowledgement is a record of the index** (`Record::Acknowledged { segment, key, own }`), written after the commits durable so far, which it makes durable first, so that the store opened again holds those extents in the bucket, not in files it removed. Local format 7 (TASK-165.7's) gains the record; no store of format 7 predates it. TASK-165.9 removes the index, and with it this record. *Instead:* leaving the files until the next checkpoint covers the change, which keeps a segment's worth more on disk per commit. *Undoing it:* `Record::Acknowledged`.
- **A store with no uploader keeps everything**: unattached, nothing is acknowledged and nothing leaves; what its segments hold that no extent is in stays until no extent is in them. Every store has a bucket (TASK-165.6) and fxdax attaches an uploader to each it opens, so only extent_store's own tests and fxdaxctl's offline commands run so. *Instead:* local cleaning kept for unattached stores, a second cleaner doc-14 removes.
- **A follower whose view's extent the collector reclaimed is cut off** (`ESTALE`): the fetch finds it `Lost`, which for a writer stays `EIO`. A follower reads what the host's cache holds of it until then, whatever the collector did.
- **A store forked where no daemon serves it attaches an uploader to the bucket its description names** (`ExtentStore::kept_in_bucket`, as an image's is attached to its own): fxfs-guest-import forks a base it imported offline, which had none, and the fork failed (`ENOTSUP`, GCE r701). The rule stands where the fork is made (`host_store::fork` refuses a store with no uploader, `forks_with_no_uploader_make_nothing`); its callers attach one first. fxdaxctl's local `fork` attached one already. *Instead:* the guest case attaching it itself, which every other offline forker would have to remember.
- **An in-memory bucket (`memory:///`) is its process's**: what it acknowledged has left the store, and is gone once the process exits. Tests across processes, and development across a daemon's restarts, use a file bucket (README, Multi-node only); handover's and fxdaxctl's import-then-fork tests do. *Instead:* keeping acknowledged data locally for memory buckets, a second behaviour for test buckets alone.
- **Local disk's bound is the uploader's lag bound, the open segment's room and the cache's capacity**, shown at the store's level (`acknowledged_segments_leave_a_churning_guests_files`, a guest freeing all but two blocks a segment as it goes: no file left after each acknowledgement), not under a guest VM, which GCE runs.
<!-- SECTION:NOTES:END -->
