---
id: TASK-149.43
title: task-164.4-protect-by-entries survives the release kernel's DAX suites
status: To Do
assignee: []
created_date: '2026-11-08 05:38'
labels:
  - testing
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 676000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-149.2 (GCE r1000): guest/mutants/task-164.4-protect-by-entries.patch (writeback's write-protect left to the DAX entries, as it was: a fault holding its page's entry past its iomap_end, which dax_writeback_one gives up on, keeps its writable PTE; the rebind's read-only MAP under it is refused) is killed by guest/sim's cow_writeback_during_fault alone. In a release guest kernel, fxfs-guest-fsx-dax, -fsx-delay-dax, -checks-dax and -script-dax (its files then the page cache's) all pass with it: no VM's stores meet a writeback's locked entry. Whether a debug or KASAN kernel, more delay injection (fxfs.dax_delay_us), frequent rebinds (fxfs.dax_rebind_every), or a targeted guest check (stores faulting while a writeback runs over their chunk) catches it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A guest case or check kills task-164.4-protect-by-entries in a VM, its header naming it as a second killer, or the header says why none can
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
