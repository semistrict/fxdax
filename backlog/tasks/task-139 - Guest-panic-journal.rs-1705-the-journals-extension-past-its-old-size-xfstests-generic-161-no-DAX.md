---
id: TASK-139
title: >-
  Guest panic: journal.rs:1705, the journal's extension past its old size
  (xfstests generic/161, no DAX)
status: To Do
assignee: []
created_date: '2026-10-31 16:32'
labels:
  - bug
dependencies: []
priority: high
ordinal: 485000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r342, xfstests-fxfs-guest, generic/161, in writeback: 'fxfs: panicked at crates/fxfs/src/object_store/journal.rs:1705:13: assertion failed: inner.writer.journal_file_checkpoint().file_offset < size'. A guest's kernel panics: TASK-60's rule, never panic on what a workload does. The transaction extending the journal has to reach the journal within its old size, and a large reflink test's did not. Logs: /tmp/fxdax-r342-xfstests/e2e/xfstests-fxfs-guest.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 generic/161 passes, without and with DAX
- [ ] #2 A workload can't panic the guest here
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
