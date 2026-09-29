---
id: TASK-4
title: Background writeback and dirty-byte limits
status: Done
assignee:
  - '@claude'
created_date: '2026-09-28 02:41'
updated_date: '2026-09-29 00:49'
labels: []
milestone: m-0
dependencies: []
references:
  - vendor/fuchsia/src/storage/fxfs/platform/src/fuchsia/volume.rs
  - vendor/fuchsia/src/storage/fxfs/platform/src/fuchsia/volumes_directory.rs
type: feature
ordinal: 4000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
FxVolume has no parent VolumesDirectory, so the port lacks two upstream mechanisms:

- FxVolume::start_background_task, which flushes all files 70 s after mount and then every 20 s (5 s / 1.5 s under warning / critical memory pressure), and minimizes memory under critical pressure.
- VolumesDirectory::report_pager_dirty, which under critical pressure flushes all volumes before letting more pages be dirtied once dirty bytes reach ~1% of physical memory.

Without them, dirty pages are written back only on explicit sync, connection close or volume termination. The kernel cannot evict dirty pages, so a guest writing without fsync grows host memory without bound, and a host crash loses all data since the last sync.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A mounted volume flushes dirty files periodically without an explicit sync, shown by a test
- [x] #2 Kernel memory-pressure levels change the background period and trigger minimize_memory, shown by a test
- [x] #3 Under critical pressure, dirtying past the limit flushes before the page is marked dirty, shown by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/kernel-cfg.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Kernel: port zx_system_get_event (zircon/kernel/lib/syscalls/system.rs) over the memory watchdog's pressure events; there are no jobs, so the root job isn't checked. Test through zx in zircon-core-tests.
2. fidl-fuchsia-memorypressure stand-in: Level, and the Watcher protocol served in-process (as fidl-fuchsia-io's protocols are).
3. fxfs-platform memory_pressure.rs: upstream's MemoryPressureMonitor, with the monitor serving the Watcher protocol in-process. start() connects it to a provider standing in for Fuchsia's memory monitor: it waits on the kernel's pressure events through a port and reports level changes (vendor the memory monitor's pressure observer as its source).
4. volume.rs: restore the parent, background_task, start_background_task, background_task(), for_level, minimize_memory and parent-reported dirty/clean verbatim.
5. volumes_directory.rs: an excerpt of VolumesDirectory: mounted volumes, mount_store/add_mount/lock_mount/terminate/unmount, memory pressure monitor, PagerDirtyByteCount, is_flush_required_to_dirty, report_pager_dirty/clean.
6. testing.rs: the fixture with a volumes directory and memory pressure proxy, as upstream.
7. fxdax Vfs mounts through VolumesDirectory with the kernel-backed monitor.
8. Tests: port test_background_flush(_with_warning/_critical_memory_pressure), test_dirty_pages_accumulate_in_parent and test_flush_before_mark_dirty_under_critical_memory_pressure; an integration test driving real kernel pressure (small memory_bytes and thresholds via BootOptions) through the monitor.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- Kernel: zx_system_get_event (the watchdog's pressure events; there are no jobs, so the root job isn't checked) and zx_object_wait_many, both ported. Tests: memory_pressure.rs (pressure_events_follow_free_memory, system_get_event_rejects_unknown_kinds) and objects.rs (wait_many_*).
- fidl stand-in: ServerEnd::serve_until_dropped, which closes a served connection when its Serving guard drops, as dropping a request-stream task closes its channel. New stand-ins: fidl-fuchsia-memorypressure (Level, Provider, Watcher; RegisterWatcher takes the watcher's proxy, as there are no client ends) and fidl-fuchsia-fs-startup (CreateOptions).
- memory-pressure-signaler: a port of src/developer/memory/pressure_signaler (observer on its own thread with wait_many; notifier on a local executor). It is process-wide, via provider(). There are no crash reports. A watcher whose notification fails is released (upstream would leave it pending). Test: tests/signaler.rs drives real kernel pressure.
- fxfs-platform: memory_pressure.rs (monitor verbatim, serving its watcher in-process); volume.rs background task, for_level, minimize_memory and parent-reported dirty/clean verbatim, with the timer fused and replaced because the portable timer has no reset; volumes_directory.rs excerpt; the testing fixture with a volumes directory and memory pressure proxy. Ported tests: test_background_flush (+ warning/critical), test_volume_creation, test_dirty_pages_accumulate_in_parent, test_flush_before_mark_dirty_under_critical_memory_pressure.
- fxdax Vfs mounts through VolumesDirectory with MemoryPressureMonitor::start(). Integration test crates/fxdax/tests/memory_pressure.rs: under real critical pressure the volume writes dirty pages back with no fsync. With no monitor, 64 pages stay dirty and the test fails, which confirms it tests the pressure path.
- Vendored for citation: sdk/fidl/fuchsia.memorypressure, src/developer/memory/pressure_signaler/pressure_{observer,notifier}.{cc,h}, src/storage/fidl/fuchsia.fs.startup/startup.fidl.
- Validation: scripts/test.sh 1754 passed on macOS, 1755 on Lima; kernel-cfg --check and upstream-changes.py --check pass; macOS guest tests (virtiofs-fxdax, -nodax) pass.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxdax volumes now write back and limit dirty data as Fxfs does on Fuchsia. The kernel's memory watchdog events reach a port of Fuchsia's memory pressure signaler (via newly ported zx_system_get_event and zx_object_wait_many), which feeds Fxfs's MemoryPressureMonitor. Each volume's background task, ported from upstream, flushes files periodically and flushes at once under critical pressure. A VolumesDirectory excerpt enforces upstream's critical-pressure dirty limit. The five upstream tests are ported and pass, and an integration test shows real kernel pressure making a Vfs write back dirty pages without fsync.
<!-- SECTION:FINAL_SUMMARY:END -->
