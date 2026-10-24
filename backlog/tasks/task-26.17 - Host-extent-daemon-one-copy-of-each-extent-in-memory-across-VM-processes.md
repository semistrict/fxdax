---
id: TASK-26.17
title: 'Host extent daemon: one copy of each extent in memory across VM processes'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-15 02:13'
updated_date: '2026-10-24 01:05'
labels:
  - host
  - dax
dependencies: []
parent_task_id: TASK-26
ordinal: 53000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
libkrun runs one VM per process (krun_start_enter takes the process over and exit()s with the guest), and fxdax's extent cache lives in the in-process Zircon kernel's memory, so the host page cache that doc-2/doc-4 promise to share between VMs (forks of a VM, VMs of one base image) is shared only by one VM's windows. The user decided (2026-10-15) to switch to a multiprocess architecture using shared memory (memfd), Linux first. The in-process kernel's physical memory is already a memfd (crates/zircon/src/vm/physmap.rs), and DAX windows already map its pages into KVM's memory with mmap(MAP_SHARED): one process owning the kernel, cache, pager and stores can let every VM process map the same pages.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A per-host daemon (fxdax-extentd) serves extent stores to VM processes over a Unix socket, and libkrun's extent device uses it when configured to; the in-process store stays for macOS and for tests
- [x] #2 Two VM processes mapping the same sealed extent (stores forked from one) map the same physical pages of the daemon's memory, as a test with two processes shows
- [x] #3 A page leaves a VM's window (unmapped, or made read-only) before the daemon frees it, reuses it or shares it with other VMs
- [x] #4 A VM process that exits releases its store, window and open extents in the daemon
- [x] #5 The guest e2e suites and pjdfstest pass with the extent device in daemon mode on Linux
- [x] #6 The GCE benchmark in daemon mode is recorded against in-process mode and ext4 DAX on writable pmem
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. zircon SecondStage::map passes the physical memory offset of the pages it maps; UffdSecondStage maps by (memfd, offset) instead of mremap of the window's own mapping (same behavior, any process).
2. UffdSecondStage takes faults to a resolver trait (the in-process Window, or a remote one).
3. Protocol (crates/fxdax/src/extentd/): length-prefixed messages over a Unix stream, the physical memory memfd passed once with SCM_RIGHTS. VM->daemon: open store, READ, WRITE, FREE, MAP, UNMAP, COMMIT, ROOT, INFO, create window, fault, stage ack. Daemon->VM: replies, and window stage ops (map memfd offset / protect / unmap), applied in order by the VM's reader thread before later replies; unmap and protect-to-read-only are acknowledged before the daemon goes on.
4. Daemon: ExtentServer serving connections with the existing ExtentStore (cache, pager, Dax) and a RemoteSecondStage per window; a connection's close drops its store and window.
5. Client: RemoteExtentStore with ExtentStore's API (incl. create_dax_window_in_host_memory), and an enum the libkrun device uses for either.
6. Binary fxdax-extentd; libkrun's device connects when FXDAX_EXTENTD names its socket.
7. Tests: protocol and window ops through a socketpair; the extents.rs tests against a daemon; a two-process test that two clients mapping one extent get the same memfd offsets; guest e2e in daemon mode; GCE benchmark.
Decisions: copy READ/WRITE data over the socket at first (sharing guest RAM as vhost-user does is a later optimization); one store per connection; the daemon refuses a store another connection has open.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-23 audit, the user agreeing. AC1: crates/fxdax/src/bin/fxdax-extentd.rs (c9525b70); its in-process store for macOS was dropped on purpose (537c7eb4, eeaf8b38: daemon only), and Firecracker's extent device replaced libkrun's. AC2: extentd/tests.rs two_processes_map_the_same_pages. AC3: dax/shuttle_tests.rs stores_race_seals_and_frees_*, needs_frees_to_map_zeros. AC4: extentd/tests.rs stores_open_once_their_vm_process_goes, tenants_arenas_go_with_them. AC5: GCE r63, 2026-10-23, kernel vmlinux-drv3 at main 2428910c+ (guest code unchanged since): extent-guest, fxfs-guest, fxfs-guest-dax, fxfs-guest-checks, fxfs-guest-checks-dax, fxfs-guest-crash, fxfs-guest-crash-dax, pjdfstest-fxfs-guest, pjdfstest-fxfs-guest-dax: 9/9 OK, every device served by fxdax-extentd (daemon mode, the only one since 537c7eb4). Workspace nextest: macOS 2466 passed (scripts/test.sh), Linux 2544 passed (GCE). scripts/upstream-changes.py --check passes. AC6: docs/measurements/fs-bench-history.md, 2026-10-15T0137Z (in-process) and T0507Z (daemon), both against ext4-dax-pmem.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxdax-extentd, a per-host daemon, serves extent stores to VM processes, which map the same physical pages of a shared extent; pages leave a window before the daemon frees or shares them; a VM process's exit releases its store. Guest suites and pjdfstest pass in daemon mode on GCE. Closed in the 2026-10-23 audit of stale tasks.
<!-- SECTION:FINAL_SUMMARY:END -->
