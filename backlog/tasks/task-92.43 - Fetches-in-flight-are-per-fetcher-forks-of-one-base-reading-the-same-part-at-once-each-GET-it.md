---
id: TASK-92.43
title: >-
  Fetches in flight are per fetcher: forks of one base reading the same part at
  once each GET it
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-23 03:10'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: low
type: enhancement
ordinal: 327000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-92.21 (round 1) made the fetched-parts memory host-wide, so forks of one base share parts once fetched. GETs in flight (fetch::Parts) are still per fetcher: forks booting from one base that read the same part at the same moment each send a GET, and the shared memory dedupes only once the first lands.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Concurrent reads of one object part from several stores' fetchers send one GET
- [x] #2 A test counts the GETs of N fetchers reading one part at once
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A host's fetchers now share the GETs of parts in flight in the host's Memory (a0f558c3). Flights are keyed by the object's key, the part, and how the sending tail says the part is stored, so forks reading one part at once send one GET. A read by the tail of a deleted object never joins a GET of the object recreated at its key (1509b298). A read of a part already in the host's memory takes it without joining a flight (0d1a1f3b). That took back the 40% the host-wide lock had added: 8 fetchers doing 32 Ki reads each took 540/542/459 ms before, 735/801/754 ms with shared flights, and 353/330/377 ms after. fetch::tests::fetchers_of_a_host_reading_a_part_at_once_send_one_get_of_it counts 4 part GETs before the change and 1 after. It passes on macOS and GCE Linux, along with the promotion test and the recreated-object test.
<!-- SECTION:FINAL_SUMMARY:END -->
