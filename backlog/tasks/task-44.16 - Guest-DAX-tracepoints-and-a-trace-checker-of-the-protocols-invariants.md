---
id: TASK-44.16
title: Guest DAX tracepoints and a trace checker of the protocol's invariants
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
updated_date: '2026-11-08 15:12'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: low
ordinal: 395000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The guest's beliefs about its pages and the host's mappings are each logged in part; nothing checks them against each other over a run. Tracepoints at the protocol's steps, merged with extentd's log of MAP, UNMAP and PRIVATE, let an offline checker replay the protocol's state machine (or the kernel's RV monitors run it live) and name the first step that breaks an invariant.

At dde8d58d3, after writable DAX was deleted (37c331a6c), the protocol is doc-13's copy-on-write: no region is open in the window, nothing is evacuated, and no page is writable through the window except a private one. The steps to trace: MAP of a sealed extent or zeros, read-only (fxfs_dax_map_pages, guest/linux/fs/fxfs/dax.c:820-905); PRIVATE, before a page is dirty (fxfs_dax_dirty, dax.c:2384-2443; fxfs_device_private, device.c:273); writeback's write-protect and its write through fxfs_write_many (fxfs_dax_write_back, dax.c:2066-2130; fxfs_dax_protect, 2034-2050); the seal (fxfs_device_seal_begin/end, device.c:167-194); rebind (fxfs_dax_rebind, dax.c:2142-2175); UNMAP. A range's page state is pages.c's (bda2e53a0), whose named transitions (fill, dirty, swap, rebind, cut, unmap) are where the guest's side changes. The invariants are specs/dax-cow's (DaxCow.tla), among them DirtyImpliesPrivate. dax.c's trace ring (fxfs.dax_trace, dax.c:513-602) records a page's maps and iomaps, not the host's side.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The guest emits tracepoints at the DAX protocol's steps, and extentd logs its MAPs with the same offsets
- [ ] #2 A checker replays a run's merged trace against the protocol's invariants and reports the first violation
- [ ] #3 A run with a refused store (guest/mutants/task-164.4-protect-by-entries.patch put back on a guest: a fault's writable PTE kept across a writeback, the host refusing its store after the rebind), replayed, is reported at its first broken invariant
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Sandbox team round 7 (TASK-111) asks for tracepoints or USDT probes at seal start and end, MAP by reason, evacuation, guest fault to extentd, and window reclaim, so bpftrace can measure latencies.

2026-11-05 triage: citations brought to dde8d58d3 (dax.c moved by f22c9459e and bda2e53a0); the mutants moved from guest/sim/mutants to guest/mutants (b7653f781).

2026-11-08, AC1's guest half (commit ad266aff6, rebased on 8d7086f11; held from main until the user decides on the boot cost: event tracing in the default guest kernel, or an opt-in build): TRACE_EVENT tracepoints in fs/fxfs/fxfs_trace.h, created in device.c: fxfs_device_submit/done (an id of the guest's own: the wire has none), fxfs_window_map_start/end, fxfs_window_unmap_start/end, fxfs_window_private_start/end, fxfs_dax_map (why: reach, range, rebind, zeros), fxfs_seal_begin/end (count; bytes sealed), fxfs_dax_fault_start/end, fxfs_dax_reclaim, fxfs_dax_writeback_start/end, fxfs_dax_rebind_start/end, fxfs_fsync_start/end. Fields of UAPI types so bpftrace compiles the formats without BTF. The fxfs.dax_trace ring is kept. fxdax.config: CONFIG_FTRACE, ENABLE_DEFAULT_TRACERS, BPF_EVENTS, no function tracer. Tests: fxfs-guest-checks' tracepoints check (tracefs, steps in order, ids paired) and fxfs-guest-bpftrace-dax (scripts/bpftrace/fxfs-latency.bt under Alpine's bpftrace). docs/guest-tracepoints.md. GCE, release (fxdax-test-r1001-t1492): fxfs-guest, fxfs-guest-dax, fxfs-guest-checks-dax (tracepoints PASS 78 ms), fxfs-guest-crash-dax OK; bpftrace-dax OK after the UAPI-types fix (seals 128-256 us, MAPs 64-512 us); the rerun's checks-dax failed only a_removed_fill_gives_back_all_but_the_reservation (TASK-257). Debug (fxdax-keep-n2-standard-4-us-east4-a-1): fxfs-guest-dax, checks-dax (tracepoints PASS 88 ms), crash-dax OK. Cost: init reached at 0.294 s median (5 boots) against 0.251 s (15 boots, main, t221), same host: ~40-45 ms; size not measured. AC1 stays open: extentd logs no MAP (TASK-44.16.1).

2026-11-08, the user's decision: event tracing stays in every guest kernel (fxdax.config's CONFIG_FTRACE, ENABLE_DEFAULT_TRACERS, BPF_EVENTS), at its measured cost of ~40-45 ms per boot (init at 0.294 s median against 0.251 s, GCE, same host); no opt-in build.
<!-- SECTION:NOTES:END -->
