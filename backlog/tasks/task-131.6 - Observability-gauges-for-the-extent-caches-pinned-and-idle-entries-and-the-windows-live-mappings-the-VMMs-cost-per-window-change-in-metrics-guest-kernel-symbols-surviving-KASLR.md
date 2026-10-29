---
id: TASK-131.6
title: >-
  Observability: gauges for the extent cache's pinned and idle entries and the
  window's live mappings; the VMM's cost per window change in /metrics; guest
  kernel symbols surviving KASLR
status: In Progress
assignee: []
created_date: '2026-10-29 21:13'
updated_date: '2026-10-29 22:02'
labels:
  - observability
dependencies: []
parent_task_id: TASK-131
priority: medium
ordinal: 477000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team round 8, the parts no task has: gauges for the extent cache's pinned and idle entries and the window's live mappings; the VMM's cost per window change in its /metrics; guest kernel symbols that survive KASLR (profiles and stacks symbolized). Latency histograms are TASK-111.6, tracepoints TASK-44.16, the build-commit stamp TASK-111.7.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each gauge and the VMM's cost exported, and a guest profile symbolized under KASLR
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-29 (on main): extentd /metrics gauges fxdax_extentd_extent_cache_pinned_entries and _idle_entries (crate::cache::entries: held by something but the cache, or not), fxdax_extentd_window_mappings, _window_mapped_bytes and _window_pages (crate::dax::live, kept as windows' states change); tests live_gauges_follow_the_windows_mappings_and_pages (macOS, Linux) and metrics_gauge_the_extent_cache_and_the_windows_mappings (Linux, OrbStack). The VMM's cost per window change is fxdax_extentd_stage_ns_total over fxdax_extentd_stage_ops_total, as its VM processes report it (STAGE_COUNTS), now documented. KASLR: the guest kernel's panics say 'Kernel Offset: disabled' (r206): Firecracker boots the vmlinux ELF at its 64-bit entry, past the decompressor that alone randomizes the text, so the build's vmlinux symbolizes host profiles of the guest as is; fxfs-guest now checks it (_text at 0xffffffff81000000 on x86_64), passing on GCE r261. A runtime booting a bzImage would randomize it: nokaslr there, or the guest's /proc/kallsyms for perf kvm --guestkallsyms. Open for AC1: a host profile of a guest actually symbolized (perf kvm --guestvmlinux).
<!-- SECTION:NOTES:END -->
