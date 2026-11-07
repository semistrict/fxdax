---
id: TASK-160.17
title: 'extent_store: the uploader''s holds with one owner, acknowledged in one call'
status: Done
assignee: []
created_date: '2026-11-03 13:08'
updated_date: '2026-11-07 09:28'
labels:
  - structure
dependencies:
  - TASK-165.9
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 588000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Segments not yet uploaded are owned by Uploads.unuploaded and mirrored by hand into Segment.uploading; frees kept for the uploader mirrored into three counters; one acknowledgement is release(epoch) then uploaded(n), which must agree. Explains TASK-85, 79.89, 126f21bcf. Proposal: Segments owns the hold (Phase::Closed{held}), and an Uploader handle with acknowledged(through). After TASK-153.8 and 41.1.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 One owner of the upload hold
- [x] #2 One acknowledgement call
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-06 tidy: depends on TASK-165.9, which moves the uploader into extent_store. Since TASK-165.8, Store::acknowledged(number, key) (crates/extent_store/src/store.rs:1643) replaces Store::uploaded, but an acknowledgement is still that and Store::release(epoch) (store.rs:1160), two calls; Uploads keeps unuploaded (crates/extent_store/src/uploads.rs:131-137).

Re-checked against main at 00e541868, after TASK-165.9: the problem described is gone. Uploads.unuploaded and its hand-made mirror Segment.uploading no longer exist (uploads.rs holds only Closed, a segment handed to the uploader); the segments not yet acknowledged are the WAL's queue alone (wal::Queue, which takes them for the uploader in order and acknowledges them in order). The release(epoch)-then-uploaded(n) pair that had to agree is one call: Store::acknowledged(number) takes the first queued segment, writes the base, and releases the kept frees through the epoch its own footer names (State::release_kept(footer_last(footer))), so the two can't disagree; nothing else calls release_kept or Queue::acknowledge. The uploader acknowledges through that call alone (upload.rs's acknowledged: writer.acknowledge(), then store.acknowledged(n) for each segment its writer acknowledged). Segment.acknowledged records an acknowledged segment's key, for its files to leave (TASK-165.8), not a hold. KeptExtents' uploader flag, released set and released_to are one owner's own indices. Covered by wal's queues_give_what_may_upload_in_order, store's acknowledged_segments_leave_a_churning_guests_files and frees_kept_in_segments_acknowledged_hold_no_file (3/3 passing), and Shuttle's writer_closer_and_uploader_randomly (the uploader acknowledging as commits, closes and views race).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Moot after TASK-165.9: the segments not yet acknowledged have one owner, the WAL's queue, and one call, Store::acknowledged(number), acknowledges a segment and releases the frees its footer's commits kept, so the mirrored hold and the two calls that had to agree are gone. Verified against main 00e541868 by reading every caller and by the queue, acknowledgement and Shuttle uploader tests.
<!-- SECTION:FINAL_SUMMARY:END -->
