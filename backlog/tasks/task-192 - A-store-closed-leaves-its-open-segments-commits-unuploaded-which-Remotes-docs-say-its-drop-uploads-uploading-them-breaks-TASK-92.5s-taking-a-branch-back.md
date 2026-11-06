---
id: TASK-192
title: >-
  A store closed leaves its open segment's commits unuploaded, which Remote's
  docs say its drop uploads; uploading them breaks TASK-92.5's taking a branch
  back
status: To Do
assignee: []
created_date: '2026-11-06 03:21'
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
- [ ] #1 Remote's docs and ExtentStore::drop agree on whether a closed store's open segment is uploaded, by a test
- [ ] #2 TASK-92.5's take-back flows are tested as production times them, the upload interval passed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
