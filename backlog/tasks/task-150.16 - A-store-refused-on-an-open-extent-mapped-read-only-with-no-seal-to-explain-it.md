---
id: TASK-150.16
title: >-
  A store refused on an open extent, mapped read-only, with no seal to explain
  it
status: To Do
assignee: []
created_date: '2026-11-01 16:53'
updated_date: '2026-11-01 18:44'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 531000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
r394 failed scripts/dax-refusals.py after both xfstests suites passed: in xfstests-fxfs-guest-dax a store at window offset 0x20b85000 to an extent read-only, open in the window, 0x2f000 bytes open in the store, mapped read-only 0x50000 bytes from 0x20b36000 1.2 ms before. TASK-99 signature, whose version stopped a VM. r389 had none and dax.c did not change between them. Results in /tmp/fxdax-r394-xfstests. The audit finding on fxfs_dax_map_slice may or may not explain it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test fails on the code as it stands, or shows the finding false or unreachable, which closes this as not a bug
- [ ] #2 Fixed, the test red again with the fix taken out
- [ ] #3 The notes name the blind spot, the test of its class, and what a search around it found
- [ ] #4 Passes on GCE in the guest suites, DAX and not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Not seen again: r397 and r398 had no unexplained refusals. Rare, still undiagnosed.

Recurred in r422 (all 14 default suites passing): store at 0x6c0000, one of 1002 refusals, read-only from 0x1000, 'open in the window; in the store: Ok(None)': freed in the store, the window still calling it open. r394's was open in both. So two of the last five DAX runs with refusals; this gate fails runs that otherwise pass.

Correction to the note above: 2 of the 8 runs since r394 with any refusals had an unexplained one (r394: 1 of 55; r422: 1 of 1002). Runs top out at 1001 or 1002 refusals logged, which looks like a cap on what the daemon logs: if so the gate sees only the first thousand and undercounts. Not yet checked.

Checked: crates/fxdax/src/extentd/server.rs:1118 logs a refusal only if count <= 1000 or a power of two, so scripts/dax-refusals.py sees the first 1000 and undercounts. A blind spot in the gate itself: it needs the count of unexplained refusals from the daemon, not from what it logged.
<!-- SECTION:NOTES:END -->
