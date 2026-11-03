---
id: TASK-153.11
title: 'To try: read the checksums Fxfs writes, so a flipped byte is an error'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 23:09'
updated_date: '2026-11-03 09:41'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: high
type: task
ordinal: 558000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-149.9: about 84% of file-data flips past the last device flush read back wrong with no error; no read or fsck checks data checksums, layer extents carry fletcher sums nothing reads, and from reading the code DAX writes have none at all. Checking layer sums on read would catch most metadata flips with no format change. Data on DAX is the larger question: a read through the window checks nothing, by design, so where a check could live (on the host, on the way to object storage, or a scrub) is the decision.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Layer checksums read, no format change, cost measured
- [x] #2 A design says what guards file data on DAX, or why nothing does
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Per the decision in OPEN_QUESTIONS.md (host CRC32C in extent_frame; the guest doesn't check): 1. Inventory every host read path of extent data and whether the frame CRC is checked before the data reaches a guest, a MAP, or another object. 2. Close the gaps: raw parts of framed files read whole and checked (Source::locate); files uploaded as they are checked (read_object). 3. A test per path flipping a stored byte and expecting an error; each red with the gap put back. 4. Measure the checks on hot paths (pagein-bench, store-bench cleaning, a CRC bench). 5. Record the uncovered cases. 6. scripts/test.sh on macOS and test-gce.sh all on GCE.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Inventory: host read paths of extent data (2026-11-03)

Decision (OPEN_QUESTIONS.md): the host's CRC32C in extent_frame guards file data; the guest checks nothing. Every path, and whether a part's CRC is checked before its bytes reach a guest, a MAP of the window, or another object:

| Path | Code | Before | Now | Test (flips a stored byte, expects an error) |
|---|---|---|---|---|
| Local store files: closed segments, sealed extents' own (`Store::read`, `read_kept`, `read_view`) | `Source::locate`, `Store::read_parts` | compressed parts checked; **raw parts read in place, unchecked** | every part read whole and checked | extent_store tests/corrupt.rs `flipped_bytes_fail_the_reads_of_their_parts` |
| Extent cache page-ins, and the guest's READs through it | fxdax cache.rs `StoredExtent::read` → `Store::read` / `read_kept` / fetcher | same gap | checked; a failed page-in is EIO, nothing mapped | fxdax extents.rs `flipped_bytes_fail_page_ins_and_reads` |
| Object storage, ranged fetches (page-ins, `Fetcher::read`, lookups) | extent_remote fetch.rs `Fetching::part` | checked: a mismatch refetches the tail once, then `Corrupt` | unchanged | existing `parts_failing_their_crcs_are_fetched_again_once` |
| Object storage, whole objects (`extent_remote::read`, the collector's GETs, checkpoints) | `crate::read` (a fetcher of its own), `layout::data`, `table::decode` | checked | unchanged | collector.rs `objects_with_flipped_bytes_are_not_compacted` (both its GET and `crate::read`) |
| The host's disk cache of fetched parts | fetch.rs `Fetching::kept` | checked; a bad copy dropped and refetched | unchanged | existing `parts_kept_failing_their_crcs_are_fetched` |
| Collector's compaction moves | collector.rs `compact` | checked (`layout::data`) before copying under new CRCs | unchanged; the pass fails, no moves object | `objects_with_flipped_bytes_are_not_compacted`, red with the bytes taken unchecked |
| Local cleaning copies (`copy_out`) | store.rs → `read_into` | **raw parts copied unchecked, then framed under a new CRC** | checked; the pass fails | corrupt.rs `cleaning_copies_no_flipped_byte` |
| Seal copies of open extents with pieces in closed segments | store.rs seal → `parts_locked` / `read_parts` | same gap | checked; the commit fails | corrupt.rs `flipped_bytes_fail_reads_and_seals_of_open_extents_in_closed_segments` |
| Forks and inheritance (a fork reading its base's files) | `Lineage::segment_file` → `Source::Framed` | same gap | checked | corrupt.rs `flipped_bytes_fail_a_forks_reads_of_what_it_inherited` |
| Followers (`read_view`, and a follower's fetches) | `Store::read_view`, extents.rs `read_stored` | same gap | checked | `flipped_bytes_fail_the_reads_of_their_parts` (read_view) |
| Uploads of files as they are (closed segments, own extents) | fxdax remote.rs `stored` | **unchecked: a damaged file went to the bucket under the CRC it fails** | `extent_store::read_object` checks the whole frame; the writer stops, the commit fails EIO | remote.rs `files_with_flipped_bytes_are_not_uploaded`; corrupt.rs `closed_segments_with_flipped_bytes_are_not_taken` |
| Uploads by batch (what the store reads for a segment it plans) | remote.rs `send` → `read_kept` | same gap as local reads | checked | as local reads |
| Imports, the image device | import.rs (OCI layers by sha256 digest; docker-export tarballs have none); storage/image.rs `read_with_opts` → `locate` | layers checked; device reads had the raw-part gap | checked | storage/image.rs `flipped_bytes_fail_reads`; existing import.rs test of a corrupt layer |
| Reads of open extents | raw files (open segment, open extent's own), `OpenPages` in memory | unchecked | unchanged: uncovered, below | — |

Every new test went red with the gap put back (raw parts read in place; `read_object` not checking; the collector taking the bytes unchecked).

## Cost (M-series Mac, release)

- CRC32C (crc_fast): 16 µs a MiB stored (`bench_checking_parts`).
- Page-ins (`pagein-bench 256 5`, 3 pairs of runs, base vs new): raw parts' CPU per part 187/193, 169/185, 182/215 µs; compressed 593/593, 571/580, 586/593 µs. Wall time is disk-bound and noisy either way. A first version, copying a raw part into a new Arc, cost +80-100 µs; keeping the read buffer (`Arc<Aligned>`) brought it to the CRC's.
- Cleaning (`store-bench cleaning`, MemDir): a pass copying 23 MiB of 4 KiB raw extents 15.2-15.7 ms → 23.0-36.8 ms, each extent's 4 KiB read now reading its 1 MiB part (kept for the next). On a real disk, 1 MiB reads replace 4 KiB ones.
- Uploads: one CRC pass of each file, about 0.5 ms for a 32 MiB segment, against its PUT.

## Still uncovered, and what could still be checked there

- **Data on a host's disk before it's sealed:** the open segment's raw file and open extents' own raw files carry no CRC; a seal, a commit's copies, `close_left_open` after a crash, and framing a closed segment read them and frame them under new CRCs, blessing a flip. Could still be checked: a CRC per write, kept by the store and checked as the raw bytes are framed (a CRC pass per write, 16 µs a MiB); after a crash those CRCs are gone unless the index journals them. Exposure is short: a raw file lives until the next seal or close (segment_age 60 s, or 32 MiB).
- **Pages already in memory:** DAX window pages (extent cache, `OpenPages`), the store's parts unpacked lately, and the fetcher's recent parts were checked once, as read, and are trusted after. A memory flip there is ECC's to catch; a scrub could re-read a cached extent's parts against their CRCs and compare, at a full read per extent scrubbed.
- **Docker-export tarballs** imported have no digest to check.
- Layer checksums in Fxfs (this task's AC #1) are not read: the decision is that the guest checks nothing.

## Blind spot and structure

Found by reading `Source::locate`: raw parts were `Part::Data`, the same variant as raw files, so a framed file's raw part looked like bytes needing no check; every test of corruption used compressible data, or flipped bytes the fetcher checks (its raw parts are checked there). Structure: one variant now means unchecked bytes (raw files alone), and every framed part goes through `Packed::unpack`, the one place a CRC is checked on the local read path. The upload path read files with `read_whole`; `read_object` is what anything sending a file on as it is now uses. Searched for other `read_whole` users of extent data: segments.rs `close_left_open` checks the whole file (`extent_frame::data`); tests otherwise.

Verification: macOS scripts/test.sh passes (workspace 2019/2019, Shuttle and the other suites all passing; clippy's rules). GCE (n2-standard-4, us-west1-a, scripts/test-gce.sh all fxdax-test-t15311-checksums): workspace 2178/2178, Shuttle suites passing, 14 of 14 guest cases OK; host deleted. Linux ran on GCE, as AGENTS.md has it, not Lima. scripts/upstream-changes.py --check passes; no upstream file changed, so no deviation to document. AC #1 left unchecked: the decision is that the guest reads no Fxfs checksums, so layer checksums aren't read.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Every host read path of extent data now checks extent_frame's CRC32C before the bytes reach a guest, a MAP or another object. Gaps found and closed: a framed file's raw (incompressible) parts were read in place unchecked, through local reads, page-ins, followers, forks, the image device, and cleaning and seal copies, which framed them on under new CRCs; and the uploader put files as they were, unchecked. Raw parts are now read whole, checked and kept like compressed ones (+10-30 µs CPU on a ~185 µs raw page-in; CRC 16 µs/MiB); uploads go through read_object. A test per path flips a stored byte and expects an error, each red with its gap put back. The inventory, costs and uncovered cases (raw data before sealing; pages in memory; docker tarballs) are in the notes. Not done: AC #1, layer checksums, as the decision has the guest check nothing. Verified: scripts/test.sh on macOS, test-gce.sh all on GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
