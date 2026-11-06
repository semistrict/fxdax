---
id: TASK-165.9
title: >-
  One recovery: no index; the open segment's commit log; opening by lease,
  bucket recovery and the WAL; the uploader in extent_store
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-05 03:17'
updated_date: '2026-11-06 12:56'
labels: []
dependencies:
  - TASK-165.5
  - TASK-165.8
documentation:
  - doc-14
parent_task_id: TASK-165
priority: high
ordinal: 631000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 9. The last of the second engine: the local index (checkpoints and deltas) and the marker, Plan, diff and adopt that reconcile it with the bucket. Afterwards a store directory is its description, its open segment with its log, and its WAL, and opening one is the same whether the branch was here, elsewhere or never anywhere.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A commit is durable locally when the flush has synced the open segment and appended its record to the open segment's log; index.rs and the delta codec are gone
- [ ] #2 Opening a store takes the lease, recovers the branch, closes a segment left open from its log, and rebases the WAL; a diverged WAL refuses (ESTALE) until fxdaxctl take drops it
- [ ] #3 remote.rs keeps only the host's configuration, budgets, runtime and errno mapping; the uploader is extent_store's; Marker, InFlight, Plan, diff, take, adopt, spend and snapshot are gone
- [ ] #4 The crash rounds recover a prefix at least as late as the last durable commit with the disk, and at least as late as the last acknowledged without it; Shuttle scenarios cover the uploader acknowledging while commits, closes and views race
- [ ] #5 The audit feature asserts at every close that the live state equals the replay of the segment's footer over the state before it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Commits on task-165-9, each compiling and passing; refactors apart from behaviour.
1. extent_frame: restamp(object, footer), a framed object's tail anew over the same parts (additive).
2. Refactor, footers unchanged: the open segment's Window folds each commit's Contents (what its log will record), one fold for closing and for recovery.
3. Refactor, behaviour unchanged: the uploader (Remote, Failure, Lease, UploadStatus, Options) moves from fxdax/src/remote.rs into extent_store (upload.rs); remote.rs keeps Host, host_token, budgets, runtime, Fetcher, errno mapping.
4. One recovery (format 8): index.rs, the delta codec, Checkpoint, Change::Adopt, adopt/spend/snapshot/rebuild/release/read_kept/unreleased, Uploads' diff keeping, Marker, InFlight, Plan, diff, take, take_branch, start_point's take go. Local disk: store (description: format, block size, branch, bucket, capacity as the default before a commit sets one, token), open-segments/<n> raw + logs/<n> (each commit's Contents, hashed, appended and synced by the flush), segments/<n> and extents/<seq> the WAL (framed, framings ordered), wal (A/B slots: the WAL's base, written before acknowledged files leave). Opening: take the directory, close left-open segments from their logs into the WAL, check the WAL follows the branch as recovered (no fence if plainly diverged), take the lease (Writer::open), Wal::rebase (restamps in memory; uploads re-tail with extent_frame::restamp), base written, landed files into the cache, state = bucket recovery + the WAL's footers by State::apply_contents. Points: recovery, no lease, no WAL. Diverged -> ESTALE until fxdaxctl take drops the WAL. A child's directory is its description; opening it recovers it. Store::create/open of extent_store's tests open where the bucket holds nothing of the branch (an empty Opening), which refuses a WAL that acknowledged anything (Diverged). Flushes wait for framings through their last commit.
5. TASK-192: a store closed uploads its open segment (Uploader stops after closing, framing and acknowledging it); TASK-92.5's take-back flows rewritten to the production timing.
6. Audit (AC #5): at every close the live state equals the replay of the footer over the state at the segment's opening.
7. Crash safety: crash rounds against a memory bucket (with and without the disk), Shuttle scenarios of the uploader's take/acknowledged racing commits, closes and views, a test cutting writes at each point of the open path; the simulation's crash profiles.
8. specs/bucket checked (restamps not durable: what Opened reads of a WAL is its commits); doc-14/OPEN_QUESTIONS decisions; notes.

9. TASK-196 taken in (the user, 2026-11-06): a store directory is whole only once its maker removes its 'unmade' marker (written before its description); opening refuses a marked store; ImportImage and CreateStore remake what an import or create left unmade. Its test (cherry-picked from land-w a7f9706bd) un-ignored.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Step 3 as planned (a pure move of the uploader) was dropped: remote.rs's uploader tests (gather, InFlight, the marker) reach its internals through fxdax's ExtentStore, so the move would have made them public or rewritten them twice; the uploader moves in step 4, rewritten over the WAL, with its tests.

2026-11-06, the design commit 1597b18aa (TASK-165.9 with TASK-196 and TASK-192), on task-165-9 before its rebase onto main:

- Steps 4-8 of the plan went in one commit: the uploader could not move apart from the index's removal (its tests reach its internals), and the audit, crash safety and TASK-192 change the same code. Refactors stayed separate (74f68b93d restamp, 0f3979ad1 Window::fold).
- AC #1: index.rs and the delta codec are gone; a commit is durable once the flush syncs the open segment and appends its record (its footer's Contents, hashed) to logs/<b>/<n>; a framed segment is its commits' record. Local format 8.
- AC #2: upload::open probes, reads the record, checks the WAL follows (Diverged, ESTALE, before the lease), takes the lease (Writer::open/open_recovered), recovery closes again a segment left open from its raw file and log, and Recovering::open rebases the WAL (Wal::of, Wal::rebase), writing the base before landed files leave. fxdaxctl take drops a diverged WAL (host_store::take).
- AC #3: remote.rs keeps Host, host_token, budgets, runtime(), Fetcher and errno(&Failure); the uploader is extent_store::upload (Remote, Failure, Lease, UploadStatus, Options, open, delete_branch). Marker, InFlight's marker, Plan, diff, take, adopt, spend, snapshot, rebuild, release, unreleased gone.
- AC #4: crash rounds as before (300, with the disk) plus tests/crash/kept.rs: 100 rounds of a store kept in a bucket, its uploader acknowledging as the workload goes, crashing at random; taken from the bucket alone it holds a prefix at least as late as the last acknowledged, opened from its disk one at least as late as the last durable (51 rounds had acknowledged commits, 96 held commits the bucket lacked); and 30 rounds of openings cut at each of their writes (897 cuts), each opening again to at least the last durable. The cut test found a real bug: recovery refused a WAL whose whole framed segment followed one it closed again (the framed file's entry not durable); recovery now drops what follows the first segment not whole whose log holds commits, durably, first. Shuttle: 34 tests, the uploader's acknowledgements racing commits, closes and views (shuttle/main.rs's Bucket model).
- AC #5: State::closed replays each footer over the state before (State<Lengths>) and asserts extents, root, spent and capacity at every close (audit feature, on in all extent_store tests).
- TASK-196: Store::make writes 'unmade' first; made() removes it; recover refuses (Unmade, ENOENT); ImportImage and CreateStore remove an unmade store and make it anew; a_store_whose_importer_died_is_made_anew un-ignored and passing.
- TASK-192: ExtentStore::drop stops the uploader first (closes, uploads, stops), then closes the store. dropped_stores_upload_their_open_segments_commits: red with the old order, green now. TASK-192 lives on task-141-3, not main: its notes can't be added here; TASK-92.5's take-back flows are extentd's a_zygotes_restores_open_its_point_once (object storage 50 ms away) and restores_fork_their_point_anew_once_another_writer_took_its_branch, green on Linux.
- fdatasync's wait (real GCS r740-r744, 130 ms against 16 ms): unchanged. COMMIT_DURABLE still waits for the upload and its acknowledgement (extents.rs make_durable); making it local is TASK-165.11's.
- Also found and fixed: a store whose WAL base names acknowledged commits opened against a bucket without its branch made the branch anew, losing them (now NoBranch, ENOENT; stores_whose_bucket_lacks_their_branch_refuse, red without the guard); the lag in bytes now counts the commits sealed in the open segment; the store's next sequence after opening is past what it holds (fxfs's orphan search), not the floor-stamped footers'; the WAL base file is opened per write (a file a fork fewer).
- Decisions: OPEN_QUESTIONS.md, the TASK-165.9 entry. doc-14: implementation notes under its layout and its commits sections. specs/bucket: Opened's comment says the WAL's restamps are in memory, equivalent for the model; no rule changes.
- Tests gone with the code they tested: index.rs's 8 (checkpoints, deltas, wishes), layout's checkpoints_are_tables, store.rs's checkpoints*, deltas_make_checkpoints, damaged_index_records_are_none, adopted_extents_*, releases_let_their_commits_go, uploads_keep_freed_data_until_released, segments_closed_by_commits_not_durable_close_again; remote.rs's markers_*, segments_uploading_are_marked_*, *_marker_was_lost_*, attaches_upload_*, stores_with_no_uploader_attach_one_to_fork, writers_of_one_file_at_once_each_write_their_own (the marker's); host_store's forks_with_no_uploader_make_nothing (every store has one); Shuttle's needs_durable_commits_only, needs_commits_durable_once_synced, needs_frees_decided_as_commits_are_published (their rules, shown unneeded), needs_closes_to_wait_for_framings_after_a_failed_flush (framings are one queue: no schedule fails without the rule there; needs_closes_to_wait_for_framings still shows it). Rewritten: footers_chain_*, reaping_*, rebuilding_spends_* (reopening_spends_*), branches_open_reading_* (by their logs' segments' tails), resizes_are_* (commits of their own), segments_set_aside_*, requests_served_during_a_flush_by_pct, gathers_take_what_may_upload_at_once (in upload.rs). New: wal.rs's 4, crash/kept.rs's 2, stores_unmade_*, logs_failing_*, commits_reopen_from_their_logs, the lag, logs_caught_up_upload_in_order, dropped_stores_*, stores_whose_bucket_lacks_*.

Rebased onto main ccbe81d0e: the design is 872d739e7 (was 1597b18aa), TASK-195 3402efc60, TASK-204 3a63ada14. The rebase took main's TASK-141.4 drain (remote::uploaders now extent_store::upload::uploaders, counted as each uploader starts and stops) and TASK-194's ExtentStore::holds_what (now by Store::root and Store::extents); main's condensed OPEN_QUESTIONS keeps a line for the decisions, the full text below.

Decision moved here from OPEN_QUESTIONS.md:
## One recovery: a store opens from its open segment's log, its lease, the bucket and its WAL; the uploader is extent_store's (TASK-165.9, TASK-196, 2026-11-06)

The index and its delta codec are gone; a commit is durable locally once the flush synced the open segment and appended the commit's record (its footer's `Contents`) to the open segment's log, and a framed segment is the durable record of its commits. Opening takes the branch's lease, recovers the branch, closes a segment a crash left open from its raw file and log, and rebases the WAL on the branch; a WAL the branch moved past refuses (`ESTALE`) until `fxdaxctl take` drops it. Decided where doc-14 left it open:
- **Local format 8, no migration**: a store of an older format is refused (`ENOTSUP`, `extent_store::Format`), as TASK-165.6's were. *Undoing it:* none; a store is remade from its bucket.
- **A store says it is whole last** (TASK-196): `Store::make` writes an `unmade` marker before anything else and `made` removes it once its maker filled it; `Store::recover`, `Storage::open`, `ExtentStore::open` and extentd refuse an unmade store (`extent_store::Unmade`, `ENOENT`), and ImportImage and CreateStore remove one and make it anew. *Instead:* the description written last, which a store's own files (its WAL) need before it is described. *Undoing it:* `store::UNMADE`.
- **Restamps live in memory**: a WAL segment's footer on disk is as its chain stamped it when it closed; a rebase onto another writer's chain restamps the queued footers in memory, and the upload restamps the bytes (`extent_frame::restamp`). A crash before the upload leaves them stamped as before, and the next opening rebases again. *Instead:* rewriting each file at the rebase, a write per segment on the open path. *Undoing it:* `Closed::restamped`.
- **The WAL's base is a file of two 4 KiB slots** (serial, base), written and synced before what an acknowledgement let go of leaves the store, so a crash after never finds the bucket's files gone and the base behind them. *Instead:* a record in the open segment's log, which an acknowledgement of a closed segment has no business appending to. *Undoing it:* `wal::BaseFile`.
- **Framings are one queue, in the order segments closed** (`Framing { queue, running }`), so a framed file a crash leaves whole follows whole ones; the flush waits for every framing through its last commit. *Instead:* framings in parallel, and recovery taking only a prefix of whole files. *Undoing it:* `Store::frame_behind`.
- **A segment goes to the uploader once framed and its commits are durable**: one a crash lost here, acknowledged in the bucket, would leave the branch past the WAL, which the opening reads as diverged (`ESTALE`) though the commits are the store's own. The uploader flushes before it takes segments, so the bound holds back only a segment a commit closed between; it is plain code, not one of the Shuttle tests' rules: `durable_only`, `durable_once_synced` and `decide_at_publish` are gone, as no schedule failed without them, their oracle having no crash. *Instead:* taking framed segments of commits not yet durable, and the rebase taking the branch's commits past the WAL as its own, which needs the writer to tell its own commits from another's by more than the chain. *Undoing it:* `Store::uploadable`.
- **A capacity change is a commit of its own, sealing nothing**, as commits no longer seal what is open. *Undoing it:* `Store::set_capacity`.
- **Frees keep their data for the bucket** until their segment is acknowledged, always (the uploader reads what it uploads from the log's files). *Undoing it:* `State::release_kept`.
- **A store's next sequence, as it opens, is one past the highest of its branch's extents it holds**, not the footers' (`Chain::close` stamps them past their writer's floor): fxfs looks for orphans below it, and each opening's writer mints past a floor above every sequence an earlier one minted, freed ones included. *Instead:* the footers' value, which has fxfs look for orphans a writer's range below its floor. *Undoing it:* `Recovering::open`'s `next_sequence`.
- **A store whose WAL is empty takes the branch as the bucket has it**, wherever it moved; before, a store that held the branch refused once another moved it on. Only commits of its own the branch lacks make it refuse.
- **The lag's bytes count the open segment's** (`Store::wal_bytes`), as the commits not yet released did before.
- **Dropping a store stops its uploader first**, which closes the open segment, uploads it and stops (TASK-192), then closes the store; while the bucket is out of reach it waits, as uploads in flight made it wait before. A host that goes without it leaves the commits in its WAL, which its next opening uploads.
- **A commit asked to be durable (`COMMIT_DURABLE`) still waits for its upload and acknowledgement**, as doc-14 keeps it: GCE's 130 ms fdatasync against 16 ms on local disk (r740-r744, TASK-165.11) is that wait, which this design does not change; making it local is TASK-165.11's.
- **Acknowledged files leave at once**, the base made durable first, not at the next durable commit.
- **Recovery keeps the WAL's whole segments to the first not whole whose log holds commits**, which it closes again; those after it hold no durable commit (a flush makes none durable before every segment closed before is framed and its entry synced) and go first, durably: one kept after a segment closed again would follow the commit that closed it, which the log lacks. Before, recovery refused such a store. Found by the crash test cutting openings at each write (`openings_cut_at_each_write_open_again`). The WAL's base file is opened for each writing rather than held open, a file a store fewer (300 forks of a base held 11 files each, the bound 10).
- **A store whose WAL base names commits the bucket acknowledged refuses to open where the bucket has no such branch** (`NoBranch`, `ENOENT`): deleted, or another bucket, as a `memory:///` bucket is in another process. Making the branch anew would lose them silently. `fxdaxctl take` drops the WAL, base and all, and makes it anew.
- **Stat of a closed store opens it**, as the daemon opens any store, taking its branch's lease: there is one opening. A copy placed before the branch's holder is refused (`ESTALE`).

a46d953d6: a second recovery bug the kept crash rounds found (round 42, once in about 60 runs, under load): an acknowledged segment whose framed file acknowledging removed, but whose raw data and log a crash brought back (their removals not durable), was taken for the first torn segment, and the durable whole segments after it dropped: the store reopened a commit behind what it made durable. Recovery now takes for the cut only a torn segment whose log holds commits after the WAL's base; segments_acknowledged_brought_back_are_no_logs is red without it. The same commit has the kept rounds' host crash cut off from the bucket too (Host::crash, a copy of the bucket as it was), so that the crashed host's uploader, which runs on until dropped, uploads nothing anyone reads afterwards, as a dead host's never does.
<!-- SECTION:NOTES:END -->
