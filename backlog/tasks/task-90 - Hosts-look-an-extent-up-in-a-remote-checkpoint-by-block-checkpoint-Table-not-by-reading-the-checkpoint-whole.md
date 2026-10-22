---
id: TASK-90
title: >-
  Hosts look an extent up in a remote checkpoint by block (checkpoint::Table),
  not by reading the checkpoint whole
status: To Do
assignee: []
created_date: '2026-10-22 14:46'
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
