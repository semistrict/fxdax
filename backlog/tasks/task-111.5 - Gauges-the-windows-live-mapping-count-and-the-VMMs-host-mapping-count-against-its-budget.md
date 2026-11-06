---
id: TASK-111.5
title: >-
  Gauges: the window's live mapping count, and the VMM's host mapping count
  against its budget
status: Done
assignee:
  - '@claude'
created_date: '2026-10-28 01:25'
updated_date: '2026-11-06 04:38'
labels: []
dependencies: []
parent_task_id: TASK-111
priority: medium
ordinal: 419000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sandbox team sampled /proc/<pid>/maps every 2 s to see these (TASK-111). As of 2026-11-05 the metrics had all windows' gauges together (fxdax_extentd_window_mappings, _mapped_bytes, _pages, _private_pages, TASK-131.6) and the extent cache's pinned and idle entries (fxdax_extentd_extent_cache_pinned_entries / _idle_entries, which covered TASK-72's ask), not each VM's. The VMM's host mappings are the runs of pages its window's uffd second stage keeps installed (dax::uffd), each a mapping vm.max_map_count limits; their budget, half of what the process may map beyond 4,096, the VMM derives from the max_map_count extentd sends in OPEN's reply since TASK-172. Writable DAX's deletion removed no gauge named here.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 extentd /metrics exports each window live mapping count
- [x] #2 extentd /metrics exports each VMM host mapping count and its budget
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. dax: each window's gauges (mappings, mapped bytes, pages, private pages) as atomics of its own (dax::Gauges, DaxWindow::gauges), updated where WindowState changes; all windows' totals (dax::live) summed over a registry of them; a window's fall to nothing as it goes.
2. VMM: STAGE_COUNTS and ACK carry the runs its uffd second stage has installed (args[4], UffdSecondStage::installed); WINDOW carries their budget (args[2], from the max_map_count extentd sent in OPEN's reply, TASK-172).
3. extentd /metrics, per VM process that opened a store, labels branch and connection: fxdax_extentd_vm_window_mappings, _mapped_bytes, _pages, _private_pages (with a window), vm_host_mappings (as last said), vm_host_mapping_budget (as said). GetSnapshot (TASK-111.4) reports them, fresh.
4. Tests: dax per-window gauges, held past their window; extentd metrics per VM, writer and follower apart; uffd installed() against /proc/self/maps.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in b772af8da. Verified: dax::tests::live_gauges_follow_the_windows_mappings_and_pages (each window's gauges, adding up to all; zero once the window goes, held or not), extentd::tests::metrics_gauge_each_vm_processs_window_and_host_mappings (writer and follower apart; no series before a window; budget as the VM process said; host mappings as of a snapshot), dax::uffd::tests::windows_keep_their_mappings_within_the_budget now through installed(), which it compares with /proc/self/maps; on GCE r681 a live VM's snapshot showed host_mappings 1 against a budget of 522,240 (docs/measurements/fxdaxctl-stats-2026-11-05). Same runs as TASK-111.4. /metrics shows the VMM's installed runs as it last said them (with each ACK and, while it makes stage operations, every 500 ms), not asked per scrape: decision in OPEN_QUESTIONS.md.

Rebased 2026-11-06 onto origin/main 5f18867c5 (TASK-187, TASK-190, TASK-141.1 and others): fxdax 82e0eb8fc; vendor/firecracker 96cb97976 on the fork's e64c4b283 (branch fxdax-t1114; its Cargo.lock keeps fxdax's url, which the fork's 8b1a45e6a lacked). OPEN_QUESTIONS.md's conflict, both sides' decisions kept. Reruns: the device's tests (vmm extent::) 16/16, Linux arm64 Docker, built --locked; macOS workspace 2119/2119 and the shuttle parts; Linux Docker 2303/2310, the 6 known Docker-only failures (eu-stack stacks, grants as root) and extentd vm_processes_that_leave_replies_unread_are_let_go, which failed once under the full run's load (the daemon hung up while the test still sent its READs: its send unwrapped a broken pipe) and passed 40/40 alone. Supersedes the SHAs above (b772af8da, dcecf56a7).

Rebased 2026-11-06 onto origin/main 6754d1469 (TASK-160.7's one table of request types): fxdax 7a0f88f44; vendor/firecracker unchanged, 96cb97976. STATS is a row of the driver's vext_types[] (vext_args_stats from req->offset, data out of data_len bytes, not writing the store), which replaces the four switches and the request_names array (whose resizing went with it); its sysfs loop runs to VIRTIO_EXTENT_T_LAST, 17, over a table of T_LAST + 1. Reruns: guest kernel built in Docker (arm64), check-guest.sh arm64 ok; the device's tests (vmm extent::) 16/16 --locked; Linux Docker fxdax 2304/2310, the 6 known Docker-only failures; GCE r682 (us-west2-a, Firecracker jailed) fxfs-guest-stats, fxfs-guest, fxfs-guest-dax passed, the snapshot's three builds 21a094a79, the VM's answer in 1.1 ms. Supersedes the SHAs above.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
extentd's /metrics gauge each VM process's window (mappings, bytes, pages, private pages) and the runs its second stage has installed in its memory against their budget, labelled by branch and connection; each window's gauges are its own atomics, all windows' their sum. Verified by dax, uffd and extentd metrics tests and a live VM's snapshot on GCE r681.
<!-- SECTION:FINAL_SUMMARY:END -->
