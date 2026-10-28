---
id: TASK-112.4
title: >-
  VMM: a commit's reply waits behind every window change queued before it, the
  background evacuation and restage MAPs included (~10.5 stage ops a commit at
  ~25 us)
status: To Do
assignee: []
created_date: '2026-10-28 03:23'
labels: []
dependencies: []
parent_task_id: TASK-112
priority: high
ordinal: 427000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-9: a DAX commit takes the guest ~430 us against ~263 without DAX; extentd spends 126 us. The VM process applies the daemon window changes on one thread in the order sent, before the replies after them, so a commit reply waits for evacuation and restage changes of earlier commits, which the guest runs in the background, as well as its own write-protects, which must precede it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A commit reply waits only for the window changes it ordered (its write-protects), not for other requests changes to other window ranges
- [ ] #2 A test shows a commit reply delivered while an unrelated MAP stage change is still being applied
- [ ] #3 fxfs-guest-pgbench-dax on GCE: guest device_commits_ns per commit falls, tps recorded against ff7961411
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
