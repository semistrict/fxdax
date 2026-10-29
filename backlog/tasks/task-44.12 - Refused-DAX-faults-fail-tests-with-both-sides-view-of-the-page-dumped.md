---
id: TASK-44.12
title: 'Refused DAX faults fail tests, with both sides'' view of the page dumped'
status: In Progress
assignee: []
created_date: '2026-10-24 21:31'
updated_date: '2026-10-29 18:36'
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
- [x] #3 Test runs assert no refusals past those a seal race explains, from extentd's and the VMM's counters
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

2026-10-29, the refusals classified (GCE r241-r244, main): extentd logs, for each refused access, the next change of the window covering the page (ExtentStore::refused), Firecracker each vCPU run of EFAULTs as it ends (38cb48619 in the fork), and the VMM the faulting thread (UFFD_FEATURE_THREAD_ID). Of r244's 133 refused stores every one was by a kworker of the system workqueue, KVM's asynchronous page faults, the guest's task faulted in as its guest went on, never a VMM thread; every one to a page mapped read-only of an extent sealed in the store; and r242's 191 refusals stalled vCPUs only 14 times, once each. A store reaching the window after a seal made its page read-only, before the guest's seal made its mapping read-only, is refused, and the guest's task retries through its own fault, which maps the page anew (at the same own-space offset, of a new extent: r241's 40 'met: MAP writably' of other extents). AC3: 5a2a6a12b, scripts/dax-refusals.py (tested), run by test-e2e.sh, fails a run with a refused store no seal explains (an extent open, or a page writable); none in r231, r240b, r241, r244 or the DAX gate's 40 suite rounds. AC2 (a refusal repeated for one page fails the run): a persistent refusal stops its VM (Firecracker's 1 s of EFAULTs), failing the test, with extentd's view logged; the guest's view at that point is not.
<!-- SECTION:NOTES:END -->
