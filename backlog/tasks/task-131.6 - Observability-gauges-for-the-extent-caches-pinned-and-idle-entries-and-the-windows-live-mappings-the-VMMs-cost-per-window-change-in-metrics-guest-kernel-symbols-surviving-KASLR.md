---
id: TASK-131.6
title: >-
  Observability: gauges for the extent cache's pinned and idle entries and the
  window's live mappings; the VMM's cost per window change in /metrics; guest
  kernel symbols surviving KASLR
status: In Progress
assignee: []
created_date: '2026-10-29 21:13'
updated_date: '2026-10-30 04:23'
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
- [x] #1 Each gauge and the VMM's cost exported, and a guest profile symbolized under KASLR
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

Host profile of a guest, symbolized (70471e017): fxfs-guest-pgbench with FXDAX_HOST_GUEST_PROFILE=DELAY:SECONDS runs perf kvm --guest record -a for SECONDS from DELAY, under timeout (given a command, perf kvm passes it its own options); perf kvm report leaves the guest's samples [g] 0xADDRESS (it records no relocation of the guest kernel), so the case resolves them against the guest's vmlinux (nm -n), as the text is where vmlinux links it, sums them by symbol into perf-kvm.txt, and fails unless an fxfs_ symbol resolves. GCE r261 (pgbench-dax, 20 s from 120 s): top guest symbols vm_interrupt 1.07%, fxfs_dax_evacuee_gather 1.06% (before 88ded8774), x2apic_send_IPI, vm_notify, rep_movs_alternative; passed. Also found: the guest test runner ignored start_vm's exit status, so a host side that failed or aborted passed if the guests' output matched; fixed (ab1370009), a wide set of 20 cases passing with it.
<!-- SECTION:NOTES:END -->
