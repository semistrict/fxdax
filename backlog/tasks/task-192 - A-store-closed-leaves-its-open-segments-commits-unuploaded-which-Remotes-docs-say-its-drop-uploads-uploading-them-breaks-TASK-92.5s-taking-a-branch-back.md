---
id: TASK-192
title: >-
  A store closed leaves its open segment's commits unuploaded, which Remote's
  docs say its drop uploads; uploading them breaks TASK-92.5's taking a branch
  back
status: Done
assignee: []
created_date: '2026-11-06 03:21'
updated_date: '2026-11-07 09:26'
labels: []
dependencies: []
priority: medium
type: bug
ordinal: 643000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-141.4's drain test (crates/fxdax/tests/drain.rs, 2026-11-05). ExtentStore::drop (crates/fxdax/src/extents.rs:421) closes its store before its fields drop, so its Remote (crates/fxdax/src/remote.rs, 'Dropping it uploads what the store made durable, then stops') stops on a closed store, and a closed store closes no segment (extent_store close_between_commits: 'Closed, it changes nothing: its last segment closes when it is next opened'). A VM's last commits, in the open segment, reach the bucket only once the store opens again here, or the upload interval (1 s) ran first. A drain uploads them itself before closing (extentd::server run_connection, stores::uploaded while draining), so TASK-141.4 doesn't depend on this.

Tried: dropping the Remote first in ExtentStore::drop (uploader stops with the store open, gathering and closing the open segment). It uploads a store's take commit as it closes, and four tests go red, as they rely on it not landing: remote::tests::later_placements_fence_earlier_ones (the first copy, placed later still, takes the branch back), extentd's restores_fork_their_point_anew_once_another_writer_took_its_branch and control_forks_of_a_point_fork_it_anew_once_another_writer_took_its_branch (EIO: 'moved on elsewhere (commit 0x20000000001): its store here is stale'), and serves_hundreds_of_forks_of_a_base (137 'fxdax remote' threads: closes gathering on blocking threads). In production the take commit lands within the interval anyway, so those flows hold only for writers that go within a second.

Decide: whether a close uploads its open segment (and the take-back flows are rewritten to a fresh copy), or Remote's docs say it doesn't.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Remote's docs and ExtentStore::drop agree on whether a closed store's open segment is uploaded, by a test
- [x] #2 TASK-92.5's take-back flows are tested as production times them, the upload interval passed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Verified: CI 42352682020 on land-9 (landed as c9207e5d4) green, Linux workspace included; macOS workspace 2079/2079 (scripts/test.sh at c9207e5d4); scripts/upstream-changes.py --check passes. Decided in TASK-165.9: a close uploads its open segment. ExtentStore::drop stops the uploader first, which closes the open segment, uploads it and stops, then closes the store: dropped_stores_upload_their_open_segments_commits (crates/fxdax/src/remote.rs), red with the old order. TASK-92.5's take-back flows, rewritten for it (restores_fork_their_point_anew_once_another_writer_took_its_branch, control_forks_of_a_point_fork_it_anew_once_another_writer_took_its_branch, a_zygotes_restores_open_its_point_once with 50 ms object storage), run through extentd with production's upload interval and pass on Linux (CI).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A dropped store uploads what it committed, its open segment's commits too: ExtentStore::drop stops its uploader first, then closes the store, as extent_store::upload's docs say; the take-back flows are tested as production runs them. Landed with TASK-165.9; dropped_stores_upload_their_open_segments_commits. Verified by CI 42352682020 and the macOS workspace.
<!-- SECTION:FINAL_SUMMARY:END -->
