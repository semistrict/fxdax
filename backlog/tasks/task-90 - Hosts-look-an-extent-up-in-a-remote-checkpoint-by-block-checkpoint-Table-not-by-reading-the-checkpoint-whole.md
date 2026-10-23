---
id: TASK-90
title: >-
  Hosts look an extent up in a remote checkpoint by block (checkpoint::Table),
  not by reading the checkpoint whole
status: To Do
assignee: []
created_date: '2026-10-22 14:46'
updated_date: '2026-10-23 03:10'
labels:
  - review-2026-10-21
dependencies: []
priority: medium
type: enhancement
ordinal: 264000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-28.6 built checkpoint::Table so that a host resuming a large branch from object storage reads one extent's place with a suffix GET, the block index and one 64 KiB block (doc-7, Checkpoints). No host uses it: hosts read checkpoints whole. The object-storage refactor (review of 2026-10-21) left it so; the user chose to wire it in rather than delete it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A host's lookups of extents in a remote checkpoint go through Table, through the host's Fetcher, and the second part reader (object::range) goes
- [ ] #2 A test counts the requests and bytes one lookup takes: the tail, the index and one block
- [ ] #3 The extent_remote suites, its simulation included, and fxdax's pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Round 1 (review-r1): extent_remote's fetcher looks extents up in a checkpoint by the blocks of its table, read as its parts (Fetcher::lookup over checkpoint::Table), and object::range is gone; a test counts one lookup. AC #1 is not met: no host calls Fetcher::lookup yet. Hosts' writers recover a branch's state whole, because the attach diff, take's adopt of every extent and Writer::checkpoint need every extent, and a taken branch's reads use the writer's in-memory state. Left: the writer's state keeps its checkpoint as a checkpoint::Table under an overlay of the footers after it, recovery reads the checkpoint's tail rather than its entries, and a host that took a branch (fxdax remote::Fetcher::offset/read via Writer::location) finds extents by block. That changes Writer::checkpoint and recovery, which the TASK-91.2 session was working in.
<!-- SECTION:NOTES:END -->
