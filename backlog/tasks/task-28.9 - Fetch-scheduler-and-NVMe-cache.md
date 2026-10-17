---
id: TASK-28.9
title: Fetch scheduler and NVMe cache
status: To Do
assignee: []
created_date: '2026-10-17 01:32'
labels: []
milestone: m-5
dependencies:
  - TASK-28.7
  - TASK-28.8
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
type: feature
ordinal: 89000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-7 Reading: misses go to object storage through one scheduler per host, and the NVMe cache is the local store own files, sparse. object_store get_ranges fans out only 10 ways, fixed; one connection gives roughly 80-100 MB/s, so a host needs tens of GETs in flight.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Misses fetch 1 MiB parts of objects; concurrent misses on a part share one GET; demand misses go before read-ahead; GETs in flight are bounded in number and memory
- [ ] #2 Fetched parts are kept in sparse local copies at their real offsets and evicted by punching holes, keeping a free-space floor; unuploaded data is never evicted
- [ ] #3 Every read from NVMe or object storage is checked against the footer CRCs; a mismatch refetches once, then fails with EIO, shown by a test that corrupts both
- [ ] #4 Read-ahead grows along sequential misses and takes neighbouring parts of a segment
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
