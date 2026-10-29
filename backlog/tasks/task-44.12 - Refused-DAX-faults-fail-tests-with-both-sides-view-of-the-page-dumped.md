---
id: TASK-44.12
title: 'Refused DAX faults fail tests, with both sides'' view of the page dumped'
status: In Progress
assignee: []
created_date: '2026-10-24 21:31'
updated_date: '2026-10-29 07:41'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: high
ordinal: 391000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A store the host refuses, the guest believing the page writable, stops the VM a second later with only KVM_RUN's EFAULT (TASK-99); the sandbox team had no logs to give. The host now logs refusals (extentd's describe_window_offset, the VMM's refused()). A refusal that repeats is an invariant broken: guest belief and host truth differ. Tests should fail on it at once, with what the guest thinks of the page (its region, range bits, PTE) beside what extentd maps.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A guest debug interface reports its view of a window offset: the region or range, its writable and mapped state
- [ ] #2 In tests, a refusal repeated for one page fails the run with both views logged
- [ ] #3 Test runs assert no refusals past those a seal race explains, from extentd's and the VMM's counters
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
The window's latest 4096 MAPs/UNMAPs (and zeros mapped in place of freed extents) kept in ExtentStore::changes; a refused store's log names the page's history. GCE r84 on TASK-99's kernel b7: 'a store at DAX window offset 0x10060000 refused: ExtentId{..} 0x1000 bytes read-only; open in the window; in the store: open; the page's latest MAPs: [MAP of ExtentId{..} from 0x0, read-only, Ok, 743 us ago]': TASK-99's read-only MAP of an open extent, named in one line.

2026-10-29: 633ba8bc0 (on main): the runner fails a passing test whose VMM logs (firecracker.log, <name>.log) say the DAX window refused a guest's access (vm::refusals, unit-tested), as a kernel's complaint does: toward AC3, with no allowance for seal races until one shows. Kept logs of failed runs before TASK-121's fixes held 21 and 72 refusals; GCE r216 (main) shows whether passing suites have any.

GCE r216 (main, kernel built): with 633ba8bc0 every DAX suite failed on refusals its VM survived: checks-dax 469, checks-dax-delay 501, crash-dax 3, pgbench-dax 104 (load.log). Reverted (06fffef19). Refusals the window resolves, likely a seal's (a store to a page the host made read-only as the guest still took it for writable), are routine; the check must tell those from refusals that persist, by offset over time (a page refused again after a seal ended), before it fails tests.
<!-- SECTION:NOTES:END -->
